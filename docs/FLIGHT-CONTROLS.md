# Desktop flight controls and Escape menu

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

On the [F/A-XX concept](spec/fa-xx.md), normal rudder input opens the split
flap on the commanded side. F-22 bay and device controls remain available.
**H** deploys or retracts its added hook over three seconds. The hook starts
stowed and is completely hidden when retracted.

The F/A-18D cockpit now covers the full flight canvas. The world renders behind transparent cockpit artwork and independently toggled instrument windows. There is no half-height viewport or opaque lower PANEL fill. Menus retain the proportional 640×480 canvas. This is still a development flight adapter; [behaviour provenance](behavior-provenance.md) records which flight and system components are spec-derived, native, fitted or opinionated.

Start with `cargo run --locked -p tore-app -- --free-flight`, or Choose Activity → Create Quick Mission → OK. Free flight skips loadout and starts with clean external stations. On a MacBook, use **Fn/Globe with the function keys** when macOS assigns those keys to system actions. Fn-Right and Fn-Down supply End and Page Down, and Fn-Delete supplies Delete; there is no Insert key, so bind chaff yourself in **Pref → Controls...**. The physical US key positions are used in flight, including shifted numbers and Option combinations.

Rudder now turns the air-relative flight path through sideslip force and roll
coupling. The HUD shows available NSW percent below BRAKE while on the wheels and hides the readout when displayed authority reaches zero.
Steering turns the nosewheel and steerable strut, leaving separate braces fixed.
Nosewheel steering has 90-degree authority through 10 mph, fading
linearly to zero at 50 mph ground speed (John widened the fade from 25 mph on 2026-10-12; opinionated). [Rules and fitted constants](spec/lateral-flight.md).

## AC-130 directed guns

