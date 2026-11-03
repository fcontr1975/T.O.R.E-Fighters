# Controllers and pilot input

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

This is an authored T.O.R.E input layer, not recovered retail controller dispatch.
Keyboard, gamepad, stick, throttle, pedals and button-box controls share typed
pilot actions. The original instruments and their scope operations are driven
through those same actions. This layer adds no external screen export, no window
rearrangement, no sensor behaviour of its own and no fabricated readings; the
shared [sensor component](radar.md) decides what the scopes show.

## Cockpit lighting

Ctrl+I then -/+ adjusts instruments and cockpit art. Ctrl+Shift+F then -/+ adjusts
red flood lighting. These chords expire after four seconds or another key;
Ctrl may stay held. Ctrl+H cycles HUD colour, while Shift+[ / Shift+] retain
retail HUD brightness. Controls > View lists the selectors and separate
bindable dim/bright actions. [Levels and defaults](spec/cockpit-lighting.md).

## Quick start

On Choose Activity, Tab or the menu arrows reach **Leave Feedback** and, when
shown, **Update Available**. Enter/Space activates the focused item; controller
menu directions and Accept do the same. The saved check toggle is in Pref.
See the [project-link contract](spec/main-menu-presentation.md#updates-and-project-feedback).

Every default key, mouse input and gamepad button is in the
[controls master list](CONTROLS.md). On Linux a controller exposing the standard
two-stick gamepad controls receives the following default mapping. The 8BitDo Ultimate 2 wireless controller's input
capabilities have been inspected on the development host, and the user confirmed
the Linux rumble test pulse works. Flight handling and disconnect checks remain open.

```sh
cargo run --locked -p tore-app -- --free-flight
cargo run --locked -p tore-app -- --list-inputs
cargo run --locked -p tore-app -- --monitor-inputs 30
```

| Standard Linux control | Default flight action |
| --- | --- |
| Left stick | Roll / pitch; pulling the stick back pulls the aircraft up |
| Right stick | Head-look / exterior orbit |
| Left / right trigger | Left / right rudder; equal pulls cancel |
| Left / right shoulder | Decrease / increase throttle while held |
| South / east face button (A/B on Xbox layout) | Gear / airbrake toggle |
| West / north face button (X/Y on Xbox layout) | Flaps / afterburner toggle |
| View (Select) / left-stick click | Combat modifier / forward cockpit view |
| Start | Pause/resume |
| Guide | Escape flight menu, if not intercepted by the desktop |
| Right-stick click | Recenter look, including a head tracker |
| D-pad left/right | Previous/next instrument selection |
| D-pad up/down | Selected instrument's first/second stock button |
| D-pad and south/east buttons in menus | Navigate and accept/back |

O toggles the F-22 main bays, as FA's bomb-bay key does. The `bay` action is available in the controls
editor, text profiles and recorded input. Other aircraft ignore it.

Keyboard assignments continue working. Standard keyboard axes have priority over
controller axes while pressed. These gamepad defaults are convenience mappings,
not a claim about FA's original joystick layout. Back/paddle/extra buttons are
available only if firmware/driver exposes them as independent inputs. Devices
with different descriptors, including Windows/macOS raw controller numbering,
need a custom profile; no universal button-index layout is assumed.

`--no-controllers` disables native device discovery and feedback while retaining
keyboard controls and any custom keyboard bindings. It also allows deterministic
bridge tests without touching hardware.

## VTOL, tiltrotor and helicopter controls

The six powered-lift aircraft share the ordinary stick and rudder, and some
stock keys change meaning with the aircraft flown. John chose the retail nozzle
keys and the collective on the throttle controls on 2026-10-08 (VTOL overhaul
decisions 4 and 5); the trim keys, the stability chord and the gamepad layer
are agent decisions, made after checking the defaults for clashes. Every other
aircraft keeps its keys. The flying guide is
[flight controls](FLIGHT-CONTROLS.md#vtol-tiltrotors-and-helicopters).

| Key | AV-8, Yak-141 | Helicopters (AH-64, Mi-24, CH-47) | V-22 | Every other aircraft |
| --- | --- | --- | --- | --- |
| Z / X | Nozzles up / down 10 degrees | Rudder | Rudder | Rudder |
| Shift+Z | Nozzles to 0, or from the 100-degree braking stop to 90 | Rudder (as Z) | Rudder (as Z) | Rudder (as Z) |
| Shift+X | Nozzles to 90, again to 100 | Rudder (as X) | Rudder (as X) | Rudder (as X) |
| End / Page Down | Rudder | Rudder | Rudder | Rudder |
| Ctrl+Up / Ctrl+Down | Nozzle slew | Cyclic trim forward / aft | Cyclic trim forward / aft | Nothing |
| Ctrl+Left / Ctrl+Right | Nothing | Cyclic trim left / right | Cyclic trim left / right | Nothing |
| 0 | Nozzles to 0 | Trim to centre | Nacelles forward | Nothing |
| 1-8 | Engine throttle | Collective | Collective (thrust control lever) | Engine throttle |
| Ctrl+Page Up / Ctrl+Page Down | Nothing | Nothing | Nacelles toward airplane / helicopter | Nothing |
| Ctrl+Home / Ctrl+End | Nothing | Collective up / down | Collective up / down | Nothing |
| Ctrl+Alt+A | Hover hold (refused) | Hover hold | Hover hold | Hover hold (refused) |
| Ctrl+Shift+A | Stability level | Stability level | Stability level | Nothing |

How the aircraft-dependent keys work:

- **Contextual stock keys.** `crates/tore-app/src/input_catalog.rs` lists them
  (`CONTEXTUAL`): each says on which aircraft its action is live, and the input
  layer switches the stock bindings with the aircraft. A key never has two
  meanings on the same aircraft; the catalog test checks it. A player's own
  binding on one of these keys turns the aircraft-dependent stock meaning off,
  so it wins. Removing such a key in the controls screen removes it from every
  aircraft, because a removed stock key is removed by key.
- **Throttle controls drive the collective** on the helicopters and the V-22,
  whose engines are governed: the 1 to 8 keys, a bound throttle lever and the
  gamepad throttle buttons become collective settings, steps and rates in the
  input layer, so the simulation still receives an explicit collective and the
  wire is unchanged. A throttle lever picks up against the collective. The
  dedicated `collective` lever and Ctrl+Home / Ctrl+End stay, and a bound
  collective lever takes priority over the throttle lever.
- **Trim keys** (`trim-pitch-rate`, `trim-roll-rate`, `trim-pedal-rate`) move
  the trim a 2 percent tap at once and then, held past 0.2 s, 10 percent of
  travel a second in 20 Hz steps (fitted). Each step is a trim command, so the
  host, recordings and tapes see it. At the Attitude stability level the
  cyclic trim keys move the attitude the aircraft returns to instead, 5
  degrees a second. `trim-set` (no keyboard default; bind a stick button) makes
  the current stick plus trim the trim and ignores the stick until it is back
  within 5 percent of centre: force trim on a spring-centred stick.
  `trim-centre` clears it.
- **Stability level.** `stability-level` cycles Off, Damper and Attitude and
  `stability-level=off`, `=damper` and `=attitude` choose one; Damper is the
  default (John, 2026-10-08). The choice is announced (`Stability: Damper`),
  saved as a preference (Pref → Stability level, which also cycles it) and
  applied to every powered-lift flight, restart and seat as a pilot command.
- **Hover hold** (`hover-hold`) is the autopilot mode of the helicopters and
  the V-22 (VTOL overhaul slice P9). Ctrl+Alt+A is the stock key; on some
  European layouts Windows reports AltGr as Ctrl+Alt.

The gamepad keeps its layer:

| Control | Keyboard default | Standard Linux gamepad default |
| --- | --- | --- |
| Nozzle pitch, forward / vertical | Ctrl+Up / Ctrl+Down (vectoring jets) | Hold Select, right stick forward / back |
| V-22 conversion, airplane / helicopter | Ctrl+Page Up / Ctrl+Page Down | Hold Select, right stick left / right |
| Collective, increase / decrease | Ctrl+Home / Ctrl+End, or the throttle keys | Hold Select, right stick up / down, or the throttle buttons |
| Nozzles/nacelles forward neutral | 0 (not on helicopters) | Hold Select, D-pad up |
| Engine on/off | E | Hold Select, Start |
| Engine power (vectoring jets) | 1-8 throttle presets and steps | Shoulder buttons |

Pitch, roll and rudder keep their ordinary arrows and End / Page Down
assignments. The right-stick additions use the selected
aircraft's modifier map.
Combat shoulder, face, countermeasure and click bindings retain their actions.
Powered-lift engine and forward-neutral commands use the contextual Start and
D-pad Up alternatives below. The left stick stays live during collective
adjustments. Shared right-stick axes act according to aircraft capability.
Added modifier mappings apply only to aircraft with the relevant controls.
Other aircraft retain their ordinary look, rudder and modifier bindings.
Automatic gamepad rows use an `auto-gamepad:` device alias so this distinction
survives profile saves. Explicit user bindings use ordinary device identities or
user aliases and always override automatic rows on the same control.
On powered-lift aircraft Select+Start toggles the engine; on vectoring jets and
tiltrotors Select+D-pad Up returns nozzles/nacelles forward. These contextual
bindings replace the developer fixture alternatives on those aircraft.
Center a stick after changing the modifier layer;
focus loss or pause requires release before controls become active again.

**Flight controls** lists separate absolute lever, rate-axis and decrease/increase
rows for nozzle pitch, nozzle yaw, V-22 conversion and collective. Absolute
lever positions use 0..1, except signed nozzle yaw, which uses -1..1. Axis rows
support the editor's deadzone, sensitivity, inversion and response curves;
min/center/max calibration remains a profile-file setting. The tokens are
`vector-pitch`, `vector-yaw`, `conversion`, `collective` and their `-rate`
forms. Unsupported aircraft ignore the added demands. Releasing a rate input
holds its last requested position. Neutral resets conversion and both nozzle
axes, and leaves collective and power unchanged. Digital position/reset commands
release an absolute lever's ownership until it moves at least 2% of calibrated
travel. This fitted host rule prevents a stationary HOTAS lever from immediately
undoing the command, including after pause or focus loss. Defaults and limits reset
with the aircraft state on restart or seat handoff.

The [VTOL / Helicopters keyboard sheet](tore-keyboard-map.html#lift) shows these
controls together with ordinary attitude, engine, gear and brake keys. Pilot
input tapes write version 3 (`tore-pilot 3`), which adds the trim, stability,
nozzle-key and hover-hold commands; version 1 and 2 tapes remain readable, and
reject those words. Old tapes of the six powered-lift aircraft do not reproduce
once their physics changes.

The Apache and Chinook keep their fixed landing gear down; G reports that the
gear is fixed. The Hind retains retractable gear. This is an explicit
[fitted equipment rule](spec/variety-flight.md), with no new binding.

## AC-130 directed gun groups

Ctrl+7 selects the next gun candidate; Ctrl+8 links or unlinks it. Hold Select
and press the gamepad D-pad up or down for the same actions (they sat on the
right stick before the gunsight took it, John 2026-10-09).
These gestures act on AC-130 only. The Weapons instrument shows the authoritative
`LINK` membership and marks the current candidate with its ordinary selection
arrow. Fire releases every enabled member together, each retaining its own
ammunition and cadence; it needs no target or firing solution. Empty or
blocked members do not stop the others. An empty group
reports GROUP EMPTY. Bracket selection retains ordinary single-gun operation
until multiple guns are linked; NAV suspends group fire.

Mounts follow the gunsight's aim point (a target, a pin or the free line of
sight) at 30 degrees/second and shots use their actual barrel directions and
muzzle positions. NO TARGET, CANNOT BEAR, SLEWING, MAX RANGE and TERRAIN MASK
only say the shot is not solved; the guns still fire. SAFE, EMPTY, GROUP EMPTY
and NO LINE OF FIRE (the gun's own airframe) are what stop a gun.
The [gunship contract](spec/ac130-linked-guns.md) records source arcs, fitted
articulation/clearance, limits and shared host/client/replay state. These controls
and the linked firing capability are authored choices, not recovered retail
key assignments or original linked fire behaviour.

### AC-130 gunsight controls

The gunsight is the AC-130's target camera. These keys were chosen by John on
2026-10-09; the behaviour behind them is the AC-130 gunsight project's, and the
[controls list](CONTROLS.md) has every default. Since the air-to-ground slice T1
the same keys drive the targeting sight of every aircraft with a laser
designator or a FLIR ([air-to-ground](spec/air-to-ground.md#the-targeting-sight));
on an aircraft without one, Backslash, Shift+Backslash and the zoom keys say "no
targeting sight". The gamepad gestures stay the AC-130's.

| Action | Keyboard | Gamepad | Notes |
| --- | --- | --- | --- |
| Designate what is under the crosshair | Backslash | Select + A, tapped | An object if one is there, else the ground. Retail's Backslash is "designate the IR/laser target": every aircraft with a sight |
| Next visible ground object | / | None | The first press takes the one nearest the line of sight, then each press steps right, wrapping to the leftmost |
| Laser on or off by hand | Ctrl+L | None | Aircraft with a laser designator; the laser also comes on by itself with a laser weapon selected |
| Pin the ground under the crosshair | Shift+Backslash | Select + A, held half a second | |
| Drop the target or pin; again, back to the default view | L or ; | Select + B | The existing clear-designation keys |
| Slew the sight | Alt + arrows (or Alt + keypad 4, 6, 8, 2) | Select + right stick | Actions `sight-left`, `sight-right`, `sight-up`, `sight-down` for keys and buttons; `sight-x` and `sight-y` for sticks and HOTAS axes (right and up positive) |
| Zoom in / out | Shift+' / Shift+; | None | Retail's bomb camera zoom keys; six steps |
| Next gun / link gun | Ctrl+7 / Ctrl+8 | Select + D-pad up / down | Moved from the right stick |
| Pin or release (console: orbit or search) | P | None | AC-130 only (`sight-pin-toggle`): pins the ground under the crosshair with the sight free, else lets the pin or track go, as L does. In the sensor console that is ORBIT or SEARCH. Agent's key choice, 2026-10-14 (P is free in the original, docs/spec/keyboard.md) |
| Sensor console and orbit hold | Ctrl+9 | Select + Guide (Xbox) | AC-130 only (`sensor-console`): opens the console; the autopilot searches straight and level with the sight free, or circles the sight's pin or track; again hands the aircraft back ([the sensor console](spec/ac130-linked-guns.md#the-sensor-console-and-the-orbit)). Agent choice of keys, 2026-10-13: next to the gun keys; Select + Guide was the range's incoming-missile fixture, which is off on the AC-130 |
| Sensor picture | Ctrl+Backslash | None | The AC-130 sensor console only (Ctrl+9; [sensor console](spec/target-window.md#sensor-console)): white hot, black hot, TV and night vision in turn; does nothing elsewhere. Action `sight-polarity`; agent's key choice beside the other Backslash sight keys (2026-10-13), the two further modes John's (2026-10-14) |
| Sensor picture, direct | Shift+F5 / F6 / F7 / F8 | None | The AC-130 sensor console only: straight to white hot / black hot / TV / night vision; nothing elsewhere. Actions `sight-whot`, `sight-bhot`, `sight-tv`, `sight-nvg`; agent's key choice (nr-replayviews, 2026-10-16, John asked for a shortcut to each picture): Shift with the function keys is free in flight. The replay's Sensor View reads the same keys |
| Console radar range | Ctrl+0 | None | The AC-130 sensor console only: the A-G radar's range, 5, 10 and 25 nm in turn; does nothing elsewhere. Action `console-range`; agent's key choice beside Ctrl+9 (2026-10-14). In the console T and Shift+T step through the radar contacts the sensor sees |

**Inside the sensor console** (slice nr-cona; John asked for slew, zoom,
pin/release and heading, altitude and throttle keys on 2026-10-14; which keys
is an agent decision). The stick does not fly the aircraft there, so flight
controls take console meanings, with no new bindings to learn:

| Control | In the console |
| --- | --- |
| Arrows, the stick (gamepad left stick), mouse look (right button held) | Slew the sight (the arrows with the hold ramp below; the mouse one field of view per 1,000 pixels at sensitivity 1, capped at the full slew rate) |
| = and -, the mouse wheel | Zoom the sight a step |
| Shift + Left / Right (the look keys), the look axes | Step the search heading 5 degrees |
| Shift + Up / Down, the look axes | Step the search altitude, or the orbit's height above its point, 500 ft |
| 1 to 6, 7 and 8 (and their bound buttons) | The autopilot's speed: 0 to 100% of the corner-to-maximum band, then 5 kt steps |
| P | Pin or release: ORBIT or SEARCH |
| Shift+P (`gun-lock`) | Gun lock: the guns stay on the target or point while the sensor slews free; again lets them follow the sensor. Agent's key choice, 2026-10-14, beside P (Shift+P is free in the original) |
| Ctrl+Insert / Ctrl+Delete (`console-chaff` / `console-flare`) | Chaff / flare: the console's own actions, bound apart from Insert and Delete (John, 2026-10-14); the keys are an agent choice, the Ctrl versions of the retail chaff and flare keys. They work outside the console too |

**The gamepad in the console** is John's layout (2026-10-14). While the
console is open it owns every standard gamepad completely, whatever the
profile (stock defaults on or off, the player's own gamepad bindings): the
layout (`CONSOLE_PAD` in `crates/tore-app/src/input.rs`) is added for each
connected gamepad and every other binding of that gamepad stands aside, the
triggers' rudder, the throttle buttons and the flight switches included.
Only menu navigation, Start (pause) and Xbox (the flight menu) keep working
(John's bug of 2026-10-14: "make sure all sensor override controls take
complete control of the controller"). A console action the player bound on
their gamepad themselves wins: the layout's binding of that action, and the
layout's binding on that control, stand aside. The triggers do nothing in
the console (agent decision: John's layout fires on A). The layout is never
saved into the player's controls file. Closed, the gamepad's flight bindings
are back. A joystick or HOTAS keeps its own bindings, and the keyboard is
unchanged.

| Gamepad | In the console |
| --- | --- |
| View + Xbox | Open and close the console |
| Left stick | Slew |
| Left stick press (L3) | Pin or release |
| Right stick | Left and right: heading; up and down: altitude or orbit height |
| D-pad up / down | Speed up / down |
| A (held) | Fire |
| Y | Next target |
| View + Y | Drop the target |
| View + D-pad right | Previous target |
| X | Select gun (next candidate) |
| B | Link gun (link or unlink the candidate) |
| LB / RB | Zoom out / in |
| D-pad right | Sensor radar range 5 / 10 / 25 nm |
| D-pad left | Camera mode: white hot, black hot, TV, night vision |
| View + A / View + B | Flare / chaff |
| View + right stick press (R3) | Gun lock |

A held heading or altitude key steps at once, then every 0.15 s after 0.3 s.
The view does not turn in the console. Each key reaches the host as the seat
command `Console` ([the console's keys](spec/ac130-linked-guns.md#the-consoles-keys)).
Rebinding the arrows, the look keys, the zoom or the throttle keys moves the
console meanings with them, since the console reads the flight actions.

A held slew key or button starts at a quarter of full deflection for a quarter
second, so a short press nudges, then runs at full rate; an analog axis is
proportional. The seat input carries the deflection (`sight`, -127 to 127 on
each axis) and the zoom step (`sight_zoom`, 1 to 6, 3 at the start of a flight
and of each new plane); the host turns them into degrees, so single player and
multiplayer slew alike.

A hat (such as a HOTAS point-of-view switch) binds to the four `sight-` holds,
one direction each, using its virtual button tokens: `bind stick axis:16=-1
sight-left hold`. Hat positions are not `position=N` bindings for these
actions, because a slew is held, not pressed.

Select + A stays the radar `designate` on every other aircraft; the AC-130
swaps in the sight's designate and pin. The Select + D-pad uses that belong to
the vectoring jets, the range reset and the damage test are off on the AC-130,
where the D-pad picks and links guns, and so is Select + Guide's incoming-missile
fixture, where Guide opens the sensor console. The live-fire range reset, which took
Backslash, is now Ctrl+Shift+Backslash.

On [the keyboard map](tore-keyboard-map.html) the sight keys are on the Fly &
Fight sheet, with a sight legend box that also lists the AC-130's gamepad
gestures; Tab (the gun), Ctrl+M (delivery mode) and Ctrl+R (the ground radar)
are on that sheet too. The range reset is a developer fixture and stays off it,
as the [map's rules](tore-keyboard-map-rules.md) require.

## In-game controls and saved preferences

**Alt-Enter** switches between native borderless fullscreen and the previous
windowed size, on every screen: the menus, the Quick Mission creator, the locate
screen, flight and the replay viewer. It is not rebindable and no controller
button is assigned to it. F11 is not used, because it already opens the
in-flight keyboard help. The game starts in borderless fullscreen; the choice is
saved as `fullscreen` in `preferences-v1.conf` and `--windowed` starts one run
in a window without changing it.

### The controls screen

Open **Pref → Controls...** on the main menu, **Escape → Control** in flight, or **Escape → Control** in the replay viewer.
Both open the same input configuration screen; in flight the game stays paused.
The layout follows John's 2026-09-22 mockup; its details are an opinionated agent
design drawn with the imported raster font. Every default is listed in the
[controls master list](CONTROLS.md).

- **Devices on the left.** Keyboard, Mouse, each detected controller, stick,
  throttle, pedal set or button box, any disconnected device the profile still
  names, and Head tracker. The device type is guessed from its name and controls
  (a fitted rule): it only picks the tab title and which settings appear.
- **Settings at the top right**, per device. Controllers: rumble, modifier
  buttons, deadzone, stick sensitivity, trigger sensitivity and defaults for new
  gamepads. Sticks, throttles and pedals: modifiers, deadzone and sensitivity.
  Mouse: mouse look, sensitivity and invert. Head tracker: see
  [head tracking](#head-tracking-and-trackir). Deadzone and sensitivity apply to
  every axis binding on that device; sensitivity scales the axis output from 0.10
  to 1.00. Trigger sensitivity scales trigger (`trigger-*`) bindings the same way.
- **Mappings below**, grouped as Flight controls, Systems, Weapons, Sensors and
  instruments, View, Communication and Game and menus. Click a group header to
  fold it. Each action has a primary and secondary input; click one to capture
  a replacement and operate the control. Axis rows also have an invert box and a
  response curve button (1.0, 1.5, 2.0, 3.0). The invert box has no label, so
  resting the pointer on it for about half a second shows a small tooltip,
  `INV: invert this axis`, below it (above on the last rows, clipped inside the
  screen, never over the box; John, 2026-10-23). Moving off, clicking or
  scrolling hides it. **Clear** removes the action's
  inputs on this device; Delete or Backspace clears only the focused input.
  Profile bindings for actions the screen does not list appear under
  **Other bindings**, where they can be cleared.
- **Find**, between the mappings title and the list (John, 2026-09-28),
  narrows the list for the selected device. **Actions** matches action names
  and group titles, so `gear` finds Landing gear and `weapons` lists the whole
  Weapons group: commands to bind. **Keys** (Inputs on other devices) lists
  the actions an input is already bound to. Type its name as the rows show it
  (`page` finds Page Up and Page Down; a short name such as `f` or `lb` must be
  a whole word, so `f` finds F and Ctrl+Shift+F but not F1), or click **Press** and
  press it. A pressed keyboard key matches exactly, modifiers included. A
  pressed button, D-pad direction or axis on a controller matches every
  binding that uses it, including combinations where it is the modifier.
  Folded groups open while a search is active. Ctrl+Shift+F jumps to the field;
  Backspace edits, Delete or **Clear** empties it, and Esc empties it before
  it backs out of the screen. Typed text stays when you change device; a
  pressed input is dropped, because it names a control on the old device.
  There is no search on the Head tracker tab.
- **Keyboard remapping is complete.** Stock keys show as the primary input and can
  be changed or cleared. A key given to one action is taken from its previous
  action, and the message names it. Giving an action back its own stock key simply
  re-enables it. Esc, Alt+Enter and Alt+F4 cannot be taken.
- **Capture** picks the behaviour from the row: a stick or axis on an axis row,
  a button or D-pad direction on a direction row (for example Rudder left), a
  trigger moved on a direction row becomes a trigger contribution, and a trigger
  pushed on a button action (such as Fire) acts as a button once past halfway. A
  stick moved on a direction row is assigned to the whole axis row instead. Esc
  cancels capture.
- **Apply** validates and saves before replacing live bindings. **Reset device**
  restores the selected device's defaults in the draft. **Back** or Esc discards
  anything not applied. Arrow keys, Tab, Page Up/Down and Enter navigate; a
  controller's D-pad, A and B do too, except while capturing.

**Click to cycle** (John, 2026-10-23): on this screen a left click turns a value
to its next one and a right click to the one before, as in the Quick Mission
creator and the lobby's Settings. The response curve button wraps at both ends
(1.0, 1.5, 2.0, 3.0, then 1.0; backward, 1.0 goes to 3.0). The invert box and
the on/off settings (rumble, defaults for new gamepads, mouse look and invert,
head source and invert) flip in either direction. A stepped value (deadzone,
sensitivities, head scales) steps down on a right click and stops at its ends,
as the left third of its box does. A right click needs its press and release on
the same control, does not start the modifier capture, and is left to the
capture while an input is being captured (the right button is then the input
captured on the Mouse tab). Keyboard and controller keep their keys: Enter
turns a value forward, Left and Right step a stepped setting; there is no
reverse key for the curve button.

The editor saves the explicitly loaded `--input-profile` file, or `input-v1.conf`
in the [application data directory](DEVELOPMENT.md). It writes a canonical profile
(comments/formatting are not retained), using a temporary file and rename so a
failed write does not truncate the current file. Save errors appear in the editor;
invalid drafts do not replace either the file or live bindings.

**Default mappings for new gamepads** preserves automatic standard Linux mappings
when enabling rumble before connecting a pad. Turn it off for entirely explicit
profiles. The version-1 directive is `gamepad-defaults on|off`; absent means off
for existing custom profiles. Known device mappings already in the profile are
retained instead of appending another default set. With defaults on, removing all
bindings for a device lets it receive defaults again when reconnected.

macOS GameController capture explicitly uses shared `*` bindings because its
public identity is session-only. Native generic HID identities and Windows/Linux
identities keep their normal matching rules. The screen covers the current
binding model, not a calibration wizard, persistent Apple player assignment,
device firmware remapping or unsupported aircraft systems. Per-binding priority
and min/center/max calibration remain profile-file settings; the screen keeps
whatever the file holds.

### Modifiers

A modifier is a held control that switches its device to another layer of
bindings, like Ctrl or Shift on a keyboard. A device can have up to four. Select
the **Modifier buttons** row with a click, Enter or the controller's A button,
then press a button or D-pad direction: a new one is added, and one that is
already a modifier is removed. Left and right on the D-pad do nothing on that
row, so moving through the settings never starts a capture. Esc cancels. Any button
or a single D-pad direction can be a modifier, so D-pad left and D-pad right can
be two separate modifiers. While capturing, hold one or two modifiers and press
the control: `View + D-pad left + A` is a valid binding. When several held
modifiers match, the binding naming the most of them wins.

A declared modifier is dedicated: its own unmodified bindings stop acting, so
declaring D-pad left removes its Previous instrument and menu-left actions.
Standard gamepad defaults declare View (Select) as the combat modifier. Keyboard
modifiers are simply Ctrl, Alt and Shift in any combination, such as Ctrl+G,
Ctrl+Shift+G or Ctrl+Alt+G.

### Mouse look

Hold the right mouse button and drag to look around in flight. Sensitivity 1.0
turns the view 2 radians per 1,000 pixels of mouse travel (an opinionated agent
value); the range is 0.1 to 5.0. Mouse look obeys the same limits as the
keyboard and stick: in the cockpit you cannot look below the forward eye line.
Turn mouse look off to bind the right button to an action instead. The middle,
back and forward buttons and the wheel are bindable. The wheel zooms by default.
The left button always operates instruments and the HUD.

With Pref → Debug panels? on, a right-click that does not drag (released within
4 pixels of where it was pressed) opens the
[debug menu](REPLAYS.md#the-right-click-menu), and dragging is still mouse
look. A right button bound to an action, with mouse look off, keeps its
binding and never opens the menu. The left button and the wheel operate the
debug panels and menu when the pointer is on them.

### Head tracking and TrackIR

The game listens for head poses on loopback UDP port 4242 in opentrack's "UDP
over network" format: 48-byte datagrams of six little-endian doubles, x/y/z in
centimetres then yaw/pitch/roll in degrees. The listener binds `127.0.0.1` only,
so it needs no firewall permission and accepts no remote sender. The head angle
is added to the player's look angle, the way the right stick and mouse look turn
the view, and the result obeys the same cockpit limit: you cannot look below the
forward eye line. **Center view** (Shift+/, or right-stick press) makes the
current head position straight ahead. Head position and roll are not used; zoom
stays on its own controls.

TrackIR's own software only sends data to games registered with NaturalPoint,
so T.O.R.E does not talk to it directly. Run [opentrack](https://github.com/opentrack/opentrack)
with its TrackIR input (or a webcam or phone tracker) and the "UDP over network"
output set to `127.0.0.1` port 4242. Trackers that appear as a joystick instead can
bind the **Head tracker yaw** and **Head tracker pitch** axis rows on that
device's tab; full axis travel maps to plus or minus 180 degrees of yaw and 90
degrees of pitch before the head sensitivity setting.

Head tracker settings: receiver on or off, yaw and pitch sensitivity (0.1 to
3.0) and inversion for each. The sign convention is fitted: opentrack's positive
yaw is taken as a turn to the left. It has not been checked against real
TrackIR hardware; use the invert settings if a direction is reversed. A port
already in use is reported in the Head tracker status line and on the console.

### Profile directives for these settings

```text
modifier DEVICE CONTROL         # up to 64 in total; CONTROL may be a D-pad direction
disable keyboard KEY            # a stock key the player removed
disable replay-keyboard KEY     # a stock drone key removed only in replays
disable replay-mouse CONTROL    # a stock drone mouse input removed only in replays
disable mouse wheel:up          # a stock mouse binding the player removed
mouse-look on|off               # default on
mouse-sensitivity 0.1..5        # default 1
mouse-invert on|off             # default off
head-tracker off|udp:PORT       # default udp:4242, ports 1024..65535
head-scale YAW PITCH            # default 1 1; 0.1..3, negative inverts
```

Existing profiles without these lines keep the defaults. Older builds cannot read
a profile saved with them.

Normal sessions also save `preferences-v1.conf`: large/small instrument page sets,
active layout/selection, scope settings, cockpit/HUD/ladder visibility, the
weapon diagnostic panel (`weapon-diagnostics`, off by default), HUD
brightness and zoom. The file format is now **version 7**; version 7 moved
music and effects to the Sound/Music Prefs file `sound-v1.conf`
([spec](spec/sound-prefs.md)), and older files still load. The
retired `radar-mode` and `rwr-range` keys are gone (the RWR follows the shared
radar range), `rcs-range`, `radar-channel` and `radar-history` are present, and
`fullscreen` stores the window mode, so the exposure page's scale, the selected
sensor channel and the history toggle persist along with the shared scope range.
Version 1 to 4 files still load and drop a saved `rwr-range` after validating
it; they, and a version 5 file written before `weapon-diagnostics` existed, load
with the diagnostic panel hidden. Version 1 and 2 files also have their retired scope mode validated and
dropped, and their saved scope range migrates by its old nautical-mile value to
the nearest current setting, with equal distances choosing the lower one. That
maps 10 to 10, 20 to 25, 40 to 50, 80 to 100 and 160 to 150 nautical miles. An
old index is never reinterpreted as a different range. These persist across
launches, aircraft changes and flight restarts. Pause, head-look and aircraft state are not restored as user
preferences. Preference loading is silent; a malformed file is reported and
preserved. Smoke/capture/performance diagnostics ignore these display preferences
and do not write them, keeping existing visual probes reproducible. Explicit
instrument layout/page and zoom flags override saved values for normal launches.

Hybrid spin handling uses continuous pitch and rudder axis values for torque
and nose response, including fine deflections. John requested analog controls
as the baseline on 2026-09-16. Keyboard values feed the same flight model; they
do not define a separate all-or-nothing recovery law. Existing calibration,
deadzones and response curves remain user-configurable. See
[spin dynamics](spec/spin-transitions.md) for the fitted flight response.

## Key placement

Default keys follow Fighters Anthology wherever the original has the command
([keyboard spec](spec/keyboard.md)). John asked for the retail keys on
2026-09-26. The placement rules below are an agent decision from the same day:

- A command FA has uses FA's key, even where T.O.R.E's version of the feature is
  still fitted.
- A T.O.R.E-only command uses a key FA leaves free. Where one sat on an FA key,
  it kept its letter and gained Ctrl+Shift (tower repeat and cancel, the
  incoming-missile range fixture), or moved to a free neighbour (next airport,
  to Shift+N).
- An FA key whose feature T.O.R.E lacks stays unassigned. Pressing it says "not
  implemented yet": Shift+A and Shift+G
  (AWACS and air-to-ground radar links), Shift+F (target damage), Shift+I
  (airbase inventory), Shift+J (drop tanks) and Shift+D (message history).
  Alt+F (engage the IR or laser target) and Alt+W (engage every target of the
  designated target's class) are built, as the wing orders onto the ground
  below. U (IFF) and Ctrl+T (Show Target
  Info) work now, and the multiplayer keys K and Alt+Shift+E, W, B and H sit on
  keys FA leaves free ([controls](CONTROLS.md)).
- The keypad follows FA whatever NumLock says: 0 and period release chaff and
  flares, 1 and 3 are the rudder, 8, 2, 4 and 6 are the stick, 5 centers the
  view, and plus and minus zoom. In the controls screen a keypad key is
  recorded under the key it stands for, so the keypad cannot be bound
  separately.

T.O.R.E additions on keys FA leaves free: E (engine), Shift+U (HUD), F11
(help), Ctrl+1–6, Ctrl+Shift+1–4 and Ctrl+Tab (instruments), Shift+N and
Shift+L (next airport, request landing), Ctrl+Shift+R and Ctrl+Shift+C (tower
repeat and cancel), Alt+L (land at selected airport), Alt+A (sort: a
different bandit for each wingman, [data link](DATALINK.md#giving-assignments)),
Alt+N (monitor the [battle net](DATALINK.md#frequencies)), Alt+0 with
Alt+Shift+1–4 (address the flight or one wingman), Ctrl+B (mark a
[mission replay](REPLAYS.md#when) moment), and on the powered-lift aircraft
Ctrl+Alt+A (hover hold), Ctrl+Shift+A (stability level) and the Ctrl+arrow
cyclic trim of the helicopters and the V-22, where FA's Ctrl+arrows (thrust
vectoring) do nothing. With Pref → Debug panels? on, a
right-click that does not drag opens the debug menu, which takes no key.

Deliberate differences from FA:

| Key | T.O.R.E | FA | Why |
| --- | --- | --- | --- |
| Ctrl+A | Waypoint autopilot | Find-nearest cheat, when the Multi menu allows it | John requested the two autopilot modes on 2026-09-17 |
| Z / X | Rudder, alongside End / Page Down; on the AV-8 and Yak-141 the nozzle steps, as in FA | Wing sweep and, on the VTOL jets, nozzle steps (Shift+Z / Shift+X presets) | Laptops lack End and Page Down; T.O.R.E sweeps the F-14 automatically. The jets' nozzle keys are John's choice of 2026-10-08 |
| M | Cycle sensor channels | HARM seeker | No HARM channel yet |
| Shift+K | Jettison all external tanks and their remaining fuel | Retail used Shift+J for tanks | Opinionated, John 2026-10-15; [jettison](spec/jettison.md) |
| Ctrl+Shift+K | Jettison the selected external store group | Retail Shift+K dropped all air-to-ground ordnance | Opinionated, John 2026-10-15 |
| L | Clear designation, alongside ; | Unused | Earlier T.O.R.E key, kept |
| Shift+/ | Center view, alongside keypad 5 | Unused | Laptops lack a keypad |
| Shift+B | Afterburner switch, alongside 6 | Unused | Gamepad Y uses the same switch |
| Backslash | Designate what is under the sight's crosshair (the AC-130's gunsight, or the targeting sight of an aircraft with a laser designator or a FLIR); elsewhere "no targeting sight" | IR/laser designation, only with advanced targeting on | The AC-130 gunsight is that targeting, John 2026-10-09; every sight aircraft since A2G slice T1. The live-fire range reset moved to Ctrl+Shift+Backslash (kept its key, gained Ctrl+Shift) |
| Shift+Backslash | Pin the sight on the ground under the crosshair | Unused | T.O.R.E addition, John 2026-10-09 |
| P | AC-130 gunsight: pin or release (`sight-pin-toggle`); in the sensor console ORBIT or SEARCH. Elsewhere "no gunsight" | Unused (free in the original) | T.O.R.E addition, John 2026-10-14 (pin/release); the key is an agent choice |
| Ctrl+9 | AC-130 sensor console and orbit hold (`sensor-console`); elsewhere "No sensor console on this aircraft" | Unused | T.O.R.E addition, John 2026-10-13 (the console); the key is an agent choice |
| Ctrl+0 | AC-130 sensor console's radar range, 5, 10 and 25 nm (`console-range`); nothing elsewhere | Unused (the retail manual lists no Ctrl+0) | T.O.R.E addition, John 2026-10-14 (the range selector); the key is an agent choice |
| Shift+' / Shift+; | Zoom the sight in / out; elsewhere a message | Zoom the high-altitude bombing window in / out | The gunsight is the target camera, John 2026-10-09 |
| Alt + arrows | Slew the sight | Unused | T.O.R.E addition, John 2026-10-09 |
| Tab | Fire the gun while held, whatever is selected (`fire-gun`, a hold action like the trigger; [the gun key](spec/air-to-ground.md#the-gun-key)) | Fire the gun (no default pad button) | Retail key, bound by A2G slice K1, built by FX-T |
| Shift+R / Ctrl+R | Air-to-ground radar page on or off (`a-g-radar`): turns the air radar off, never clears the designation; R returns to the air radar | The same | Retail keys, bound by K1, built by G1. Ctrl+Shift+R is still the tower repeat |
| / | Sight: next visible object (`sight-next`): the sight tracks the next visible ground object to the right; "no targeting sight" without one | Designate the next visible object as the IR/laser target | Retail key, bound by K1, built by T1 |
| Ctrl+L | Laser on or off by hand (`laze`); "no laser designator" without one | Unused | T.O.R.E addition, John (A2G decision 5), built by T1 |
| Ctrl+M | Delivery mode: CCIP / LVL for bombs, BORE / CUED for missiles (`weapon-seeker-mode`) | Unused | T.O.R.E addition, A2G design 2.7; the action existed without a key |
| Ctrl+X | Nearest hostile ground target, under Easy targeting or on the AC-130 (`nearest-ground`); otherwise a message says it needs the cheat | Nearest ground target cheat when the Multi menu allows it | John, A2G decision 21; built by G1 |
| V | Save the view to Other View and open it | The same, then return the main view to the front | The main view stays |

## Profiles and calibration

Generate an editable profile without importing media or opening a window:

```sh
cargo run --locked -p tore-app -- --write-input-profile my-input.conf
cargo run --locked -p tore-app -- --input-profile my-input.conf --free-flight
```

The generator refuses to overwrite an existing file. It writes active standard
Linux gamepad bindings when the required capabilities are present and commented
suggestions for other controls. It does not infer unknown equipment functions.
Copy an edited profile to `input-v1.conf` in the application's data directory for
automatic loading; see [platform paths](DEVELOPMENT.md). `--input-profile` chooses
an explicit file. A custom profile replaces automatic controller defaults unless it enables
`gamepad-defaults on`;
existing keyboard flight/navigation shortcuts remain unless explicitly rebound.
Settings load at startup and can be edited in the [controls screen](#the-controls-screen).

Profiles use UTF-8 text, `#` comments and whitespace-separated tokens. The first
non-comment line must be `tore-input 1`. Limits: 256 KiB, 1,024 bindings and 64
aliases. Unknown actions, incompatible modes, invalid calibration and duplicate
aliases fail with the file's path and a line number. A profile named with
`--input-profile` that fails to load stops the start with that message, because the
player asked for that file. A damaged `input-v1.conf`, which loads without being
asked for, does not: the game starts with the default controls and the session log
warns `Controls file not loaded, using the default controls: PATH: REASON.`, as for
the other settings files (empty and binary files included; requested by John on
2026-09-29). The controls screen's next save writes a fresh `input-v1.conf` over it.
Identity/control tokens contain no whitespace.

Example (replace the identity with the monitor's exact device ID):

```text
tore-input 1
rumble off
alias stick DEVICE_ID_FROM_MONITOR
bind stick axis:0 roll axis -1 0 1 0.08 1.5 1 10
bind stick axis:1 pitch axis -1 0 1 0.08 1.5 1 10
bind stick button:304 gear press
bind keyboard Ctrl-g gear press
```

A binding is:

```text
bind DEVICE CONTROL ACTION MODE [MIN CENTER MAX DEADZONE CURVE SCALE PRIORITY]
```

`DEVICE` is an exact device identity, an alias, `keyboard`, `mouse`, or `*` for
every native device (never the keyboard or mouse). Prefer aliases/exact identities when different equipment shares
axis numbers. Never use enumeration order for persistent assignments. USB serials
are preferred; serial-less devices fall back to physical connection identity and
may require rebinding when moved. Devices with no serial or physical identity use
a session node identity; those bindings may need updating after reconnection. Some
composite peripherals expose generic button-only interfaces, which remain
unassigned until explicitly bound. Windows IDs are local to that machine. The
macOS raw HID fallback uses location identity. Apple GameController endpoints use
`macos-gc-session-…` identities: the public API does not expose the HID serial used
by our raw backend. These IDs deliberately change between processes/reconnections;
never use device names or enumeration order as persistent physical identities.
Generated Apple-gamepad suggestions use `bind * …`, commented by default, to share
named controls across gamepads. Such bindings can persist across sessions but cannot
distinguish two identical gamepads; per-device persistent Apple-gamepad assignment
needs a later explicit player-assignment flow. Profiles are platform-local.

The monitor shows **raw native values**. Absolute axes are normalized using the
reported range to `-1..1` before profile calibration. Thus the default calibration
is `-1 0 1 0.08 1 1 10`, even when the monitor reports `0..65535`. Independent
negative/positive spans, inversion (`SCALE=-1`), dead zone and exponent curves are
supported. Unit throttle axes map the calibrated endpoints to `0..1`; their
center/deadzone/curve are not applied. Trigger modes map to a signed unipolar
contribution and do apply dead zone/curve. No extra temporal input filter is added
on top of the aircraft's existing model-owned response. Radial stick dead zones
and automatic calibration wizards are not implemented.

| Mode | Meaning / compatible actions |
| --- | --- |
| `axis` | Centered pitch, roll, yaw, throttle-rate, look-x, look-y, sight-x or sight-y |
| `unit` | Absolute throttle position with pickup |
| `positive` / `negative` | Button-held signed contribution to a centered action |
| `trigger-positive` / `trigger-negative` | Signed unipolar analog contribution; pairs add within a device/priority |
| `press` / `release` | One command on the selected edge; repeated reports do not retrigger |
| `tap` / `long` | A command on release if the control was held under half a second (`tap`), or once when it has been held half a second (`long`). Used together on one control, as the gunsight's designate and pin are on Select + A. 60 ticks at 120 Hz; a pause or lost focus forgets the press |
| `hold` | Hold an equipment switch on while any assigned, armed source holds it |
| `switch` | Explicit equipment on/off when the physical contact changes; initial state does not actuate |
| `follow` | Continuously request the physical equipment setting, including at initialization/resume |
| `position=N` | One command on entering a discrete position; initial position does not actuate |
| `delta` | Encoder steps; throttle-rate means 1% per step; UI actions repeat for matching direction |

For a counterclockwise encoder UI binding use `SCALE=-1`; clockwise uses `1`.
Deltas are limited to 32 steps per event. Native discrete hats/selectors can use
`position=N`; multiple contacts are separate physical controls and are not
silently guessed to be one three-position selector. Device-specific neutral
combinations need explicit bindings or future reviewed composition support.

`eject` is a one-shot pilot action, normally bound with `press`. Press twice
within the [confirmation interval](spec/ejection.md#host-rules), releasing
between presses; holding or key repeat cannot confirm. It is not an on/off
switch and has no default gamepad button. The stock shortcut is Shift+E,
also available through the `key:Shift-e` shortcut alias.

Equipment actions: `gear`, `flaps`, `airbrake`, `hook`, `engine`, `burner`, `radar`,
`jammer`, `autopilot`, `waypoint-autopilot`. A and Ctrl-A toggle the two
[autopilot modes](spec/autopilot.md); controller bindings and pilot recordings
use the same simulation commands. `press` toggles; `switch`/`follow` request a setting. These request the
existing system behavior and never force animation fractions or bypass aircraft
capabilities. Rafale's unavailable hook stays unavailable. `throttle=0.75`
requests a preset. Axes are `pitch`, `roll`, `yaw`, `throttle`, `throttle-rate`,
`look-x`, `look-y`, `sight-x`, `sight-y` (the AC-130 gunsight, right and up
positive), and the absolute `head-yaw` and `head-pitch`. Mouse controls
are `button:right`, `button:middle`, `button:back`, `button:forward`, `wheel:up`
and `wheel:down`; each wheel notch is one press.

UI actions: `pause`, `menu`, `end-flight`, `restart`, `bookmark`, `view-front`, `view-back`,
`view-up`, `view-external`, `view-track`, `view-threat`, `view-wing`,
`view-target`, `view-target-player`, `view-fly-by`, `view-missile`, `store-view`,
`view-target-track`, `center-look`, `cockpit`, `hud`, `zoom-in`, `zoom-out`,
`range-down`, `range-up`, `radar-mode`, `sensor-channel`, `sensor-infrared`,
`sensor-history`, the AC-130 gunsight's `sight-designate`, `sight-pin`, `sensor-console`,
`sight-zoom-in`, `sight-zoom-out` and the holds `sight-left`, `sight-right`,
`sight-up`, `sight-down`, `page-0` through `page-9`, instrument
commands below, and `menu-up/down/left/right/accept/back`. `sensor-channel` is an
alias of `radar-mode`; both cycle the available radar and infrared channels
rather than the retired cosmetic display mode. `sensor-infrared` requests the
passive channel and `sensor-history` toggles the scope contact trail. Existing
profiles that bind `radar-mode` keep working and now cycle channels. `key:Shift-0` or
`key:Ctrl-t` dispatches a stock flight shortcut through the same menu/availability
handler; it is a one-shot command, not a synthetic held keyboard key. Unsupported
systems still report unavailable. Bind continuous flight actions directly.
Custom keyboard controls use physical names such as `g`, `ArrowDown`, or
`Ctrl-Shift-g`; modifier order is Ctrl, Alt, Shift, Super. OS exit shortcuts should
remain on the keyboard.

## Shared assignments and interruption

Every binding tracks its own physical baseline and contribution. Releasing or
disconnecting one source never releases another source's held contribution.
Equipment `hold` combines sources with OR; `switch` emits on/off only on a real
change. A `follow` binding keeps physical authority over that equipment, including
a keyboard toggle; disconnecting it relinquishes authority and retains the last
setting. Use `switch` when keyboard/cockpit overrides should persist.

Centered analog actions choose the highest-priority active source. Equal-priority
ownership stays with the current source until it becomes neutral; ties on initial
acquisition use stable binding/device order. Signed button/trigger contributions
combine within a device and priority, then participate in the same arbitration.
This avoids jitter-driven last-event ownership. Independent axes can have different
owners. Opposing contributions cancel. Throttle uses pickup: after a preset,
rate adjustment, reconnect or resume, the physical lever must reach/cross the
current setting (4% tolerance) before taking over. It never snaps to zero on
unplug. When multiple absolute throttles are assigned, priorities determine the
eligible owner rather than averaging positions.

Menus and explicit/focus pause clear pending gameplay commands and held output.
The running-flight respawn selector uses this same input context and menu bindings,
while the network simulation continues. Closing it restores the flight context;
see [multiplayer revival](MULTIPLAYER.md#ai-aircraft-and-player-choice).
Physical state remains tracked. Neutral buttons/axes rearm on resume; controls
still held must return to neutral/release first. Custom keyboard chords retain
the original release owner even after modifiers change. Menu buttons cannot leak
into flight as a new press. An actively contributing primary controller's
removal pauses flight; reconnecting does not resume it. A switch-only box removal
does not change latched equipment settings. Nonfinite samples and native/core queue overflow drop stale
commands and pauses rather than silently leaving controls stuck.

## Instrument selection without screen changes

`Ctrl-Tab` / `Ctrl-Shift-Tab` selects the next/previous existing instrument slot.
`Ctrl-1..6` selects a slot directly. `Ctrl-Shift-1..4` operates its four existing
button positions. These are authored shortcuts, not decoded retail bindings.

Controller actions are `instrument-next`, `instrument-previous`, `instrument-1`
through `instrument-6`, `control-1` through `control-4`, and direct actions such as
`instrument-2-control-1`. `page-N` toggles the existing instrument page. A brief
existing-style notice identifies focus; there is no raster alteration, window
movement or new screen content. Layout/page changes reset focus to the first
slot. Absent slots and unimplemented controls report unavailable. Scope
buttons are instantaneous commands, so there is no invented held sensor action.
RCS buttons 1/2 change its scale. RWR and radar buttons 1/2 change the shared
scope range (the RWR shows it capped at 50 miles, see the
[RWR specification](spec/rwr.md#range)). Radar button 3 cycles the available
sensor channels and button 4 toggles contact history. Buttons with no action on a page, and unsupported pages, remain
unavailable.

## Fixed ticks and input tapes

`tore-sim` receives a typed `PilotInput` for each 120 Hz tick: continuous demand
plus ordered one-shot commands. Device/key strings and calibration stay outside
simulation. Equipment commands and throttle presets now apply at tick entry;
actuator audio follows actual state changes. Existing aircraft response and
configuration remain separate. Camera look resolution cannot acquire flight-axis
or throttle ownership.

Native discovery/polling runs on a dedicated worker (4 ms service interval with a
controller connected and the game in flight, 16 ms with a controller in a menu,
50 ms with none), with bounded messages to the app (polled every 8 ms in flight,
the replay viewer and the controls screen, 16 ms in menus with a controller, 33 ms
without). Linux delivers ordered
evdev transitions at report boundaries; macOS generic devices use HID queues.
Windows raw controllers and Apple GameController profiles sample native current
state, so a complete pulse between samples can be missed. Once captured, short command press/release pairs survive until their
next simulation tick. Live input is admitted to the next available tick; this is
not a claim of identical wall-clock input sampling under arbitrary renderer
stalls. Paused gameplay edges are discarded, not played back at resume.

```sh
cargo run --locked -p tore-app -- --free-flight --record-input flight-input.txt
cargo run --locked -p tore-app -- --replay-input flight-input.txt
```

Pilot-only recording retains its clean-aircraft start, with no external stores,
to preserve existing replay initial conditions. Normal unrecorded free flight
loads supported default weapons. Use `--record-combat` for weapon-service tapes.

These input tapes are separate from the mission recordings every flight makes
([mission replays](REPLAYS.md)), which keep what happened rather than the
controls. `bookmark` (Ctrl+B by default, "Mark replay moment" in the Game and
menus group) marks the moment in that recording and shows "Bookmark N saved".
The printable [keyboard map](tore-keyboard-map.html#replay) shows it on the
Replay sheet, beside the [replay viewer's keys](#replay-viewer-shortcuts).

Recording requires a direct free-flight start without a capture, probe or initial control/device-pose override and stops when that flight ends/restarts. Files are create-new and flushed
on exit. Tapes contain **pilot inputs only**, not mission saves, UI/camera commands,
assets or initial state. Replay uses a fresh flight in the chosen theater; supply
the same aircraft, theater, model flag, assets and configuration as the recording.
The format is bounded to one hour/64 MiB with ordered ticks and finite values.
Fractional axes and ordered commands round-trip and reproduce identical states
under 30/60/144 Hz render schedules in synthetic tests. Cross-CPU bitwise flight
parity and whole-mission replay remain separate acceptance work.

## Feedback and native backends

`rumble off` is the default. `rumble on` enables authored event feedback on
connected rumble-capable devices with actual pilot/equipment bindings. Devices at
rest still receive feedback (including when the keyboard engages afterburner);
unassigned and UI-only devices do not. Targets are captured when an event is
accepted, so connection/reconnection alone never replays an effect.

| Confirmed event | Strong / weak | Nominal duration | Live producer |
| --- | --- | --- | --- |
| Gun fired | 8% / 16% | 67 ms | Hook only; gun simulation unavailable |
| Missile launched | 18% / 10% | 150 ms | Hook only; weapon release unavailable |
| Bomb released | 12% / 7% | 100 ms | Hook only; weapon release unavailable |
| Rocket launched | 10% / 16% | 83 ms | Hook only; weapon release unavailable |
| Turbulence | Up to 10% / 6%, scaled by severity | 125 ms | Hook only; no turbulence producer yet |
| Afterburner engaged | 6% / 10% | 150 ms | Actual inactive-to-active transition |
| Afterburner running | 3.5% / 1% | Continuous quiet bed | Actual active state; bounded renewable native leases |
| Damage | 25% / 18% | 167 ms | Hook only; combat damage unavailable |
| Crash | 35% / 20% | 183 ms | Actual transition into crashed state |

These are opinionated tactile designs, chosen by the implementation rather than
recovered from the original. They are not directional flight-stick forces. The
dependency-free `tore_input::feedback` mixer has eight
fixed slots and combines motor strengths by maximum, capped by these designs,
rather than adding overlapping effects. Gun/rocket repeats are admitted at most
20 Hz, turbulence/missile/bomb/damage at 10 Hz, afterburner at 2 Hz and crash at 1 Hz.
Mixer updates are limited to 20 per simulated second and catch-up ticks are
coalesced before native submission; small cues may be delayed up to 50 ms
behind another update. Each native pulse has a finite duration; stopping event
production lets the effect expire. No rumble alters authoritative simulation or RNG.

The app queues events after the 120 Hz flight tick, then advances the mixer once.
Afterburner feedback requires actual activation: merely setting its switch below
the model's throttle threshold, with no fuel, or with the engine off does not
produce a pulse. Raising throttle through that threshold with the switch armed
can activate it. While active, a quiet low-frequency bed continues beneath other
impulses, using 750 ms native leases renewed every 500 ms of simulated flight.
The engagement impulse is not retriggered by renewal. Disengaging afterburner
cancels its bed and remaining engagement pulse; unrelated effects may finish.
A stalled/exited producer cannot leave an infinite native effect. Pause/focus
loss and flight teardown clear pending impulses/cooldowns as well as native effects.

Future weapon systems must call `Input::feedback` with the typed event after a
successful shot/release, never for dry fire, an unavailable control or a held fire
key alone. A future turbulence producer must supply finite normalized severity;
steady wind, turns, G-load and stall are not substituted for turbulence. These
hooks do not implement weapons, loadouts, damage or turbulent flight dynamics.

```sh
cargo run --locked -p tore-app -- --test-rumble only
# Or select an exact persistent ID from --list-inputs (Linux/Windows):
cargo run --locked -p tore-app -- --test-rumble DEVICE_ID_FROM_LIST_INPUTS
```

`only` completes discovery, then requires exactly one rumble-capable controller;
it fails instead of picking the first when several are present. Use it for Apple
gamepads whose session ID changes between CLI runs. The test does not require
`rumble on`, retail media, or a display. It requests a 200 ms pulse at 20% strength.
Native API acceptance and
physical response are distinct; the latter needs a human check. Feedback requests
are bounded to 1–2,000 ms and per-device deadlines; stop requests cannot be blocked
by a full effect queue. Pause, focus loss, overflow, shutdown and worker drop
stop effects and discard pending stale requests. Reconnect never replays them.

| Platform | Implementation / limits |
| --- | --- |
| Linux | evdev capabilities and report boundaries, nonblocking reads, SYN_DROPPED state recovery, FF_RUMBLE. USB serial plus interface identity when available. No exclusive grabs or system permission changes. |
| Windows | Built-in Windows.Gaming.Input RawGameController and Gamepad vibration through `windows` 0.58 bindings. Dedicated WinRT worker initialization/teardown; device removal and read failure stop vibration before releasing the endpoint. Current-reading polling; custom mappings required. |
| macOS | On macOS 11+, supported gamepads use GameController profiles and controller-created CoreHaptics engines. Apple's `supportsHIDDevice:` suppresses duplicate HID input. Other joystick/multi-axis/button-box devices retain IOKit HID queues, with no generic HID rumble. Custom mappings required. |

Apple gamepad controls are `gc-axis:HEX_NAME`, `gc-button:HEX_NAME` (native
pressed state) and `gc-pressure:HEX_NAME` (0..1 raw pressure, normalized as an axis).
Names are UTF-8 bytes encoded as hex so spaces cannot break a profile. Pressure
controls support analog triggers independently of digital button edges. Copy the
control token from the monitor/generated profile; do not bind both a pressure
control and its digital counterpart to the same one-shot action.

macOS uses separate left/right handle localities when both are advertised, with
strong/weak mapped to intensity and low/high sharpness respectively. Otherwise
the default locality receives `max(strong, weak)` at medium sharpness. This is an
authored haptic approximation, not identical motor frequencies across hardware.
Effects have finite native duration as well as the worker deadline. Replacement,
interruption and teardown stop retained players/engines; errors discard the engine
and never automatically replay a pulse. Engines are created only when feedback is
requested. Native cold starts can delay the input worker; no haptic call blocks
the renderer/simulation thread. macOS 11+ is the supported haptics configuration;
older macOS linking/runtime is not validated.

Only `tore-input-native` permits audited unsafe FFI. Linux adds the already-used
`libc` platform bindings; Windows adds features to the already-locked `windows`
0.58 dependency. macOS adds narrowly enabled `objc2-game-controller` and
`objc2-core-haptics` 0.3.2 bindings and `block2` 0.6.2, reusing already-locked
Objective-C/Foundation/dispatch bindings. Input policy remains hand-rolled;
`tore-input` is dependency-free and safe Rust. All other workspace crates retain
`unsafe_code = forbid`. No SDL, third-party controller runtime, USB driver,
retail executable, or device-specific output protocol is embedded or executed.

Native API references: [Linux event protocol](https://docs.kernel.org/input/event-codes.html),
[Windows raw controller reading](https://learn.microsoft.com/en-us/uwp/api/windows.gaming.input.rawgamecontroller.getcurrentreading),
[Apple HID elements](https://developer.apple.com/documentation/iokit/1588671-iohiddevicecopymatchingelements),
[Apple controller haptics](https://developer.apple.com/documentation/gamecontroller/gccontroller/haptics),
[haptic localities](https://developer.apple.com/documentation/gamecontroller/gcdevicehaptics/createengine(withlocality:)),
[HID support query](https://developer.apple.com/documentation/gamecontroller/gccontroller/supportshiddevice(_:)),
[Windows vibration](https://learn.microsoft.com/en-us/uwp/api/windows.gaming.input.gamepad.vibration).
See [acceptance and open hardware checks](baselines/input.md).

## Manual combat layer, 2026-09-14

New standard Linux gamepad profiles reserve **Select as a held combat modifier**.
Press Select first, then the action control. Release the action before switching
layers; a held control cannot become a new action when Select is pressed/released.
Old saved explicit profiles retain their assignments; use the editor to add the
new bindings or regenerate a profile deliberately. No saved file is overwritten.

| While holding Select | Keyboard equivalent / action |
| --- | --- |
| Right shoulder | Space: hold fire/release |
| Left shoulder | ] next NAV/weapon |
| South (A) | T: next radar target; on the AC-130, tapped: Backslash (designate under the sight), held half a second: Shift-Backslash (pin the ground) |
| East (B) | L: clear designation |
| West (X) | [ previous NAV/weapon |
| North (Y) | J: own jammer toggle |
| Left-stick click | R: radar toggle |
| Right-stick click | Ctrl+Shift+K: selected external group jettison |
| Right stick (AC-130) | Slew the gunsight; other aircraft keep their uses of it (look, nozzles, collective) |
| D-pad up | Ctrl-Shift-Backslash: replace range target; on the vectoring jets and the V-22, neutral nozzles or nacelles; on the AC-130, Ctrl-7 (next gun candidate) |
| D-pad down | Explicit `damage-player` developer fixture; keyboard D reports damage; on the AC-130, Ctrl-8 (link or unlink the candidate gun) |
| D-pad left | Insert: release one chaff cartridge |
| D-pad right | Delete: release one flare |
| Start | Shift-Y: target jammer fixture |
| Guide | Ctrl-Shift-I: one incoming selected source weapon fixture; on the AC-130, Ctrl-9 (the sensor console) |

Chaff and flare took D-pad left and right from the damage-class and
station-failure fixtures on 2026-09-26, an agent decision. Both fixtures remain
bindable. [Countermeasure rules](spec/countermeasures.md).

The `radar` equipment action behaves exactly like the keyboard R: while the
infrared channel is selected it returns the scope to radar, and otherwise it
toggles radar power. Bind `sensor-channel` or `sensor-infrared` to select a
channel without touching the power switch.

Guide may be intercepted by the desktop; bind `incoming` to another exposed
button or combo in **Escape → Control** when necessary. No desktop shortcuts are
changed. Unmodified Start still pauses; unmodified flight buttons, shoulders,
rudder triggers and instrument navigation retain their functions. External view
uses F10 or a custom `view-external` binding; Select is no longer its default.
No AI makes the incoming launch decision. Incoming and damage fixtures require `--live-fire`; jettison does not.

Profile chord syntax is `MODIFIER+CONTROL` or `MODIFIER+MODIFIER+CONTROL`, for
example:

```text
modifier pad button:314
modifier pad axis:16=-1
bind pad button:314+button:311 fire hold
bind pad button:314+button:304 designate press
bind pad button:314+axis:17 range-target position=-1
bind pad button:314+axis:16=-1+button:304 flaps press
```

Two or three distinct nonempty tokens are required. A token may be a virtual
button over a physical control: `axis:16=-1` is pressed while that hat or D-pad
axis reads exactly -1, and `axis:5>0` or `axis:1<-0.5` is pressed while the
normalized axis is past that threshold. Virtual buttons work as modifiers and as
bound controls. Keyboard and mouse bindings take no chords; keyboard shortcuts
keep their `Ctrl-`/`Alt-`/`Shift-` syntax. `fire` requires **hold** behavior;
press/release or axis modes are rejected instead of silently doing nothing.
Capture in the controls screen records held modifiers automatically. Exact
Windows/macOS device IDs/control names still need an explicit platform profile.

Combo actions suppress the base control, including throttle and menu actions.
Pause/focus changes, disconnect, layer transitions and reset cancel fire;
physical neutral/release is required before rearming. Keyboard and controller fire
are independent, so releasing one cannot cancel a trigger held by the other.

## Weapon haptic envelopes

Rumble remains opt-in: the controller tab's Rumble setting, then Apply. Cues are authored,
not native force-feedback recovery. Strong/weak motor amplitudes are normalized:

| Confirmed event | Duration | Strong / weak |
| --- | ---: | ---: |
| Gun representative shot (sustained fire refreshes) | 67 ms | 0.08 / 0.16 |
| Own missile launch | 150 ms | 0.18 / 0.10 |
| Bomb release | 100 ms | 0.12 / 0.07 |
| Player-owned bomb impact confirmation | 200 ms | 0.08 / 0.04 |
| Rocket launch | 83 ms | 0.10 / 0.16 |
| Actual player damage | 167 ms | 0.25 / 0.18 |
| Player destruction/crash | 183 ms | 0.35 / 0.20 |

Gun/missile/player-damage/destruction producers are connected. Bomb/rocket cues
are tested mixer contracts; the two PT defaults contain no bombs/rockets and
there is no runtime producer or enabled alternative loadout for those cues yet.
Remote target hits and incoming fixture launches do not masquerade as ownship
haptic events. A defeated incoming contact causes no damage impulse.

Nine fixed slots mix by maximum amplitude, never additive escalation; native
requests are capped at 20 Hz and use finite leases. Repeats have cooldowns.
Pause, focus loss, restart, profile replacement, disconnect and shutdown stop
feedback. Unsupported devices continue silently. A native API failure disables
feedback on that device until reconnect and reports one explanation. Native
worker deadlines and bounded requests guard against stuck effects. Actual motor
strength and comfort still require physical controller acceptance.

## Additional aircraft capabilities

F-14D, A-4E and X-31 EFM use the existing input bindings. Device commands respect
capabilities: A-4E has no burner, and X-31 has no hook. The added powered-lift
controls apply to AV-8B, Yak-141, V-22 and helicopters in the researched adapter;
X-31 keeps its existing fitted aerodynamic model. See [aircraft behavior](spec/additional-aircraft.md) and
[validation](baselines/aircraft-fa-expansion.md).

`--flight-throttle 0..1` sets the initial throttle for engine-material captures;
see the [engine material contract](spec/engine-material.md).

Researched flight is now the default; `--legacy-flight` preserves the previous
model. HUD and audio share the [stall warning signal](spec/stall-warnings.md),
including the original imported warning samples.

## Missile seeker control

Radar power OFF disables radar-missile bore and tones; IR remains independent; active missiles launch maddog, searching immediately with their own seeker.
SARH flies unguided until illumination begins. Selecting passive IR alone does not switch power off.
Surface weapons cannot use the A2A bore toggle; surface designation remains deferred.
Armed independent air-to-air missiles automatically enter BORESIGHT when radar power is on and no target is
selected. IR also supports bore with radar power off. A selected track takes
priority for IR and forces CUED acquisition against that identity, even when a
stronger bore return exists. Clear the track to return to BORESIGHT; airborne
missiles keep their own targets. Select a target to return to CUED. Press **;** (FA's unlock key) or **L**, the
`clear-designation` action, or click **RELEASE LOCK** in the upper-right weapon
diagnostic panel when it is shown, to clear
both sensor and HUD display selection. A selected target outside the HUD has a
direction chevron; the Easy targeting cheat keeps it after sensor coverage is
lost, which grants no weapon lock. [HUD target rules](spec/gunsight-targeting.md).
`weapon-seeker-mode` is on Ctrl+M (delivery mode, [key placement](#key-placement)) and
rebindable; the diagnostic panel's mode label is clickable only while the panel
is shown (**Escape → Pref → Weapon diagnostics?**). Supported radar weapons need aircraft illumination to guide, not to release.
MAX RANGE and ACQUIRING are advisory. Radar missiles can also release without
support, including surface SARH waiting for later A-G illumination. Unlocked IR shots search as BORE;
active maddog seekers can acquire friendlies. See [release rules](spec/missiles.md#player-release-advisories-and-maddog).

BORE uses a five-degree circular half-angle. Its blinking diamond marks a
provisional contact, not a guaranteed lock; the blinking triangle on the range
scale refers to that same contact. Selection favours the centre while retaining
signal-strength weighting. IR can acquire on the rail; active radar acquires only
after release. The bare percentage is a fitted estimate, not a calibrated retail percentage.
Short retail weapon labels, bore circle, estimate and readiness all move with
the forward HUD when looking around. Range, closure and estimated flight time
and target aspect angle are in the upper-right weapon diagnostic panel, hidden by default. The HUD layout
is 15 percent smaller; ARM, count/weapon and percentage with blinking IN RNG
align below speed. The range scale sits inside altitude; radar R/C/A sits below it. BORE READY is omitted. Neither clearing selection nor changing mode redirects an airborne shot.
An internal bay opens for BORESIGHT and release waits until 95 percent open.
Weapon readouts sit just below the airspeed and altitude boxes; NAV retains the bank scale. AGL and
vertical speed appear only with non-weapon ILS guidance. [Layout and startup modes](spec/hud-layout.md). CUED radar
lock diamonds blink when ready to fire. The radar instrument replaces the mouse
arrow with a crosshair while the pointer is over its plotting area.

Imported IR and radar search/lock samples provide the cues, with a louder lock
cue. Their assignment is fitted. Effects mute, pause, safe, empty and failed
stations silence them. `TORE_SEEKER_VOLUME=0..1` sets maximum amplitude, default
0.30. Re-import media to add the four samples to an older cache.
[Rules and constants](spec/missiles.md), [validation](baselines/hud-cleanup.md).

## Player wing orders

The keys follow Fighters Anthology's wingman table ([keyboard spec](spec/keyboard.md#wingman-orders)),
adopted at John's request on 2026-09-26. Orders address friendly wing 1 in a
live AI Quick Mission. FA always addresses the whole flight; addressing one
wingman is a T.O.R.E addition on keys FA leaves free: Alt-0 selects the whole
flight and Alt-Shift-1 through Alt-Shift-4 select wingmen 1 through 4. Restart
restores whole-flight addressing.
Unavailable recipients or targets produce explicit messages. Paused flight does
not issue orders. The imported flight menu has no wing-order submenu.

Both sides start in neutral formation, even with free-fire objectives. Your
wingmen wait for your engagement commands while continuing radar scans and
missile defense. AI flight leaders issue engagement orders in response to
perceived attacks on their flight or protected aircraft.

| Shortcut | Order |
| --- | --- |
| Alt-1 | Fly straight and level |
| Alt-2 / Alt-3 | Break left / right |
| Alt-4 / Alt-5 | Break low / high |
| Alt-6 / Alt-7 / Alt-8 / Alt-9 | Approach the designated target from left / right / low / high |
| Alt-E | Engage the designated target: an aircraft, else a ground object (the ground designation, or the object nearest the sight's pin) |
| Alt-R | Engage designated target from formation, medium control; takes a ground target like Alt-E |
| Alt-F | Engage the IR/laser target: the object the targeting sight tracks, its laser spot is on, or its pin is nearest to, air or ground. With none held, attack on contact |
| Alt-G | Lase my target: a wingman with a designator lases the ground target you designated (a T.O.R.E addition, John 2026-10-11) |
| Alt-A | Sort: give each addressed wingman a different bandit from the flight's [data link picture](DATALINK.md#giving-assignments) (a T.O.R.E addition, John 2026-10-05) |
| Alt-W | Engage every target of the designated target's class ("Attack SAMs", "Attack tanks"); a designated aircraft, or no designation, is "Attack bandits", attack on contact |
| Alt-P | Protect me, maintain an escort duty |
| Alt-D | Disengage and return to neutral formation |
| Alt-B | Bug out: return to base and stop answering orders |
| Alt-T | Next formation: echelon, line abreast, line astern, in turn |
| Alt-C | Toggle loose / medium control |
| Alt-H | Toggle 512 / 2048 ft horizontal spacing |
| Alt-V | Cycle level / 512 ft high / 512 ft low stacking |
| Alt-L | Land at the airport selected with Shift-N |
| Alt-S | Radio silence |
| Alt-0 / Alt-Shift-1 through Alt-Shift-4 | Address all wingmen / one wingman |

Input profiles can use these as `key:Alt-2`, `key:Alt-t`, `key:Alt-Shift-1`
and the corresponding keys above.

Where T.O.R.E's orders differ from FA's: Alt-W and Alt-F are FA's. Alt-W
engages every target of the designated target's class, as FA's does; T.O.R.E
used to put attack on contact there (to 2026-10-11), which is now what Alt-W
does with no ground target designated and what Alt-F does with nothing in the
sight, so attack on contact keeps no key of its own. FA's
IR/laser contact is the one chosen with `/` and `\` while IR/Laser advanced
targeting is on; T.O.R.E takes the sight's track, laser spot or pin, which
those keys set. The orders onto the ground (Alt-E, Alt-R, Alt-F, Alt-W) go to
the wingmen the flight lead addresses and are refused, with the wingman saying
"Winchester", by one with no surface store it can attack with. Alt-G is a
T.O.R.E addition on a key FA leaves free. Alt-T starts from the formation the
first addressed wingman flies. Bug out moved from Alt-U to FA's Alt-B on
2026-09-26, when John adopted the FA layout; Alt-U no longer gives orders.
Land at selected airport has no retail equivalent; it is an opinionated
addition John requested on 2026-09-23. Bug out sends each addressed
wingman to its own home runway, the departure runway after a ground start or
else the nearest friendly or neutral airport. A wingman with no known base
stays and is counted in the reply. Land at selected airport sends the addressed
wingmen to the airport the tower has selected (Shift-N): the runway you are
cleared for there, or else its longest usable runway. Hostile, unknown or
unpermitted neutral airports and airports with no usable runway are refused
with a message. The 22 short airstrips (a runway under 2,000 ft) are not on the
Shift-N list, so a wing is never sent to one. A bugged-out wingman no longer answers any order; later orders
skip it and say so. Both orders print the call ("Bug out", "Land at Field")
without voice, because the reviewed radio catalog has no recording for them.
Bug out is ignored on the ground, during takeoff, or from landing marshal
onward. It is accepted during the route home, as in retail; the reply counts it. Landing wingmen report holding
at marshal, landing and landed. While you approach a friendly airport with the
gear down, below 4,000 ft and within 25,000 ft, AI aircraft for that airport
hold at marshal until you are down and clear
([player priority](spec/airports.md#wing-landing-orders-and-player-priority)).

The message gives applied, rejected and no-motion counts. An explicit attack target must be alive,
hostile and present in each recipient's own radar or visual contacts. Synthetic
headless actors without sensors retain their explicit direct-awareness fallback.
Protect me needs no selected or currently detected attacker.
The first living wingman alone replies to an accepted engage/protect assignment.
Commands take effect immediately, independently of their radio recordings.
A new command interrupts queued old command audio. The Sound/Music Prefs Radio
Msg slider sets radio loudness, and Overall at off mutes it with the effects.
Missing recordings or an old cache leaves commands and text operational.
Reimport user-owned media to load [verified radio mappings](formats/radio.md).

Line abreast alternates right/left at one spacing, then right/left at two
spacings, preserving the echelon sides. Routine formation, spacing and stacking
changes now use controlled repositioning from each aircraft's current location.
Aircraft coordinate conflicting paths and can establish aft clearance before
moving inward. A new order replaces the pending path; real collision danger
still permits breakout. Harder turns can therefore interrupt a transition.
Normal vertical wandering now requests at most five feet, with smooth changes.

Formation selection and disengage cancel the engagement and let the safe rejoin
procedure return the aircraft. They remain neutral until a new engagement order
or a newly perceived attack; an already-known missile warning cannot restart
the pursuit. Evasion continues when needed. Spacing and stacking alone do not
cancel or authorize combat. Approaches assign the
selected target and continue that engagement after reaching the fitted approach
point. Protect me assigns a persistent escort duty: wingmen assess detected
hostiles near or approaching your aircraft and respond to shared attack reports.
They return when the escort pursuit limit is reached.
[Behavior and limits](spec/ai-awareness.md#mission-roles-and-rules-of-engagement).

For live testing, use a Quick Mission with at least three friendly aircraft:

1. Issue Alt-D, then change formation, spacing and stacking. Confirm smooth
   physical repositioning and one player call, without generic wingman replies.
2. Select wingman 2 with Alt-Shift-2 and issue a break. Confirm wingman 1 keeps its
   assignment. Alt-0 restores whole-flight orders.
3. Designate a detected enemy and issue Alt-E, then immediately Alt-D. Confirm
   accepted recipients disengage and stale “Engaging” audio does not follow it.
4. Try an absent or unobserved target, pause, mute sound, and restart. Confirm
   no false acknowledgment, paused order or replayed old radio.
5. Repeat a diving reversal. Confirm separate safe rejoins, restrained status
   reports and no aircraft pose jumps. A request for steady flight is advisory.
## Airport commands

Normal ground starts enter NAV with weapons disarmed. Airborne starts select
and arm the canonical gun. Bracket keys cycle NAV and weapons; NAV keeps
target cues while suppressing weapon readouts. [Layout and defaults](spec/hud-layout.md).

The controls editor exposes `airport-nav`, `airport-next`,
`airport-request-landing`, `airport-repeat`, and `airport-cancel`. The default
profile binds them to N, Shift-N, Shift-L, Ctrl-Shift-R, and Ctrl-Shift-C in
that order. N is FA's NAV mode key. Next airport, repeat and cancel left
Shift-A, Shift-R and Shift-C on 2026-09-26, because FA uses those for the AWACS
link, air-to-ground radar and slow motion ([key placement](#key-placement)). NAV mode, gear down, range, and airport-relative altitude govern automatic
ILS guidance; the threshold must also be within the aircraft's 90-degree
forward cone. Outside the 5-NM/4,000-foot-above-airport band, even ILS ARM is
hidden. A clearance is not required to display eligible guidance. Airport selection remains
explicit when supplied, otherwise the nearest usable runway is selected. A
successful landing request and its repeat play the reviewed retail clear-to-land
recording. Landing completion plays the reviewed welcome-home recording, and
repeat then replays that welcome. Other
tower replies remain text only.

## Quick Mission ground start

In the creator, set **Start** to **Ground**, then choose **Airport**. Continue
through the normal loadout screen. The player starts on that runway with engine
idling, gear/flaps down and brakes applied. **B** releases brakes; use the normal
throttle and flight controls to take off. The player's AI wingmen queue on the taxiway
and wait until the player is airborne before entering the runway. Startup
takeoff clearance and wing departure/landing reports are automatic; Alt-S
suppresses routine wing reports but keeps player clearance. Other
wings start airborne at the displayed wing altitude. Restart restores the accepted airport/start.
Airborne remains the default. Ground start requires the researched flight model;
legacy and restricted native modes remain available for airborne starts.

Shift-M toggles the live map. Escape closes it, plus/minus zoom, arrows pan and
Home resumes following the player. Map navigation takes priority over keyboard
flight bindings for those keys while open. Other flight controls remain live.
Enter or controller A switches between Map and Mission. The Mission brief
scrolls with Up/Down, the controller D-pad, Page Up/Down, Home/End, the mouse
wheel or the red scroll bar. Escape or controller B closes it. The map retains
its view when switching tabs. A/B and the D-pad do not also operate the aircraft
while this page is open. In the in-flight respawn Loadout and Aircraft pages,
select Mission... or press M outside search focus for the same brief; Close, Escape/B or Enter/A
returns to the unchanged page. Pre-flight loadout menus are unchanged.
Right-side map buttons toggle categories, with Buildings off by default.
Selections last until flight restart; no category settings appear in Escape. See the
[map specification](spec/flight-map.md).

While waiting for multiplayer revival, Escape opens **Loadout...**, plus
**Aircraft...** in Player Choice slots, throughout and after the delay. Done
saves edits; Cancel discards unsaved edits. Loadout stations open a searchable
store list; Aircraft shows all available choices in one scrolling list.
Click Search or Tab to focus it, type to filter, and use Backspace to erase.
Tab toggles search/results focus. Up/Down or D-pad navigates results;
Enter/A selects, Home/End and Page Up/Down navigate, and the wheel or red bar
scrolls. Escape/B clears search first, then closes a station sub-menu, then
cancels the page. M types into search when focused; Mission... remains clickable.
Close all menus and press
Enter or controller A to fly again when ready. Early presses do not queue a
respawn, and closing a menu never confirms one. See [revival choices](MULTIPLAYER.md#ai-aircraft-and-player-choice).

Quick Mission inline options support right-click to cycle backward, wrapping to
the last available value. Left-click retains forward cycling and the existing
list pickers. The player's wing never cycles below one. Right-click requires a
matching press/release and cannot launch, cancel or select through a modal list.
The ordnance view retains right-click quantity decrement.

The same rule (left click next, right click back, wrapping; John, 2026-10-23)
holds for every click-to-cycle value in the menus: the controls screen (see
above), the Escape menu's Stability level, HUD colour and AI observation rate
rows and its on/off rows in flight, and the respawn menu's Internal fuel row
(a tenth a click, wrapping between a tenth and full). Rows that choose one of
a set (Time, Damage, Enemy AI, the Graphics and Replays choice buttons) and
commands are unchanged, and the keys still turn forward only.

In Load Ordnance, drag a catalog weapon onto a compatible station to load it.
Its picture follows the pointer. Drag a loaded station to another station to
transfer ammunition, or back into the catalog area to empty it. Escape cancels
a drag. Left-click a loaded station to add one (a pair on a paired
station), up to its capacity. Click an
empty station to load the selected catalog weapon. Empty stations retain a red
outline. See the
[drag and quantity rules](spec/ordnance-presentation.md#dragging-and-empty-stations).
Quick Mission's Guns only restriction applies to every friendly and enemy wing
and remains active after restart.

View 4 target-camera fields and status meanings are described in the
[flight controls guide](FLIGHT-CONTROLS.md#target-camera-view-4).

Quick Mission wing skill menus include **Dummy (400 KTS)** for straight-flying
training targets. See [behavior](spec/dummy-aircraft.md).

## NAV and weapon selection

`[` and `]` cycle backward/forward through NAV and weapons. Selection controls
arming. Old `master-arm` profile entries are
accepted but do nothing. The editor offers `weapon-next` and `weapon-previous`.
NAV INFO minus/plus select destinations; button 3 switches mission/airport mode.
WEAPONS minus/plus select NAV/weapons; button 3 pages the store list.
IR boresight growl follows the actively tracked bore target and its displayed
percentage without designation. Empty bore and lost tracks are silent. A radar
missile in boresight sounds its lock tone on the bore return without
designation, cued seekers sound only while tracking, and radar tones fall
silent inside minimum range. Minimum range does not block missile release.
All-aspect IR can growl and lock head-on within its shorter acquisition range;
see [missile aspect rules](spec/missiles.md#infrared-aspect-and-acquisition).
[Behavior and current route limitations](spec/weapon-navigation-selection.md).

## Ownship damage report

D reports current aircraft damage and system readings through the bottom-center
sim log. The rebindable `damage-report` action does the same. Damage notifications
share that log; the Systems instrument retains its four gauges and two fuel rows.
The separate `damage-player` fixture remains a development action, including
existing controller profiles. See [systems behavior](spec/systems-damage.md).

## Quick Mission group objectives

Click the highlighted objective in a friendly or enemy group's briefing sentence
to choose its primary group, free fire or other duty. Right-click cycles
backward. Click its survival field to toggle required/optional. Tab/Shift-Tab
and arrow keys reach all objective and survival fields; Enter activates them.
Shift-4 shows only `Obj: Survive` for a protected/required friendly or `Obj:
Destroy` for a designated enemy objective. Other contacts have no objective
label. [Assignment rules](spec/ai-awareness.md#quick-mission-objective-stamps).

## Ejection

Press **Shift+E twice** to eject. The first press asks for confirmation; holding
the key does not confirm. The eject action can be rebound in Controls. A living
pilot can escape an already destroyed airborne aircraft. The camera follows
the pilot and parachute while the abandoned aircraft falls independently.
Low or inverted escapes can be fatal. See [ejection behaviour](spec/ejection.md)
for timing, survival and the AI recovery assessment. Undamaged AI aircraft above
200 feet AGL never eject automatically, as requested by John on 2026-09-23.

## Flight view shortcuts

F1/F2/F3 remain Forward/Back/Up and F10 remains External. F4 tracks the target;
F5 faces the nearest inbound missile; F6 faces a wingman, and pressing it again
moves to the next; F7 faces the target from the player, with a bearing compass; F8 faces the player from the target; F9 is a fixed fly-by;
F12 follows the last player missile toward its own target. V saves the current
camera into Other View and opens Shift+3. F11 still opens keyboard help.

Alt+view selects a target reference and Ctrl+view a last-missile reference.
These are separately rebindable catalog commands (`key:Alt-F7`, for example).
Alt+F4 remains protected Exit; `view-target-track` is available without a default
binding. Unmodified view keys restore the normal reference. Missing subjects
produce feedback without changing the requested selection's predecessor.
Automatic views control direction; pan/orbit remains available in F1/F2/F3/F10.
See [view behavior and fitted rules](spec/flight-views.md).

## Replay viewer shortcuts

The [replay viewer](REPLAYS.md#viewer) has its own built-in keys, an agent
design from 2026-09-26. They are listed in the
[controls master list](CONTROLS.md#built-in-controls-outside-the-tables) and on
the Replay sheet of the [keyboard map](tore-keyboard-map.html#replay). They
remain built in except the drone commands. **Controls > Replay drone** configures
mode cycling, follow/free selection, movement, boost, look hold and speed changes.
Keyboard/mouse drone bindings are separate from flight bindings. Profiles store
them under the replay-keyboard and replay-mouse device names. Custom controller
bindings use their ordinary device identity, and act only in the replay.
The drone still moves while playback is paused; the Escape menu stops input.

- **Playback:** Space plays or pauses. J, K and L play backwards, pause and
  play forwards, as in video editors; J or L again doubles the speed up to
  16x. Up and Down step through 1/8x, 0.25x, 0.5x, 0.75x, 1x, 2x, 4x, 8x and
  16x in the current direction. Left and Right jump 5 seconds, or one tick
  while paused; with Shift, 30 seconds. Home and End go to the start and end,
  Page Up and Page Down to the previous and next timeline marker.
- **Cameras:** Tab and Shift+Tab pick the aircraft. F1 to F10 and F12 give the
  flight views from it; F11 is not a view. O and Shift+O look at the next or
  previous aircraft, weapon or ground object present now. Keypad 5 or Shift+/
  recenters the look, and plus and minus zoom. Backquote switches to the drone,
  which W, A, S, D, E and Q fly; Shift is four times faster.
- **Panels and display:** I, F and G open or close the selected aircraft's AI
  thinking, telemetry and missile guidance panels, C the Comms panel; M opens
  its right-click menu, and X closes every panel. N, T and R switch name
  labels, the mission timer and flight path trails; Shift+R steps the trail
  length. B draws or hides the laser spots, designations, guided weapons and
  bomb landings in the 3D view ([the marks](REPLAYS.md#laser-and-bomb-marks)).
- **Screenshots:** H hides the whole interface and the pointer while playback
  and camera keys keep working. P saves the view without the interface as a
  PNG.
- **Pause menu:** Esc opens flight's Escape menu over the replay, pausing it;
  the arrows, Tab, Enter, Space and clicks work it as in flight, and Esc at
  its top level plays on as before. See
  [the pause menu](REPLAYS.md#pause-menu).

As in flight, letters are read by their physical key and the keypad works as
the navigation keys whatever NumLock says. Alt+Enter, Alt+F4 and, on macOS,
Command+Q still work. The viewer ignores every other Ctrl, Alt or Command
combination, so Ctrl+B marks a moment only in flight.

During flight preparation, flight controls remain paused. Escape cancels a
multiplayer join or pending content check and returns to the connection screen.
Window close and resize remain available. See [loading screen](spec/loading-screen.md).

### Multiplayer menu navigation

The [multiplayer menu specification](spec/multiplayer-menus.md) defines the
shared ? dropdown: F1 or controller Menu opens it, arrows and Enter/A choose,
M returns to main and X exits while open. Controller Left/Right move between
lounge controls; Up/Down scroll the focused Games or Players list. Settings
Close/F10 applies its draft and Escape discards it.

Internet Lounge Options uses Tab and Shift+Tab between fields and buttons.
The Master server field is followed by Default; Enter or Space on Default
restores the public address in the draft. Enter in a field or OK saves, and
Escape or Cancel discards the draft. These are the existing menu inputs.


The dedicated-server [feedback form](spec/replay-feedback.md) uses existing
menu inputs, with Tab between field and buttons and Escape to cancel. Its
on-screen keyboard accepts the controller's existing menu directions and
accept/back buttons. No global flight binding is added.

While the multiplayer score board (K) is open, Ctrl+PageUp and Ctrl+PageDown
page through its human and AI rows. These shortcuts leave ordinary flight
controls unchanged when the board is closed. See [multiplayer](MULTIPLAYER.md#debrief).

## Air target selection

T and Shift+T cycle the displayed own-radar TWS contacts, friendlies included.
Enter cycles visible aircraft left to right; Apostrophe selects the aircraft
nearest the centre of the forward view. The two visual actions can be rebound
separately. Neither requires a radar return. See the full
[target-selection rules](spec/radar.md#target-selection-keys).