On the AC-130 the TARGET CAM page (instrument 4) is a gunsight. The camera
looks 90 degrees abeam to the left and 25 degrees down until you do something
else. Slew it with Alt + the arrows and zoom with Shift+' / Shift+;. Backslash
designates whatever is under the crosshair, an object if there is one and the
ground if not; Shift+Backslash pins the ground there; L or ; drops the target
or pin, and pressed again sends the view back to the default. The guns follow
that point inside their arcs, a ring and dot (the pipper, with a bold range arc
like the fighter's LCOS pipper) shows where rounds will land, and a box on every
view marks the point. The AC-130 tracks ground
and air objects at any range, as if Easy targeting were always on. On a
gamepad hold View: the right stick slews, A designates (tap) or pins (hold),
D-pad up and down pick and link a gun. [Keys](INPUT.md#ac-130-gunsight-controls),
[behaviour](spec/ac130-linked-guns.md#the-gunsight).

Use Ctrl+7 to choose a gun candidate and Ctrl+8 to include or remove it from
the firing group. Hold Fire to release every enabled linked gun: it fires with
or without a target or solution, along wherever the barrels point, and only
SAFE, EMPTY, a lost gun or the aircraft's own wing in the way holds it. The
Weapons instrument's LINK row shows membership and its ordinary arrow
identifies the current candidate. Each gun has its own ammo, cadence,
source-informed arc and fitted tracking slew. These are player-directed
controls; [limits and gamepad defaults](INPUT.md#ac-130-directed-gun-groups).

## VTOL, tiltrotors and helicopters

Use the ordinary arrow stick, End / Page Down rudder and E engine switch. Some
keys change with the aircraft ([the full table](INPUT.md#vtol-tiltrotor-and-helicopter-controls)):

- **AV-8 and Yak-141**, the retail keys: **X** turns the nozzles down 10
  degrees and **Z** up 10. **Shift+X** puts them vertical (90 degrees), and a
  second press to the 100-degree braking stop; **Shift+Z** puts them aft (0),
  or from the braking stop back to vertical. Ctrl+Up / Ctrl+Down slew them,
  **0** sends them aft. Z and X are not rudder keys on these two; use End and
  Page Down. 1-8 set engine power as on any jet. Per the manual: Shift+X and
  full power to lift off, Z three times at 500 ft, Shift+Z past 80 to 90 kt;
  for a short takeoff, X four times (40 degrees) at 80 to 90 kt.
- **Helicopters (AH-64, Mi-24, CH-47) and the V-22**: the throttle keys 1-8
  (and a throttle lever or the gamepad throttle buttons) set the
  **collective**; the engines are governed. Ctrl+Home / Ctrl+End still move
  the collective. **Ctrl+arrows trim the cyclic**: a tap moves the trim 2
  percent of travel, a held key 10 percent a second, so a few taps of
  Ctrl+Up and the aircraft flies forward hands off. **0** recentres the trim on
  the helicopters; on the V-22 it still sends the nacelles forward. V-22
  conversion uses Ctrl+Page Up toward airplane mode and Ctrl+Page Down toward
  helicopter mode. Z and X stay rudder keys.
- **All six**: **Ctrl+Shift+A** cycles the stability level, announced as
  `Stability: Off`, `Damper` or `Attitude`; Pref → Stability level does the
  same and the choice is kept for later flights; a right click on the row turns
  the level back (Off, Attitude, Damper). **Damper** (the default)
  stops the aircraft rotating when you let go of the stick and leaves it at
  the attitude you left it; **Attitude** also returns it to the trimmed
  attitude, and at the Attitude level Ctrl+arrows move that attitude
  instead of the trim; **Off** adds nothing. No level limits the attitude:
  full stick always wins. **Ctrl+Alt+A** is hover hold on the helicopters
  and the V-22 below 40 kt: it stops the drift, then holds the spot, the
  heading and the height above the ground, through the controls. Any stick,
  pedal or collective input cancels it; Ctrl+arrows move the held spot 10 ft
  a tap. At 40 kt and above, **A** and **Ctrl+A** fly the helicopters and the
  V-22 (height on the collective, speed on the cyclic, Ctrl+Up / Ctrl+Down
  2 kt faster or slower). See [hover hold](spec/autopilot.md#hover-hold-helicopters-and-the-v-22).

A stick button bound to **Trim set** makes the current stick plus trim the new
trim and ignores the stick until it is back near centre, which turns a
spring-centred stick into a force-trim cyclic. The controls editor exposes the
lift demands, the trim axes and every new command as bindable rows. What each
aircraft does, the stability levels, the Easy flight physics cheat, the numbers
and their sources are in [powered-lift flight](spec/powered-lift-flight.md).

The Apache and Chinook keep their fixed landing gear down; G reports that the
gear is fixed. The Hind retains retractable gear. This is an explicit
[fitted equipment rule](spec/variety-flight.md), with no new binding.

## Working flight commands

| Key | Action | Evidence/status |
| --- | --- | --- |
| Arrows | Pitch/bank; Down pulls up | FA keyboard stick ([keyboard spec](spec/keyboard.md)) |
| Shift-M | Toggle live map; Escape closes it | Right-side category toggles, buildings off; [map rules](spec/flight-map.md) |
| End / Page Down, or Z / X | Left/right rudder while held | FA keyboard rudder; Z / X are a T.O.R.E second pair ([key placement](INPUT.md#key-placement)) |
| 1 / 2 / 3 / 4 / 5 | Throttle idle / 25 / 50 / 75 / 100%, afterburner off | FA keyboard throttle |
| 6 | Throttle 100% with afterburner | FA keyboard throttle (101%); afterburner requires engine, fuel and an afterburning aircraft |
| 7 / 8 | Throttle down / up 5%; 8 past 100% lights the afterburner, and any setting at or below 100% turns it off | FA keyboard throttle |
| Shift-B / E | Afterburner switch / engine toggle | T.O.R.E additions on keys FA leaves free; afterburner also needs >95% throttle |
| Shift+E twice | Eject, release between presses and confirm within 2 seconds | Manual p. 161; [ejection](spec/ejection.md) |
| O | F-22 main weapon bays | FA bomb-bay key; fitted 1-second presentation; other aircraft ignore it |
| G / F / B / H | Gear / flaps / airbrake and wheel brakes / hook | FA keys |
| Insert / Delete | Release one chaff / one flare; keypad 0 and period too | FA keys; [countermeasures](spec/countermeasures.md) |
| R / J | Radar / jammer | R returns to the radar channel when infrared is selected, and otherwise toggles radar power. Radar power gates radar contacts and locks; powered ECM applies recovered contact-probability terms |
| M | Cycle the available sensor channels | FA's HARM key; radar and the installed infrared sensor; with no infrared installed it reports it unavailable and leaves radar selected |
| I | Select the infrared channel | Passive: it stops radar transmission without moving the radar power switch |
| Y | Toggle scope contact history | Draws past observations as dimming dots; see the [sensor component](radar.md) |
| N | NAV / ILS mode | FA HUD navigation key |
| W / Shift-W | Next / previous waypoint on the NAV window | FA keys; the NAV window's airport list when it shows airports |
| ; or L | Clear the designation | FA unlock key; L is kept |
| F1 | Forward cockpit view; reset pan/zoom | FA `FMENUD.MNU` |
| F2 / F3 | Look back over the airframe / up | FA menu; turns with the aircraft, forward artwork projects out of view naturally |
| F4 | Track current target within head-look limits | Manual p. 103; fitted limits |
| F5 / F6 | Player to nearest inbound missile / wingman; F6 again for the next wingman | Manual p. 103; fitted placement and cycling |
| F7 / F8 | Player to target, with a bearing compass / target to player | Manual p. 103; fitted placement; compass requested 2026-09-28 |
| F9 | Fixed-position fly-by; press again for another pass; a new point once the aircraft is 3 nmi from it | Manual p. 103; fitted placement; opinionated 3 nmi reset (requested by John, 2026-09-28) |
| F10 | External chase view | FA menu; authored camera placement |
| F12 | Last player missile to its own target | Manual p. 103; fitted placement |
| Alt + view key / Ctrl + view key | Reference selected target / last player missile; Alt+F2 looks back over the target's own airframe; Alt+F4 still exits | Manual p. 104; [view rules](spec/flight-views.md) |
| V | Save current camera into Other View and open its window | Manual p. 89 |
| Shift + arrows | Cockpit look-around; exterior orbit | FA manual p. 104; Ctrl + arrows is FA thrust vectoring and does nothing yet |
| Keypad 5 or Shift + / | Recenter look/orbit without changing view or zoom; also recenters a head tracker | Keypad 5 is FA's; Shift + / is a T.O.R.E second key |
| Hold right mouse button and drag | Mouse look, with the same limits as keyboard look | Opinionated agent choice, 2026-09-22; see [input](INPUT.md#mouse-look) |
| Head tracker (opentrack UDP 4242) | Turns the view on top of keyboard, stick and mouse look, with the same limits | See [head tracking](INPUT.md#head-tracking-and-trackir) |
| + / - | Zoom view | USNF manual; authored 0.5–4× projection |
| Backspace | Toggle cockpit art, retain HUD/windows | FA menu (`BS`) |
| Shift-U | Toggle HUD | Development shortcut |
| Shift-[ / Shift-] | Dim / brighten HUD, 33 settings | FA menu |
| Ctrl-H | Cycle HUD green / amber / white / cyan | Authored |
| Ctrl-I then -/+ | Instrument and cockpit brightness, five levels, default 4 | Requested, authored chord |
| Ctrl-Shift-F then -/+ | Red cockpit flood, Off and five levels, default 3 | Requested, authored chord |
| D | Report ownship and systems damage in the sim log | Manual p. 161; requested summary |
| Shift-0…9 | Toggle instrument windows (four large or six small) | FA menu; oldest open window is replaced |
| Comma / period | Decrease/increase scope range | USNF manual; applies to the RWR or the RCS page if either is the last opened window, and to the radar scope otherwise |
| C / Shift-C | Cycle 1×/2×/4×/8× time / select 0.5× slow motion | FA keys; fixed 120 Hz ticks, authored adapter time scaling |
| A / Ctrl-A | Toggle heading/altitude hold / waypoint autopilot | [Autopilot behavior](spec/autopilot.md), requested USNF-ATF modes |
| Ctrl-Alt-A | Toggle hover hold (helicopters and V-22, below 40 kt) | [Hover hold](spec/autopilot.md#hover-hold-helicopters-and-the-v-22), requested 2026-10-08 |
| Ctrl-P | Pause/resume | FA menu |
| Escape | Open/close in-flight menu; return one level from submenus/help | FA menu/manual |
| Ctrl-Q | End mission and return to creator | FA menu; does not quit the application |
| Alt-F4 / Command-Q on macOS | Exit to desktop | FA menu / macOS app shortcut |
| F11 | Open keyboard help | Development shortcut |
| Alt-Enter | Switch between borderless fullscreen and the previous windowed size | Opinionated, requested by John on 2026-09-22; F11 is already keyboard help, so the window mode uses Alt-Enter alone |

These replace the earlier provisional **A/D rudder, +/- throttle, T afterburner, F2/F3 external views**, and on 2026-09-26 the development throttle (1…0 for 10…100%, Page Up/Down) and Z/X-only rudder. The headless simulation still uses the same deterministic state model; desktop key translation is separate.

Autopilot leaves throttle manual. Stick or rudder input above 15% disengages it.
Switching modes retains the captured altitude and heading. With no waypoint
system yet, Ctrl-A holds that heading and shows `AUTO` above `WP --`. Once supplied, a waypoint is labeled `WP <number>`.
Heading hold shows `AUTO` above `HDG ALT`, beside the heading tape.

## Instruments

**Escape → Pref → Large windows?** toggles the layout. Large is the default: four corner windows, with size and margins based on the 162×160 window and eight pixels at the 640×480 reference size. They anchor to the actual screen edges, including on wider displays. The initial arrangement is Systems top-left, RWR bottom-left, Radar bottom-right and Radar/Visual top-right, following the supplied large-style reference.

Small places six windows across the bottom in two groups of three. Their reference size is 96×95, with six-pixel gaps and eight-pixel outer margins. Each group anchors to its screen edge; the center gutter grows on wider displays. Its initial pages are Systems/RWR/Nav on the left and Radar/Visual/Radar/Weapons on the right. Each layout remembers its own selected pages for the session. Shift-0…9 toggles pages; opening beyond a layout's capacity replaces its oldest page. Launching a seeker weapon also opens the target camera (Shift-4) if it is closed: at the top right in the small layout, to the left of the top-right window in the large one, without closing any window ([seeker window](spec/target-window.md#seeker-window)). Switching layouts cancels any pending instrument click. Button hit testing uses the same scaling and rectangles as rendering.

The Envelope window (Shift-1) uses U for the current G curve, A for all positive-G curves, and C for locked-target comparison. Red marks the target's advantage. It shows clean-aircraft capability, with live altitude, G and speed readouts and a color-cycling square. Each aircraft sets its own chart scale. Missing comparison data is labeled explicitly. See the [envelope spec](spec/envelope.md) for fitted colors, scale and marker timing.

Lighting choices are saved between flights. Adjustment chords expire after four seconds
or another key; Ctrl may remain held. All commands are bindable in Controls > View,
and PREF > Cockpit lighting provides individual adjustment rows. Cockpit paint and
bezels fade with the world sunlight and moonlight; screens stay self-lit. The red
flood lights the darkened paint at night. Generated bezel legends have a subtle
backlight controlled by instrument brightness. See [cockpit lighting](spec/cockpit-lighting.md).

Each window is a 162×160 raster: the flying aircraft's own original frame at double size, with its title, number and button letters in the aircraft's HUD colours ([bezel spec](spec/instrument-bezel.md)). It is resampled directly to the flight overlay resolution. There is no intermediate 96×95 reduction, so small-window text retains source strokes on larger displays. These are fitted layouts, not the original placement, which uses 10- and 14-pixel margins and an 81×80 small window ([window placement](spec/instrument-bezel.md#window-placement)). Sizes and margins scale by the smaller of width/640 and height/480. Use `--instrument-layout large` or `--instrument-layout small` for startup or repeatable GPU captures.


Shift-1 Envelope; Shift-2 Forward View; Shift-3 Other View; Shift-4 Radar/Visual; Shift-5 RWR; Shift-6 Navigation; Shift-7 Systems; Shift-8 Weapons; Shift-9 Radar; Shift-0 Radar Cross Section. Forward View draws a short horizon bar centered on the nose, the flight path marker and plain TAS and MSL readouts over the picture in the HUD's primary color, with no pitch ladder; this is an opinionated addition requested by John on 2026-09-22. The marks use the same camera angles as the picture and refresh with it, about ten times a second. Page 0 now draws the exposure contour, received emitter symbols and its view scale; its buttons are `-` and `+`. Page 9 buttons are `-`, `+`, `M` for the channel cycle and `Y` for history. Hovering a contact on page 9 marks it with the selector corners, a click on it designates it, and an empty click leaves the current designation alone. Empty scopes and NO TARGET are intentional in target-free flight. Systems now shows live damage-driven TEMP/OIL/HYD, internal FUEL and external tank fuel. D reports damage in the bottom-center sim log. [System failures and fitted rates](spec/systems-damage.md).

The scope, the exposure page and the weapons all read one shared sensor
component. [What it models, what is authored tuning and what is deferred](radar.md).

## Friend or foe

Slice F2-C of [multiplayer phase 2](ARCHITECTURE.md#friend-or-foe). All three
work in single player and in a network flight, from the flight's own copy of the
mission.

- **The lock box's X** is drawn when the displayed target is on the side of the
  plane the screen presents: a player flying for the enemy side sees it on the
  enemy's aircraft. Single player always flies the friendly side.
- **U** squawks IFF on the displayed target: "IFF: Friendly" for an aircraft on
  the player's side, "IFF: no reply" for any other (agent decision: the retail
  manual names only the Friendly answer), "IFF: no target" with nothing
  designated.
- **Show Target Info** (Pref menu row, or Ctrl+T; off by default) prints each
  visible aircraft's and object's identity below it, in the HUD's font and
  size. The displayed target also shows its manoeuvre (the target window's
  activity). The text is orange, and red for a target that aims at the player.
  In a network game a human's callsign is the line beneath. Agent decisions: a
  label reaches 10 nautical miles, at most 24 show at once (aircraft first, then
  the nearest objects), and the manoeuvre and the red are known only for the
  displayed target, the only one a client's copy of the mission knows.

## In-flight messages

Cockpit, radio and system messages print in the HUD's color, with no
background, centered at the bottom of the window. The newest line is at the
bottom, its text 5 pixels (at 640x480) above the window's edge; each stays five seconds and at most seven show at once, so a burst
pushes the oldest off the top. Repeating a message that is still shown moves it
to the bottom with a fresh timer. The text uses the HUD's own font (`HUD11.FNT`)
at the HUD's on-screen size, with the same smoothed edges. Opinionated, requested by
John on 2026-09-26 as the retail look; the original's own timing, line count
and size are untraced. The debug panels (Pref → Debug panels?) end above
the band the seven lines can fill, and a mission replay prints the recorded
messages the same way just above its transport bar
([mission replays](REPLAYS.md#interface-parts)).

## Recovered commands awaiting their systems

All shortcut labels present in the supplied `FMENUD.MNU` are recognized, and the FA in-flight key table is recovered in the [keyboard spec](spec/keyboard.md). This is **not a claim that every original command or system is ported**. FA keys whose systems T.O.R.E does not have yet display a short message:

| Key | Original action / remaining work |
| --- | --- |
| Shift-R | Air-to-ground radar |
| Shift-A / Shift-G | AWACS / air-to-ground radar link |
| Shift-F / Shift-D | Target damage / message history |
| Shift-I | Airbase aircraft inventory |
| Shift-J | Jettison external fuel |
| Alt-F | Wingmen engage the IR/laser-designated target |

Tab (gun), M's HARM seeker, Z/X wing sweep, Ctrl + arrows thrust vectoring and / and \ laser designation have no T.O.R.E binding yet ([key placement](INPUT.md#key-placement)).

Space now holds the selected player trigger. Bracket keys select the previous/next weapon or NAV; T cycles displayed own-radar TWS targets, Shift-T cycles back, Enter cycles visible aircraft left to right and Apostrophe picks nearest the forward-view centre. `--live-fire` enables the
explicit PT-default test range; backslash resets its target at a suitable range
for the selected weapon. Ordinary free flight loads the aircraft's supported default weapons. Airborne
starts select and arm the canonical gun; ground starts enter NAV with weapons disarmed. [Startup rules](spec/hud-layout.md). The restricted native research adapter stays clean.
The complete native weapon/countermeasure dispatch remains unverified.
[Live-fire scope and approximations](baselines/live-fire.md). USNF manual bindings are reference evidence pending FA-specific verification; FA menu labels take precedence.

## In-flight menu

The runtime reads **? / Control / Pref / View / Window / Cheat / Multi / Pos**, including nested options, from the imported FA menu module. Arrows/Tab and Enter/Space navigate; Right opens a child menu, Left backs out or changes the top menu, and Escape backs out before closing. Mouse activation requires a matching press and release. Hover/focus remains silent.

The menu pauses flight and engine loops. Focus loss pauses and clears held controls; resume explicitly with Ctrl-P or the menu. Closing the menu preserves a pre-existing explicit/focus pause. Opening menus never advances a hidden backlog of simulation time.

**Escape → Pref → Graphics...** opens the Graphics options screen over the paused flight, and **Sound...** the retail Sound/Music Prefs dialog; closing either returns to the menu. Graphics changes apply to the view at once. The menu's tabs, rows, bottom buttons and keyboard help are drawn by a widget the [replay viewer's pause menu](REPLAYS.md#pause-menu) shares (`pause_menu.rs`); flight keeps its own rows, actions and help text.

**Escape → Pref → Weapon diagnostics?** shows or hides the upper-right weapon diagnostic panel: launch mode, seeker status, RELEASE LOCK, range, closure, estimated flight time, target aspect and the three most recent guided shots. It is off by default, reads On or Off beside the row, shows a short "Weapon diagnostics: on/off" message and is saved with the other flight preferences. With it off, the large layout's top-right instrument sits in its normal corner and the panel's click areas do nothing. The row is an authored addition after the retail Pref rows, not part of `FMENUD.MNU`; opinionated, requested by John on 2026-09-23 (the label and On/Off readout are agent choices). `--weapon-diagnostics` starts a launch with it shown, including captures.

**Escape → Pref → Debug panels?** turns on the debugging aids of the
[mission replays](REPLAYS.md#debug-panels) in flight: the mission timer at the
top of the view, a right-click menu on any aircraft or missile (a right-drag
stays mouse look), and the AI thinking, Telemetry, Guidance and Comms panels
it opens, drawn from the flight as it happens. It is off by default, reads On
or Off beside the row, shows a short "Debug panels: on (right-click an
aircraft)" or "Debug panels: off" message and is saved with the other flight
preferences. It changes nothing about how anything flies. The row is an
authored addition after Weapon diagnostics?, not part of `FMENUD.MNU`;
opinionated, requested by John on 2026-09-26 (the label and messages are agent
choices). `--debug-panels` starts a launch with it on, and `--flight-panels
thought,telemetry,guidance,comms,menu` also opens those panels, for captures.

**Escape > Debug > Slow-frame log** turns performance logging on or off for
this session, including in joined games. It starts at 50 ms. Send the current
session log after a slowdown. [Usage and log locations](DEVELOPMENT.md#diagnosing-slow-frames).

**Escape → Debug** is a tab after the retail ones with a slow-frame log switch, two AI test settings and
a Back to defaults row, for measuring what the AI's observation costs and how it
fights without part of it. *AI observation rate* cycles 120 Hz, 60 (the default
since optimization round 2), 40, 30 and 20: AI aircraft observe that often, staggered across ticks, while
their controllers and flight stay at 120 Hz. *Lean AI sensors* (On by default,
and decisions are exactly those of the full sensors) leaves out the visual
channel list and builds the received-radar list only when a missile warning
needs it; Off gives the full sensors. Both apply at the next tick, are not
saved, and are kept across a restart of the flight. In a game the player
hosts they govern that game's AI; in a joined game the rows are dimmed (they
read Host) and do nothing. The tab is an authored addition, not part of
`FMENUD.MNU`; opinionated, requested by John on 2026-10-19 (the labels are
agent choices). `--ai-observe-hz` and `--ai-lean-sensors [on|off]` start a launch
with them set. Details:
[AI test settings](ARCHITECTURE.md#ai-test-settings-observation-rate-and-lean-sensors).

Working menu actions include views, instrument windows, time/pause, cockpit, pitch ladder, weapon diagnostics, debug panels, HUD brightness, ending flight and exiting. Sound currently toggles effects; the original volume mixer is not implemented. Working cheats are listed under [cheats](#cheats). Other preferences, cheats, multiplayer and position commands are navigable placeholders with feedback. The bottom Resume / Restart / Keyboard Shortcuts actions are documented development additions. Menus do not silently enable unsupported cheats or alter the aircraft when an unrelated modifier shortcut is pressed.

With multiplayer `respawn revive`, the running-flight aircraft/loadout selector
uses this same menu widget and navigation. It does not pause the mission. Done
stages a draft for the normal Enter-to-revive request; Escape discards menu edits.
The timer remains visible. See [multiplayer revival](MULTIPLAYER.md#ai-aircraft-and-player-choice).

## HUD and presentation limits

The HUD uses imported `HUD11.FNT`; instrument/menu text uses `WIN11.FNT`. It shows wrapped heading, true airspeed in knots, MSL altitude, G, throttle, afterburner and actual gear/flap/brake/hook state. AGL and vertical speed are reserved for ILS approaches. The enlarged pitch ladder uses five-degree steps, dashed below zero, and a literal degree scale (equal spacing, about 7.26 pixels per degree at unit zoom, no compression or growth) whose spacing and motion follow actual pitch and bank, including readable +/-90-degree marks. The zero bar is the ladder's zero rung on that same scale and meets level-flight velocity to within a few pixels. The flight-path marker comes from current kinematic vertical speed and airspeed. No target, weapon solution or navigation waypoint is invented.

Layout, line symbology, frame scaling, pan, zoom and camera placement are authored. `~F18H.PIC` is uniformly scaled to cover the actual flight aspect ratio, showing more side artwork on wider screens and cropping only what is required to avoid stretching. Mirrors render live rear views every visible frame. ILS appears only with the runway threshold inside the aircraft's full 90-degree
forward cone and existing 5-NM/4,000-foot airport-relative band. The airport
service publishes tested localizer and glide deviations, but dedicated ILS HUD art remains open. Native F18 HUD callers, HUDSYM glyph meanings, full cockpit composition, corner-speed/weapon modes and native pixel parity remain open. The HUD remains aligned with the aircraft-forward datum and pans opposite head-look with the cockpit. External and remotely referenced views omit it; F4 keeps the player cockpit and nose-aligned HUD while the camera tracks the target.

See [recovery details](formats/aircraft.md), [progress](research/progress.md), and [validation](baselines/cockpit-controls.md).


The flight overlay is independent of the fixed menu canvas and tracks the window aspect. It is composed at the physical drawable size, proportionally capped at 1920×1080 for bounded CPU/GPU work. At centered forward view, cockpit art covers that entire overlay; menus remain centered at their original proportions. The HUD uses a 0.7225 layout scale, an additional 15% reduction from the previous 0.85 scale, with projection compensation for angular cues; the pitch ladder is a literal degree scale with no compression. TAS and MSL primary numbers have transparent backgrounds; surrounding tape numbers and hash marks are omitted. With the cockpit visible, HUD symbols are clipped to its reviewed glass aperture
and drawn behind the cockpit frame. Cockpit-off and below-1x wide views retain
the independent HUD. Static cockpit artwork, glass masks and unchanged
instrument rasters are cached.

`--window-size 1280x720` selects an initial logical window size for inspection (minimum 640×480). Flight captures now preserve that window's aspect and the capped overlay resolution; terrain-only captures remain 960×720. See [responsive validation](baselines/responsive-flight-ui.md).

## View and smoothness clarification

F2/F3 replaced the early prototype exterior bindings when the native menu controls were recovered. They look back/up from ownship, turning with the aircraft; F2 shows the airframe's spine and tails, F3 omits it. The forward cockpit/HUD overlay translates opposite head-look and fades at its viewing limits; back/up views do not duplicate the forward frame behind or above the pilot. **F10 shows the aircraft from outside**; F1 restores the cockpit. The oblique developer camera remains available through `--flight-view 2`. Other View (Shift+3) defaults to Back. Press V to save the current view, reference, look and zoom there; it opens automatically and keeps following that relation after you return to F1. Missing subjects leave a requested view unchanged, or return an active main view to F1 with feedback. See [all views and fitted placement](spec/flight-views.md).

The performance pass removes the extra post-render wait, interpolates camera/aircraft/HUD poses between fixed 120 Hz ticks, and keeps live instrument GPU readbacks asynchronous. Controls still drive the same authored flight adapter; this is not a new native flight-model claim. [Measurements and diagnostics](baselines/flight-performance.md).

## Look-around and exterior orbit

Hold **Shift + arrows** to turn the camera at one radian/second. In the cockpit, Left/Right turn around horizontally; Up looks upward as far as overhead. Down returns toward the forward eye line and **cannot look below it**. This limit is relative to the aircraft's forward pitch, not an altitude or world-horizon constraint. In F10 exterior view, arrows orbit around the aircraft in both axes, including below it and over the poles; the aircraft stays centered at a constant distance. Vertical orbit can make the view inverted as it crosses overhead. No ground-collision constraint is added to this inspection orbit.

Release the arrow to stop moving the view; its orientation stays where you left it. **Keypad 5** or **Shift + /** recenters the current camera without changing view or zoom. **F1** returns to the forward cockpit and resets look/zoom. A look arrow remains claimed until physical release even if Shift is released first, so a repeated key cannot unexpectedly pitch or roll the aircraft. Pause/focus loss clears held input. Shift-/ uses the physical slash key, so US keyboards may label the resulting character `?`.

Head-look rotates about the aircraft’s axes, including during banked flight.
Cockpit artwork and HUD remain pointed straight ahead relative to the aircraft.
Looking right moves both left on screen; looking left moves both right. Their
shared translation follows the projected aircraft-forward datum and never
clamps to available artwork width. The image remains flat, so horizontal look
keeps its bottom level rather than tilting it like a nearby plane. Looking up
moves it downward. Both fade from 45–65 degrees horizontal look and 35–55 degrees
upward. These are fitted presentation rules, not recovered native FA projection.
Instruments remain screen-anchored.

+/- scales artwork and HUD text/symbology with world zoom. Zoom crops around the HUD
center at the screen center in forward view, including below 1x. Below 1x,
cockpit artwork and mirrors disappear; HUD and instrument windows remain. At
1x or above, artwork returns if the cockpit toggle is enabled. Head-look still
translates the aircraft-forward datum with the camera. The finite source art provides no rear/overhead interior.
Mirrors use their original source silhouettes with live rear views. See [sliding cockpit validation](baselines/cockpit-slide.md).

The FA manual, printed page 104, specifies Shift+arrows for keyboard panning and right Shift plus joystick for joystick panning. FA uses Ctrl+arrows for thrust vectoring, so the earlier USNF Ctrl+arrows look alias was removed on 2026-09-26. [View evidence](spec/flight-views.md), [existing look validation](baselines/look-around.md).

## Flight response and vertical flight

The development adapter now carries an independent world-space velocity vector. Thrust, drag, lift and gravity accelerate that vector rather than setting it to the nose direction each tick. Pitch/roll controls have finite response, and the HUD flight-path marker uses both lateral and vertical velocity. The nose and actual travel direction can differ. These response constants are authored, not recovered native FA control laws.

Attitude rotates as an orthonormal basis and is interpolated in that basis. The old ±1.5-radian flight pitch clamp is removed; flight can pass through vertical/inverted attitudes and complete loops with sufficient energy. Cockpit head-look still cannot look below its forward eye line, this separate viewing restriction does not limit aircraft pitch. [Evidence and limitations](baselines/flight-response-sky.md).

## Exterior animations

All 37 playable profiles have bounded headless checks for their applicable
control surfaces and devices. Gear wheels, braces and doors use separate
reviewed attachments; retractable hooks, vector nozzles, nacelles and rotor
cyclic motion have aircraft-specific fits. Remaining source gaps include the
AC-130 hook, individual rotor feathering and A-4E forward flap-branch strips.
See the [fleet audit](baselines/aircraft-animations.md) and
[animation contract](spec/aircraft-animation.md) for coverage and limits.

MiG-23 and F-14 wings sweep visually with speed and flap demand. F-22 bays open
with O or automatically on a ready bay-weapon trigger. Automatic release waits
for the doors, about one second, then they close one second after release;
designating a target alone does not open them. Its exterior canopy remains
amber and 75% opaque, with clear cockpit rendering. These are the existing
[fitted bay and canopy rules](spec/aircraft-animation.md#f-22-main-weapon-bays).


Use **6** for full throttle and afterburner, and **F10** to inspect the model. G/F/B/H animate gear/flaps/airbrake/hook continuously. Pitch/roll inputs move fitted stabilators; the rudder keys move fitted trailing rudders. Engine/fuel/throttle gate afterburner consistently across HUD and audio; flame length has a short visual transition. These reuse original polygons with authored hinges and schedules. [Coverage, captures and remaining work](baselines/f18-animations.md).

During a banked pull, the adapter now retains load-related nose/flight-path separation and accounts for body-yaw turn response. The velocity marker remains projected from actual velocity: coordinated AoA is below the nose, while transient sideslip appears laterally. Native AoA/control-law parity remains open. [Details and probes](baselines/banked-pull-aoa.md).

## Rafale C selection

Select Rafale C by clicking the Wing 1 aircraft name in Quick Mission, or use
`--aircraft rafale --free-flight`. The same input/release, pause and camera
bindings apply to the selected aircraft. Its PT values, cockpit artwork, DEFA
250-round inventory and engine clips come from its own retail profile. This is
the existing fixed-tick adapter using Rafale data, not a complete native flight
model. The Rafale and Hornet retain separate fitted animation rigs, as described
below. See [initial scope and evidence](baselines/rafale-quick-mission.md).

Rafale C: H reports unavailable because the recovered RAF.SH model has no hook
animation import. G/F/B and pitch/roll/rudder animate its own gear, elevons,
airbrakes, canards and rudder; motion schedules are fitted. Aircraft changes now
replace the GPU cockpit texture as well as exterior resources.

## Mirrors and uncapped presentation

Both aircraft have live center/left/right mirrors. They render the world and
ownship into one rear texture each visible frame, with reflected,
aspect-preserving crops inside the original outlines. They follow cockpit pan,
zoom and fade; hiding the cockpit hides the mirrors. Viewpoint/crops are fitted,
not recovered native optics. The render loop requests uncapped presentation;
physics stays at 120 Hz. [Implementation, checks and measurements](baselines/mirrors.md).

## Shared controller input and instrument focus

The authored [controller layer](INPUT.md) accepts gamepads, sticks, throttles,
pedals and button boxes through typed pilot commands. Standard Linux gamepads
have documented default bindings; custom profiles cover other descriptors and
platforms. Keyboard and controller inputs may be assigned together, with explicit
axis priority, throttle pickup and independent release/disconnect handling.
`--list-inputs` and `--monitor-inputs 30` run without a window or retail media.

Ctrl-Tab / Ctrl-Shift-Tab selects the next/previous existing instrument;
Ctrl-1..6 selects a slot and Ctrl-Shift-1..4 operates its stock button positions.
Selection does not move windows or alter their raster content. Unimplemented
controls remain unavailable. These shortcuts are T.O.R.E additions, not native
FA dispatch evidence. Optional rumble and input-only tick tapes are documented
in [INPUT.md](INPUT.md); [acceptance](baselines/input.md) distinguishes physical
hardware checks from synthetic and cross-compilation evidence.

Enable rumble through **Escape → Control**, the controller tab's **Rumble** setting, then **Apply**. Actual
afterburner engagement produces a subtle impulse followed by a quiet continuous
low-frequency rumble while it remains active. The switch alone below the model's activation
threshold does not. Event feedback designs and future weapon/turbulence hooks
are listed in [INPUT.md](INPUT.md); successful gun/missile events now drive the corresponding rumble cues in the live-fire adapter.

The flight **Control** tab opens the same input configuration screen as
**Pref → Controls...** on the main menu, rather than the retail device-selection
stub. Every key, mouse input and gamepad button is listed in the
[controls master list](CONTROLS.md), and every stock key can be remapped. Instrument page sets/layout, scope controls, zoom,
cockpit/HUD options and sound preferences save automatically between normal
sessions and survive aircraft changes/restarts. [Full settings contract](INPUT.md).

## Manual weapons follow-up

`[` and `]` cycle NAV and weapons. Weapon selection arms; NAV disarms.
The HUD status reads NAV, LCOS for the armed gun, or ARM for missiles.
There is no separate master-arm control. **L** clears designation and **K**
jettisons the selected external group in any flight (the AC-130, which carries nothing external, is unchanged). Class and station-fault
fixtures remain available through the controls editor and command-line setup.
Restart repairs/reloads. Selection releases the trigger before another press.
T cycles displayed own-radar TWS contacts nearest first and Shift-T backwards,
including friendlies. Enter cycles on-screen aircraft left to right; Apostrophe
picks the aircraft nearest the forward-view centre. Visual picks need no radar
return and stay while the ownship sees the aircraft, without granting radar
support. A mouse click on the scope designates a contact directly. Radar/click
selections still drop with their sensor contact. See the complete
[target-selection rules](spec/radar.md#target-selection-keys). F4, F7 and F8 keep
their separate visual following rule (10 nmi). SAFE/EMPTY/STATION FAILED and the sensor and range advisories are shown
separately from lock; a terrain-masked target now reports NO TARGET, because
masking clears the contact rather than inhibiting the launch. The systems continuation below adds automatic source-weighted failures
for supported equipment. Quick Mission AI integration is described in the [AI spec](spec/ai.md).

`--record-combat NEW_PATH` records explicit combat-service inputs and commands;
`--replay-combat PATH` replays headlessly with the same aircraft/theater/assets.
This is separate from pilot-input recording and does not re-simulate flight.
[Controls, evidence and remaining gaps](baselines/manual-weapons.md).

## Weapons and systems continuation

**D** reports your aircraft damage percentage, temperature/oil/hydraulic readings,
remaining engine power and failed systems through the bottom-center sim log.
It never damages the aircraft. The `damage-player` developer command and controller
fixture remain available explicitly. In the `--live-fire` range, **Ctrl-Shift-I** spawns one incoming selected source weapon, and **Shift-Y** toggles
target ECM. Those two fixtures moved off I and Y when those keys took over
infrared selection and contact history; the incoming fixture left Shift-I for FA's airbase inventory key.
**J** controls own ECM and **R** radar. The incoming fixture does
not command AI or spend player ammunition. Shift-K selected-group jettison, ; or L clear designation, bracket NAV/weapon cycling,
T/Shift-T/Enter targeting, Space hold fire and backslash target replacement remain available.

Standard Linux pads use **held Select** as the combat layer: RB fire, LB weapon,
A designate, B clear, X previous NAV/weapon, Y own ECM, L3 radar, R3 jettison. D-pad up replaces
target, down requests a player hit, left cycles class, right fails the station.
Select+Start toggles target ECM; Select+Guide spawns the incoming fixture when
the desktop exposes Guide. These suppress the corresponding base flight/menu
bindings. Release before switching layers. Unmodified Start pauses. F10/custom
`view-external` replaces Select's old default exterior action. Exact mappings,
profile migration/editor behavior, unsupported controllers and haptic limits are
in [INPUT.md](INPUT.md#manual-combat-layer-2026-09-14).

Weapons page V/R/E reports visual availability, radar and ECM: `+` available/on,
`-` off, `!` failed. A failed visual sensor now also removes visual contacts, so
the V indicator and the scope agree. Automatic source-index faults are separate from the manual
developer injection. Engine, fluid, control, structural and pilot failures now
affect ownship and use the sim log. The panel retains only its retail gauges
and fuel rows. See [systems damage](spec/systems-damage.md) for exact fitted rates.
[Recovered contracts, runtime evidence and remaining gates](baselines/weapons-systems.md).


HUD brightness now uses the recovered signed range -256..256, steps of 16 and
neutral zero, applied to the original palette entry before sunlight whitening.
The primary ink comes from each aircraft's HUD module; layout remains authored.
Version-1 preferences migrate the old setting relative to its neutral value 7;
version 2 persists the new signed amount. Other saved display choices are retained.

The main HUD shows outlined current TAS/MSL values without surrounding tape
numbers or hash marks, plus a curved bank scale immediately beneath the
pitch ladder (crash/engine-off alerts take priority). Active ILS places AGL below
the altitude box and V/S below the airspeed box. The scale rotates past a
fixed triangular index, with 10-degree ticks and numbers every 30 degrees.
It follows aircraft attitude, including full rolls, independently of head-look.
The readout outlines are transparent. Combat debug status and range hints are no longer overlaid on
flight; normal HUD and instrument windows remain available.

## Target camera (view 4)

View 4 automatically magnifies the target to fill the image along the player's
sight line, including elevation. The camera sits no farther than one nautical
mile from the target, staying at the player position for nearer targets. It shows
a grayscale live target image, aircraft/object type, damage bar,
clock bearing with Hi/Lo, and range/speed alternating every three simulation
seconds. HI/LO appears only beyond 10 degrees above/below the horizontal
plane through your aircraft, independent of pitch and bank. A black bar is undamaged; white grows with damage. Existing pilot skill
and activity appear for mission aircraft. An underlined A means attacking you;
plain A means another target. Objective assignments are not yet available in
launch data, so the panel says `OBJECTIVE ?`. This does not mark every enemy as
an objective. See [behavior and remaining limits](spec/target-window.md).

## Cheats

**Escape → Cheat** toggles the session cheats below. Each row shows **On** or
**Off**; in the Damage submenu the selected choice reads On. Cheats apply on the
next simulation step, including when chosen while paused, survive Restart and
are not saved to disk. Behaviour: [cheats specification](spec/cheats.md).

| Entry | Effect |
| --- | --- |
| Damage → Invulnerable / Normal / Realistic | Invulnerable: weapon hits do no damage, no system faults and no pilot kill, and a midair collision does not kill you. Crashes into the ground still kill. Normal, the starting choice, takes hit points only: no system faults and no one-shot cockpit or core gun kills. Realistic takes hit points plus system faults (engine, fuel, hydraulics, controls, avionics, weapons) and the one-shot gun kills. AI aircraft always take Realistic damage. |
| Unlimited ammo? | The player's rounds and stores never run out. |
| Unlimited fuel? | The player's fuel never drops, including from damage leaks. |
| No spins? | No spin entry; a spin in progress damps out as a stall. |
| Pull extra G? | The player can pull 9 G whatever the aircraft's limit and load. Near stall the low-speed ceiling still ramps up to 9 G. |
| Ignore weapon weights? | Stores add no weight and no loading drag; fuel left in external tanks still counts. |
| No redout or blackout? | Turns off the G effects. Above 5 G, after a delay of 5 s just over 5 G that shrinks by a second per extra G (2 s at 8 G), the view greys out from the edges inward in proportion to G, fully black at 7.5 G and above. Below -2 G, after 3 s, it reds out, fully at -3 G. Thresholds follow published human G tolerance for a pilot in a G-suit. Vision clears in about 3 s and the controls keep working. |
| No screen-shaking? | Turns off the view shake that starts at 6 G and reaches about 4 pixels at 9 G in the cockpit views. |
| No crashes? | Ground, water and unsafe landings bounce the aircraft back into the air instead of crashing it; a building turns it around. Safe runway landings still land. |
| Easy aiming? | The player's rounds and missiles see targets 50% larger; the player's missiles turn 50% faster and their in-flight seeker cone is 25% wider. |
| Easy targeting? | The radar keeps the target selected while it is off the scope, including behind you in a merge, and outside the HUD its square floats over the target at the HUD's own brightness. Awareness only: no lock, radar lead or missile support is kept while it is off the scope. |
| Enemy AI? → Novice / Average / Unchanged | Every enemy aircraft flies at that skill from now on; Unchanged restores each one's mission skill. Friendly aircraft are unaffected. |
| Air combat guns only? | Every aircraft may fire only its gun. The player's weapon keys skip the other stations and a selected missile switches to the gun; AI aircraft stop choosing their other stores. Turning it off restores them. |
| Ignore midair collisions? | Aircraft pass through each other. With it off, two aircraft whose 28 ft contact spheres touch are both destroyed and nobody is credited with a kill; an Invulnerable player survives. |
| Easy flight physics? | Appended after the imported rows. The AV-8, Yak-141, V-22, AH-64, Mi-24 and CH-47 lose the hazards of their physics: rotor torque, the vortex ring state, retreating blade stall, an unrecoverable rotor stall (rotor speed holds at 85 percent or more in flight), the jets' low-speed roll-off and undamped puffers at stability level Off, and dynamic rollover. The helicopters (and the V-22 in helicopter mode) also get a weak attitude retention at Damper and Off, so a few taps of forward cyclic trim settle them in forward flight hands-off. Weight, power, the loaded jet's inability to hover and the V-22's corridor protection stay. Fixed-wing aircraft ignore it. In a session only the server sets it. |

Every missile or bomb burst on an aircraft, the player's or an AI's, now jolts
it: a fading roll, pitch and push away from the burst, even with Invulnerable
on. Details: [missile hit jolt](spec/cheats.md#missile-hit-jolt).
| No turbulence?, No sun whiteout? | Described below. |

Flight cheats apply to the default and legacy flight models; the native research
path ignores them.

## Sun glare cheat

**Escape → Cheat → No sun whiteout?** toggles glare suppression. **On** removes
sun whitening from world/cockpit/HUD palettes and disables lens-flare circles in
all rendered views; the original sun remains visible. It applies while paused,
lasts across mission restarts in the current session and is not saved to disk.
The developer override `TORE_SUN_GLARE=0` also disables these effects.

Main, mirror and instrument camera scenes now resolve their own altitude, fog,
palette and glare from the same weather clock. Mirror rendering stays GPU-only;
camera instruments use asynchronous feeds and can show an older completed image.
The target camera requests 24 frames per second; other camera panels retain
their roughly 10 Hz feed. Target-camera scenery is 10% darker for contrast,
with aircraft, static objects and text brightness preserved. [Evidence and remaining work](baselines/weather-cameras.md).

### Wind/turbulence continuation

**Cheat → No turbulence?** toggles physical turbulence for this session and
survives flight restart. HUD TAS, AGL and vertical speed now use the shared
AirData sample with explicit wind, terrain and standard atmosphere when in its
supported altitude range; labels remain TAS and geometric altitude. Missing
IAS/CAS or indicated/pressure altitude are not synthesized.


## Native research flight restriction

With `--native-flight-tables DIR`, the same pilot input feeds the joined native
airborne service. Existing device animation/threshold and fuel timing are host
adaptations. The **No turbulence?** setting stays on in this mode; enabling it
reports that environmental turbulence is unavailable. Reaching terrain contact
pauses with an explicit unsupported-contact message; restart resets the native
state. That limit belongs to this research option alone: ordinary free flight has
working authored ground contact. Legacy/hybrid retain their existing controls and
behavior.
[Scope and validation](baselines/native-live-flight.md).

## Additional aircraft

The additional FA aircraft use the same controls. F-14D and A-4E accept hook
commands; X-31 and the seven [roster additions](spec/roster-aircraft.md) do not.
A-4E and Su-25 ignore burner commands and rejects a nonzero
burner capture fraction. F-14 visual sweep is automatic and fitted. X-31 thrust
vectoring controls remain unavailable. See [aircraft behavior](spec/additional-aircraft.md).

F-14D, A-4E, X-31 and the seven roster additions use their own FA roll response values. Below 220 ft/s,
ordinary stick and rudder also command the source low-speed auxiliary rotation;
authority depends on throttle and is removed on the ground or without power.
There is no separate X-31 nozzle key. See the
[control contract](spec/additional-aircraft.md) for numbers and fitted components.

Researched flight is now the default; `--legacy-flight` preserves the previous
model. HUD and audio share the [stall warning signal](spec/stall-warnings.md),
including the original imported warning samples.

Hybrid [spin dynamics](spec/spin-transitions.md) use continuous axis values.
Opposite rudder decelerates rotation; wrong rudder can build it. Forward stick
moves the nose proportionally, with effectiveness reduced by fast rotation and
poor airflow. There is no fixed recovery ramp. Early intervention can stop a
spin while the aircraft remains stalled. Recovery uses the 25-degree airflow
cone and normal-rudder-rate threshold; sufficiently fast, aligned flight clears
the warning. Idle throttle is supported. TAS includes sideways/downward motion.

Spin entry torque begins gently at the clean-stall boundary, increasing with
speed deficit and back-stick. No fixed rudder percentage suddenly enables full
spin torque in hybrid flight. A-4 researched flight also uses a faster fitted
roll response; see [A-4 roll tuning](spec/additional-aircraft.md#a-4-roll-tuning).

## Gun pipper and target direction

Selected guns show ammunition and a ballistic pipper. With no usable radar
observation, `1000 FT` marks the manual's fixed reference distance. With radar
on and a current targeted aircraft observation, `RADAR` marks automatic range,
lead and drop correction. Place the pipper over the target. Its thick range arc
grows as the target closes; numeric range is in nautical miles. SAFE, empty and
failed guns hide the firing pipper.

The selected target has a square inside the HUD and a directional chevron at the
HUD edge when outside, including behind the aircraft. The cue drops with the
selection; the Easy targeting cheat keeps it and draws the square anywhere on
screen, without radar lead or weapon support. **L**, or
**RELEASE LOCK** in the weapon diagnostic panel when it is shown, clears both selections. Destroying/removing the target or
resetting the mission also removes its cue. The marker works with guns,
missiles and NAV selected. The [specification](spec/gunsight-targeting.md)
separates manual behavior from fitted projection and targeting rules.

For a controlled capture, `--hud-target-preview bearing,elevation,feet` first
selects a visible range aircraft, then repositions it relative to ownship. Use
with `--capture-flight`; the fixture cannot record or replay a mission.

## Missile seeker control

The upper range-scale value is now an estimated maximum for the observed
engagement, refreshed twice per simulation second. Launcher speed and attitude,
target direction/speed, altitude-dependent motor performance and turning losses
change it. Active-radar shots can be cued beyond the imported nominal launch
maximum and acquire with their own seeker later. Player missiles may also fire without acquisition or a predicted intercept. The lower scale value remains
the imported minimum range as an advisory. Onboard seeker angle and maximum-range limits remain. The percentage and IN RNG use the predicted
intercept. Target evasion and future loss of guidance are not predicted.
Two short horizontal bars inside the vertical scale mark the favorable firing
window. They move with the estimated engagement and disappear when no favorable
window exists. This is a fitted recommendation, not a guaranteed hit. The target
triangle remains visible outside range, clamped to the appropriate scale end.
IN RNG reflects predicted reach and release readiness, independent of percentage
rounding. [Band rules](spec/missiles.md#favorable-firing-range-bars).

The TAS/MSL boxes sit slightly lower, with the surrounding tape numbers removed.
Their horizontal positions remain on either side of the ladder. TARGET DESTROYED remains
in diagnostics and release logic but is omitted from the HUD.

Armed independent air-to-air missiles automatically enter BORESIGHT when radar power is on and no target is
selected. IR also supports bore with radar power off. A selected track takes
priority for IR and forces CUED acquisition against that identity, even when a
stronger bore return exists. Clear the track to return to BORESIGHT; airborne
missiles keep their own targets. Select a target to return to CUED. Press **L**, the existing
`clear-designation` action, or click **RELEASE LOCK** in the upper-right weapon
diagnostic panel when it is shown, to clear
selection. The manual's targeting list does not establish a retail release key.
`weapon-seeker-mode` remains rebindable but has no default key; the panel's mode
label is clickable only while the panel is shown (**Pref → Weapon diagnostics?**). With radar power off, radar-missile bore, tones and target cues
turn off; armed A2A IR missiles retain bore search and guidance; active missiles release maddog with their own seeker searching immediately. SARH
flies unguided until launcher illumination begins, including unsupported
surface SARH releases waiting for the A-G track. Selecting the passive IR channel alone does not turn
off the power switch. Supported radar weapons need aircraft lock for guided shots.
Minimum and maximum range are advisory. A missile can fire outside them and
attempts to guide. CUED IR without lock starts BORE search. Active maddog
search uses the full imported seeker cone and can lock friendlies.
The range scale and IN RNG still describe the recommended envelope.
AGM-65 and other surface profiles cannot engage the practice aircraft or use A2A
BORESIGHT. Surface designation is still deferred. See the
[minimum range and target-role rules](spec/missiles.md#minimum-engagement-and-target-role).

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
arrow with a crosshair across the entire black screen, up to the bezel.

Imported IR and radar search/lock samples provide the cues, with a louder lock
cue. Their assignment is fitted. Effects mute, pause, safe, empty and failed
stations silence them. `TORE_SEEKER_VOLUME=0..1` sets maximum amplitude, default
0.30. Re-import media to add the four samples to an older cache.
[Rules and constants](spec/missiles.md), [validation](baselines/hud-cleanup.md).

Each wing's skill selector also offers **Dummy (400 KTS)**. These training targets
hold their launch heading and altitude at 400 knots ground speed. They do not
fight, evade or follow wing commands, but remain damageable. Select a normal
skill to restore combat behavior. The player's aircraft remains under your
control. [Dummy mode specification](spec/dummy-aircraft.md).

Quick Mission wing counts launch independent AI aircraft by default, up to 29
plus the player. They use the selected aircraft and skill, with separate wing
formations. `--fixture-wings` retains straight-flight practice. Restart restores
the initial actors, stores, formations and damage state. Player wing shortcuts
are listed in [the input guide](INPUT.md#player-wing-orders).

Gun rounds have a 0.5-degree full spread cone (up to 0.25 degrees from aim),
with luminous warm cores and soft amber halos. Spread affects hits and repeats
deterministically on replay. See the [gun rules](spec/damage-smoke.md#gun-dispersion-and-luminous-tracers).

Aircraft accumulate damage in nose, cockpit, core, left wing, right wing and
tail regions. Light hits add persistent local marks. Concentrated damage grows
into local wing or fin tears, and a reviewed original damaged body appears only
when its missing region matches the hit. Global half-health still starts dark
smoke. Destroyed targets remain visible during their existing fall.
Powered missiles leave white smoke that disperses after burnout or impact.
Contrails, aircraft damage smoke and missile smoke share the clouds' live weather
palette, sunset lighting and haze, so they darken and tint at dusk and night.
Contrails last two simulation minutes: steady opacity for one minute, then
fade smoothly to invisible during the last minute. Pausing freezes their age;
stopping emission leaves existing puffs to finish their normal lifetime.
Reset restores intact aircraft and clears smoke. Regional thresholds, visual changes and smoke timing are fitted. Detached reviewed pieces inherit aircraft motion, fall, then disappear
with a brief ground-hit animation. AI aircraft take the shared Realistic system faults and use [fault responses](spec/systems-damage.md#ai-pilot-response-to-faults) to recover, protect engines and limit maneuvering. Dummies retain fixed-course behavior. The existing flight adapters stay selectable. See [damage and smoke behavior](spec/damage-smoke.md).

Damage appearance can be inspected with `--damage-preview 0..1` and
`--damage-preview-section nose|cockpit|core|left-wing|right-wing|tail`, together
with `--capture-flight PATH`. The fraction populates the selected region in
this presentation fixture; it does not simulate a shot. See the
[damage specification](spec/damage-smoke.md) for fitted thresholds.

## Starting on a runway

Quick Mission's **Start: Ground** choice exposes an airport selector for the
current theater. The player starts stationary, engine idling, gear and flaps down,
with brakes applied. Press **B** to release brakes, then increase throttle and
use the normal pitch controls for takeoff. Ground start uses the researched model;
the player's adapter is never switched automatically. The player's AI wingmen
start queued on the taxiway and wait for the player to become airborne before
entering the runway. The tower gives startup takeoff clearance, and departure
and landing reports use the [airfield radio](spec/airfield-radio.md). Other wings remain airborne. Alt-B orders bug out; Alt-L orders
landing at the airport selected with Shift-N. [Start behavior and fitted settings](spec/quick-mission-menu.md#player-ground-start).

While on a runway or using ILS, `XW` shows signed crosswind and the aircraft's
MTOW-class limit in knots. `NOTICE`, `ROUGH` and `LIMIT` identify increasing
difficulty. `TW` appears for tailwind, with a universal 10-knot limit. Positive
XW pushes toward runway right. These cues help choose a runway and manage rollout;
normal controls and tower clearance remain available. Parked tire grip remains.
[Wind thresholds and ground response](spec/runway-wind.md).

For takeoff, leave flaps extended, release **B**, apply full throttle (afterburner
only where available), and use gentle back pressure as the aircraft accelerates.
Ease the pull after liftoff and retract gear when clear. Flaps now add low-speed
lift instead of acting only as drag. Low-speed trim no longer pulls the nose
toward the former steep airborne target while accelerating on the runway. Ease
back pressure after liftoff to control the climb. There is no new takeoff-flap detent.
Headwind changes the ground speed needed for liftoff; the runway wind warnings
remain difficulty guidance. Land with controlled airspeed and settle the nose
after touchdown; overspeed full-flap approaches can float.
[Implementation and limits](spec/takeoff-ground-contact.md).

Developer takeoff captures can combine `--ground-start N`,
`--flight-probe-ticks TICKS`, `--maneuver takeoff` and `--capture-flight PATH`.
The level maneuver also supports an idle ground capture. Other ground pose
overrides remain rejected. [Acceptance and human test notes](baselines/takeoff-acceptance.md).

The original Map filter menu is omitted by request. Shift-M shows known runways
and current aircraft detections. Right-side buttons toggle Aircraft, Airfields,
Buildings, Surface and Emitters. Buildings start off; the other categories start
on. Unknown contacts use placeholders. Plus/minus zoom, arrows pan and Home follows the player. Flight
continues; map pointer input cannot operate the covered instruments.
`--flight-map --capture-flight PATH` captures this view. Map projection,
identification and surface detection are fitted rules in the [map spec](spec/flight-map.md).

NAV INFO and WEAPONS now use minus/plus selection and a third mode/page button.
See [selection and instrument rules](spec/weapon-navigation-selection.md).

Friendly selected contacts have a centered X inside their HUD target box.
[Identification and presentation rules](spec/gunsight-targeting.md#target-square-and-edge-chevron).

Partial wing or tail damage reduces control effectiveness and can cause an
uncommanded roll or yaw. Autopilot is unavailable after such structural damage.
The D report never rounds a surviving aircraft up to 100%; catastrophic whole-part
loss requires actual destruction. All damage marks and partial tears are also
currently hidden until 100%, while flight and system failures remain active.
See [damage behavior](spec/systems-damage.md).

Destroyed airborne aircraft now tumble and fall without accepting pilot commands.
Surviving engines may continue to push the wreck, including asymmetric thrust.
A 5% airburst check runs each second for five seconds, then every five seconds
until impact. A successful roll explodes and removes the aircraft. Restart clears
the wreck. See [destroyed-aircraft behavior](spec/destroyed-aircraft.md).

Pilot death immediately selects the F10 exterior view, recenters look/zoom and
closes the navigation map without pausing. Player nose loss is fatal to the pilot.
The player wreck explodes on ground impact; a safe landing does not.

A destroyed player aircraft continues trailing damage smoke while airborne,
even after pilot death. Impact or explosion stops emission; existing smoke fades.

## Ejection

Press **Shift+E twice** to eject. The first press asks for confirmation; holding
the key does not confirm. The eject action can be rebound in Controls. A living
pilot can escape an already destroyed airborne aircraft. The camera follows
the pilot and parachute while the abandoned aircraft falls independently.
Low or inverted escapes can be fatal. See [ejection behaviour](spec/ejection.md)
for timing, survival and the AI recovery assessment. Undamaged AI aircraft above
200 feet AGL never eject automatically, as requested by John on 2026-09-23.

In a network flight, K shows human and AI scores. While that board is open,
Ctrl+PageUp and Ctrl+PageDown turn its pages; ordinary flight controls apply
when it is closed. See [the controls list](CONTROLS.md) and
[multiplayer statistics](MULTIPLAYER.md#debrief).
