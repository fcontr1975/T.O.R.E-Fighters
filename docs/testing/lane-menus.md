# Battery lane: menus

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Covers the menu screens, the Quick Mission creator, the Load Ordnance page, the
preference, graphics, sound and controls screens, the briefing and debrief
screens, and the flight screen's own interface (HUD, instrument windows, views,
flight menu). Scenarios are in `tools/battery_scenarios/menus.py`. Run it with:

```sh
cargo build --locked -p tore-app
python3 tools/battery.py --lane menus --jobs 6 --windows 2 --tag menus
```

It takes about ten minutes on the dev machine with six jobs and two windows
(637 scenarios, 608 seconds at the last run): the headless ones (CPU snapshots,
instrument windows and headless mission starts, about 480) take a few minutes, the
windowed ones (about 170, each a real window through `tools/agent-run.sh`) about
eight, and the creator probe `menus-validate-creator` about eight on its own, in
parallel with the rest. The
scenarios count as the pass or fail; pictures the scenarios write are checked
for size and blankness, and were also looked at by the agent that wrote them.

## What each part checks

| Scenarios | What they do |
| --- | --- |
| `menus-validate-creator` | `--validate-creator`: loadouts for all 37 selectable identities including every removed-store case, then the creator matrix, the render sweep and the input fuzz (below) |
| `airport-allegiance` | `--airport-allegiance` (slice AL1): for every base theater, for Blue and for Redfor, the ground-start fields the creator offers and its default, each checked against the built world (the side's own or neutral, never the other side's; the creator's owner matches the imported allegiance; `start ground auto` and the AI's home runway from the map's middle and corners pick no enemy field). The summary must read `16 theaters, blue none: TVIET, red none: -, 0 problems` ([allegiance](../spec/airports.md#allegiance)); the Falklands' San Carlos, a STOL strip, is a Blue ground start since 2026-10-11 ([STOL runways](../spec/airports.md#stol-runways)) |
| `merged-airport-audit` | `--airfield-audit --merged`: all 84 base-theater pairs, source and merged positions, terrain picture IDs, flat and dry pavement, bounds, centerline clearance, four ground-start slots, 19 AI pavement routes and old-ID redirects. Requires `fields=84 problems=0`. |
| `airfield-taxi-audit` | `--airfield-audit` (slice CL-AP): every AI ground route of all 1,028 redrawn runway objects of the 75 layouts on the drawn pavement, every runway end reachable from parking, every runway line (the ILS) on a drawn runway. The summary must read `75 theaters, N airfields, 1028 runway objects, N routes, 0 problems` ([audit](../formats/redrawn-airports.md#audit)) |
| `airfield-parked-audit` | `--airfield-audit --parked`: every Quick Mission template aircraft parked on a redrawn airfield (the 108 templates, seed 1) stands on its pavement; `0 problems` |
| `menus-validate-text` | `--validate-text`: every imported string decodes without U+FFFD and is drawable in the original fonts |
| `menus-window-terrain-*` | Real-window captures on every base theater (clear, night), four theaters in each other weather, and ground starts (UKR and Cuba, the first Blue field of each) in all six weathers, checked for blank, black, one-colour, dark-day and bright-night frames |
| `menus-validate-maps`, `menus-validate-weather` | Every imported map layout and every weather module and choice |
| `menus-snap-*` | A CPU snapshot of every `--snapshot-state` value (main menu, Pref, controls tabs, graphics, sound, replays, locate, the Direct Connection screen), each checked for the right size and not being blank |
| `menus-snap-lobby-*` | The lobby screen's states, CPU text so the words are in the picture. The lobby pass (2026-10-09) added 18: the scroll bars and the ready hint (`lobby-scroll`, `lobby-scroll-top`, `lobby-king-hint`, `lobby-joiner-noslot-hint`, `lobby-joiner-hint`, `lobby-joiner-ready-hint`, `lobby-pvp-hint`, `lobby-coop-hint`) and PvP sides (`lobby-pvp-colours`, `lobby-pvp-colours-red`, `lobby-pvp-bluefor`, `lobby-pvp-redfor`, `lobby-pvp-open`, `lobby-pvp-full`, `lobby-pvp-locked`, `lobby-balanced`, `lobby-pvp-balanced-wait`, `lobby-pvp-unable`: every slot colour, each side box state, the filter, Autobalance) |
| `menus-snap-quick-lobby-creator-view*` | The read-only mission page (`--quick-mission --snapshot-state`, lobby pass L4 and K1): `lobby-creator-view`, `-gaps` (items the reader's game lacks), `-click` (a click's words), `-changed` ("The King changed the mission."), `-flying` (the King while the mission flies) and `-locked` (a server whose mission is fixed) |
| `menus-snap-quick-map*` | The creator's Map (slice CL-R; slice CL-R2 adds `map-building`, the note while the map is still built on its thread, and `map-wide` and `map-close`, the widest and narrowest zoom, all three checked for no void pixels in the plot): `map` and `map-lobby` on the default setup, and `menus-snap-quick-map-ukr-target-ground` and `-map-lobby-ukr-target-ground` on the Ukraine with a ground target, a Blue ground start at L'viv and Enemy Wing 1 parked at Donets'k. The log must name both starts by their airfields and give the 5 nm objective area (radius 30380 ft); the lobby's map shows the target's retail spot |
| `menus-flight-map-preview` | The in-flight map's mission markup, windowless and with no GPU (slice CL-MAP, `--flight-map-preview OUT_DIR`): it builds one Ukraine mission with a ground target and an enemy wing, reads the markup from the world as built (the function the creator's map and a networked game use), and draws the map as a flight does: following the player, zoomed to the whole theater, onto the objective area, scrolled by hand, with the Mission layer clicked off, as the Redfor pilot sees it, and with the player beyond the theater's edge (pinned, OFF MAP). The log must give both starts and the objective area, and no picture may show more than 50 void pixels in its plot |
| `menus-snap-quick-*` | The same for the creator, the aircraft and theater popups, the Load Ordnance page (normal, empty, drag, messages), the five debrief pages and both outcomes, for every aircraft and for every one of the 75 theater layouts |
| `menus-start-*` | The headless AI probe starts a mission through the creator's launch layout code: every aircraft on three theaters, every weather choice, every separation (1 to 300 nm, 75 added by the lobby pass: `menus-start-separation75-ukr` and `-cub`), every wing size. No aircraft may start off the map |
| `menus-loadout-*` | `--loadout none` and `--loadout guns` (stores taken off) for every aircraft, flown headless |
| `menus-window-launch-*` | The real launch path (`--launch-quick-mission --loadout none|guns`) for every aircraft: the launch line must show exactly the ammunition the page left, and the weapons window must list nothing (none) or only the gun (guns) |
| `menus-panel-*` | Instrument windows drawn on the CPU: every page (0 to 9) for every aircraft, and every panel-only systems fault (1 to 35) on the systems window |
| `menus-window-cockpit-*`, `-view-*`, `-flight-*`, `-flight-menu-*`, `-size-*` | Real-window captures: every aircraft's cockpit at 960 by 720, all 12 flight views, the flight and paused-menu screens at 960 by 720, 1280 by 720 and 640 by 900, and six more window sizes (out-of-range sizes must be refused with a message) |
| `menus-window-graphics-*`, `-preview-*`, `-weapons-page-*`, `-smoke-*` | Every graphics option value, the map, debug panels, weapon diagnostics, small layout, damage, ejection and chaff previews, the weapons window for stores-off loads, and smoke starts of the creator, the controls screen and free flight |
| `menus-combat-smoke-mig29` | The combat smoke probe, for the MiG-29 (the flight lane runs it for all 14) |

### The creator probe

`--validate-creator` (`TORE_CREATOR_STAGE=loadouts|matrix|render` runs one part):

- **Loadout drags** (`ordnance.rs`): catalog removal and refill, followed by
  transfers from the first external weapon row to compatible rows on the same
  page for every selectable aircraft. Each case starts with an empty destination.
  A transfer involving a paired point must move two stores, or leave the entire
  load unchanged if either end cannot supply or accept two. A successful transfer
  preserves the total quantity of the moved weapon, its identity and every other
  station's load, plus fuel and tanks. The destination's configured weapon and
  maximum may change. Catalog refill and old-save migration are separate actions
  and are not subject to transfer conservation. Cross-page cases are reported as
  skipped because their cards cannot receive the same drag.
- **Matrix** (`quick_mission/matrix.rs`): about 11,500 setups started through the same
  steps a flown mission takes (the creator's own refusal check, the wing launch,
  ground layout, layout plan, altitude check, combat build, flight restart, AI wing
  build, group orders). It sweeps every theater layout with every separation and
  altitude, every runway with wings of one to five (the 22 short strips were not on the list from 2026-09-30, 110 fewer setups; since 2026-10-11 they are listed, labelled STOL), every weather choice on every
  source theater, every player aircraft against every enemy aircraft, every wing's
  count, skill and aircraft, a sample of the whole wing-count product, every group
  order with survival on and off, every mission preset, and every other dropdown
  value. A setup must start with the player and every aircraft finite, on the map and
  above the ground, or be refused with a message. Result: 0 problems; 43 setups are
  refused with a clear message (listed in the probe output).
- **Render sweep**: draws the creator with every dropdown value chosen, every popup at
  every page, every group order and the help and notice overlays (518 pictures), none
  blank. `TORE_CREATOR_DUMP=DIR` also saves the worst-case-label pictures.
- **Input fuzz**: a fixed random stream of keys, pointer moves, clicks and right clicks
  (about 150,000 events) on the creator, the Load Ordnance page for every aircraft,
  the flight menu, the main menu and the graphics, sound, controls and replay screens.
  Nothing may panic, the creator's fields must stay inside their lists and the loadout
  must stay inside each station's capacity.

## Bugs found and fixed

| Symptom | Cause | Fix |
| --- | --- | --- |
| Stores taken off the Load Ordnance page (John's Mavericks) still appeared in the flight's WEAPONS window, and an aircraft with everything off started with an empty gun selected, armed and listed as `0 M61` | The weapons list, the selection ring and the flown-mission startup treated every configured station as on the aircraft, even with a count of zero. The ammunition itself was already correct (zero stayed zero) | The list leaves out empty stations, `[` and `]` step over them, startup falls back to the first loaded station or NAV, and a flown mission starts on NAV when the selected station is empty (`bb72502`, `042ec7b`, `242d106`) |
| `--validate-creator` stopped with `F18: damage region 3 at 0.1 has no distinct finite geometry` | The probe still expected visible partial damage on surviving aircraft, which the game deliberately draws intact | The probe now expects survivors intact and only a destroyed aircraft to change shape (`bb72502`) |
| The RCS window's "NO EXPOSURE DATA" ran over its 270 and 90 bearing labels | Same row as the labels | Moved below the axis (`2e79269`) |
| A broken `input-v1.conf` stopped start-up with "missing input profile version" and no file name | The error text carried no path | The message now starts with the file's path (`3570984`) |
| A malformed `graphics-v1.conf` was ignored without a word, unlike the preferences and sound files | The loader swallowed the error | It now logs why the defaults were used (`0a1d5af`) |

Also checked and fine: every settings file (preferences, graphics, sound) as empty,
random bytes, 400 KB, NUL bytes, a wrong version, duplicated lines, NaN and a directory
starts the game on the defaults, with the preferences and sound cases reported in the log
(no panics in 24 windowed starts); preference files with empty, duplicate or out-of-range
instrument pages load or are rejected cleanly.

New tests: `combat::tests::an_emptied_station_stays_empty_and_is_not_listed` and three more,
`live::tests::the_selection_ring_skips_stations_that_carry_nothing`,
`ordnance::tests::taking_every_store_off_by_any_route_leaves_zeros_that_still_validate`.

## Round two (2026-09-29)

**Garbled text.** Retail text is DOS code page 437, not UTF-8. The evidence is the
original fonts: `WIN11`, `HUD11` and the other FNT files draw exactly the CP437
text characters above 0x7F (accented letters at 0x80 to 0xA5, 0xA8, 0xAD, sharp s,
micro, degree) and no box-drawing cells. Across every imported text resource the only
byte above 0x7F is 0x89 in `KURILE.MM`, the airport `Ber\x89zovka`, an e with a
diaeresis (Berezovka with the diaeresis, "Berëzovka"). It was read lossily and printed
as U+FFFD; the map label dropped the letter; the bundled menu font had no cell for it.
Now mission layout text decodes as CP437 (`tore_formats::text`), every drawing path
(menus, creator, HUD, instruments, flight map, messages, replay and controls text, the
debrief) puts each character on its CP437 font cell, the two bundled menu font atlases
gain the CP437 letters (their ASCII columns are byte for byte unchanged), and theater
names may carry CP437 letters. Checked in the creator's airport popup ("Berëzovka") and
on the flight map ("BERËZOVKA"). `--validate-text` (scenario `menus-validate-text`)
scans 2,175 imported strings (creator lists, airports, aircraft, weapons) and every
text resource for U+FFFD and for characters the original fonts cannot draw; it found
one line and no problems. With `TORE_CREATOR_DUMP` the render sweep also draws a sample
with accented letters in every bitmap font: all the original menu fonts have real
letters at the CP437 cells, which confirms the character set. Commits `d0209f3`,
`b08cae4` and the theater-name commit between them.

**Terrain, sky and weather sweep.** 192 real-window captures at 960 by 720 (all 16
base theaters and eight variants, all six weather choices airborne, clear, dawn and
night ground starts, aircraft rotated through all 14) plus 60 in the lane
(`menus-window-terrain-*`) with an image check (`frame_stats`, `scene_problems` in
`menus.py`, tested in `tools/test_battery_menus.py`): not black, not one colour, day
bright, night dark, sky and ground both present. No blank, black or missing terrain,
no missing sky, no bright night or dark day. What the images show, all from the
imported haze tables and consistent with them:
- Cloudy and foggy weather at the default 5,000 ft start hide the ground completely (the
  tables put 256 of 256 haze at 6,000 ft cloudy and 500 ft foggy). Cloudy is visible at
  300 and 1,500 ft above ground, dim and washed out; fog is visible only on a ground
  start, and fades within a runway length.
- Athens (Greece) and Stanley (Falklands) airport pavement is pure white in daylight
  (grey at night); every other airport paving is grey or brown.
One finding for John to decide: **the HUD is hard to read over fog and cloud.** HUD
green has a luminance contrast of 1.04 to 1.08 to one against fog and cloud (1.3 over
clear sky), so it is told apart by hue alone. The lane tried a soft dark shadow one
pixel down and right of each HUD pixel (commit 78e62e1, fitted). That is a visual
design change, so the parent reverted it on the `bug-bash` branch; cherry-pick 78e62e1
to get it back.

**Feature claim audit.** 44 claims from the menu, creator, briefing, preference,
ordnance and HUD rows of `docs/features.md` and their specs were checked against the
app (capture, snapshot, headless probe or code and test): the separation list, wings up
to 29 plus the player, right-click cycling and the player wing never reaching zero,
ground-target popup text and geometry, the objective and survival selectors, Replays
after Multi, the Sound/Music Prefs sliders, defaults and file name, graphics defaults
and file name, five debrief pages and the rocker, loadout outlines, drag, sounds and
Cheat, NAV/LCOS/ARM status text, the WEAPONS window, the 162 by 160 instrument frames,
seven message lines and five seconds with a 5 pixel margin, the W waypoint key, the pack
limits (32,768 resources, 1 GiB), `--version`, fullscreen and window-size rules,
diagnostics self-test, ground start on NAV and airborne start with the gun armed,
1,000 ft pipper and damage regions for every aircraft (`--validate-creator`), AGL and
VS only on an active ILS, and the greyed Replay Last Mission and Continue Old Campaign.
Stale and fixed: the window-mode row said preferences were format version 6 (it is 7,
reading 1 to 6); the damage row said twelve aircraft (fourteen are selectable);
`--help` listed four of the creator's thirty-odd snapshot states; the graphics spec said
an unreadable settings file was silent.

**Corrupt controls file.** Decided by John on 2026-09-29: a damaged automatic
`input-v1.conf` falls back to the default controls with a warning that names the file
and the reason, like the other settings files; a file named with `--input-profile`
still fails loudly ([INPUT.md](../INPUT.md)). Round three built it; the replay lane
checks it (`replay-settings-corrupt-input*`).

## Manual audit (round three, 2026-09-29)

Each screen, dropdown, button, key and in-flight display in the retail manual
(`.local/missile-update/manual.txt`) was checked against the app. Results: **OK** works
and reads as the manual says; **FIXED** was wrong and is corrected (commit named);
**DIFFERS** is a documented, deliberate difference; **NOT IMPLEMENTED** is marked so in
`docs/features.md` or the game itself says "not implemented yet" (out of scope). Evidence
is a snapshot or capture named in the scenario list, a probe (`TORE_CREATOR_STAGE=menu`
prints every in-flight menu item with what it does), a unit test, or the code path.

| Item | Manual page | Result | Evidence |
| --- | --- | --- | --- |
| Choose Activity: nine buttons with the retail labels | 11 | OK | `menus-snap-normal` |
| Replay Last Mission, Continue Old Campaign greyed; Play Single Mission and the campaign entries say "coming soon" | 11 | NOT IMPLEMENTED | `menus-snap-notice`, features row |
| ? menu with Exit to Desktop | 12 | OK | `menus-snap-help` |
| Pref menu: Graphics, Sound (Screen Resolution absent, Controls and Re-import media added) | 12, 326 | DIFFERS | `menus-snap-pref`; the game has one window size setting via `--window-size` and Alt-Enter |
| Multi menu (retail: serial, modem, IPX, TCP) | 12 | DIFFERS | `menus-snap-multi`: Direct Connection and Internet Lobby for the new multiplayer, not the retail transports; the Direct Connection screen's five states are `menus-snap-direct*` and the Internet Lobby's are `menus-snap-internet*` (`internet`, `internet-games`, `internet-joining`, `internet-options`, `internet-unreachable`) |
| OK is Enter and Cancel is Escape on every screen | 12 | DIFFERS | Load Ordnance and debrief: Enter is OK, Escape backs out. Creator: Escape is Cancel, but Enter activates the focused field and OK is reached by Tab ([keyboard traversal spec](../spec/quick-mission-menu.md)) |
| Text buttons: left click cycles forward, right backward, Shift-click opens the list | 13 | OK | `quick_mission` right-click tests, creator fuzz |
| Aircraft menu: Fly All, Era | 14, 18, 326 | NOT IMPLEMENTED | the menu says "Aircraft era filters are not available yet" |
| Creator: wing size 0 to 5 (player wing never 0), up to three enemy wings | 19, 20 | OK | matrix sweep, `menus-snap-quick-field-4` |
| Creator: skill Ace, Experienced, Average, Novice (plus Dummy) | 19 | OK | `menus-snap-quick-field-5` |
| Creator: aircraft list is the supported imported aircraft | 19 | DIFFERS | 14 exact identities (John, 2026-09-16), not the 26 retail choices |
| Creator: location list of 16 theaters | 19 | OK | `menus-snap-quick-theater-*`; the imported layout variants are no longer listed (they load with `--theater ~CODE`, checked by `menus-snap-quick-devtheater-*` and the matrix probe) |
| Creator: altitude 5,000 to 40,000 ft | 19 | OK | field 14 popup: 5,000, 10,000, 20,000, 40,000 |
| Creator: weather Dawn, Clear, Cloudy, Overcast, Foggy, Sunset, Night | 19 | DIFFERS | six choices; Overcast is dropped as a duplicate of Cloudy ([quick-mission.md](../formats/quick-mission.md)) |
| Creator: situation Advantage, Neutral, Disadvantage | 19 | NOT IMPLEMENTED | the row is shown; the mission ignores it ([spec](../spec/quick-mission-menu.md)) |
| Creator: separation 1 to 50 nmi | 19 | OK | field 17 popup (100 to 300 added by John) |
| Creator: Standard or Custom weapons load opens the load screen | 20 | OK | field 18, `menus-window-launch-*` |
| Creator: Combat scope Guns only removes air-to-air missiles, air-to-ground allowed | 20 | DIFFERS | Guns only removes every non-gun store (John, 2026-09-22, [spec](../spec/quick-mission-menu.md#mission-wings)) |
| Creator: nationality is for designation only | 19, 20 | OK | no effect on the mission, lists match the manual |
| Creator: ground target, AAA and SAM defense | 20 | NOT IMPLEMENTED | popup "not implemented yet" |
| Quick Mission saved and replayable | 18 | NOT IMPLEMENTED | features row (no save) |
| Load Ordnance: left panel weapons with weight and guidance under each | 16, 17 | OK | `menus-snap-quick-ordnance` |
| Load Ordnance: drag to hardpoint, drag back to unload, click and right-click quantities | 16, 17 | OK | ordnance unit tests, `menus-snap-quick-ordnance-drag` |
| Load Ordnance: empty hardpoint reads NOTHING | 16 | DIFFERS | an empty station is a red outline with no text (John, 2026-09-22, [spec](../spec/ordnance-presentation.md#dragging-and-empty-stations)) |
| Load Ordnance: air-to-air and air-to-surface lights | 16 | OK | `menus-snap-quick-ordnance` |
| Load Ordnance: FLIR pods, laser pods, external tanks | 16 | NOT IMPLEMENTED | features row ("Tanks ... remain") |
| Load Ordnance: internal fuel switch and weight box | 17 | OK | ordnance tests, page shows max, current and available |
| Load Ordnance: gun rounds unload and reload | 17 | OK | `taking_every_store_off_by_any_route_leaves_zeros_that_still_validate` |
| Load Ordnance menu: Weapons > Unload All, Cheat | 329 | OK | `cheat_button_unloads_and_toggles_any_store_on_any_station` |
| Load Ordnance: Fly and Select Plane buttons | 16 | OK | Fly starts the mission, Select Plane returns to the creator |
| Debrief: clipboard pages, right and left click, arrow keys, OK | 17 | OK | `menus-snap-quick-debrief*`, `debrief::key` |
| Mission brief and map screens (single missions) | 14, 15 | NOT IMPLEMENTED | Play Single Mission is not available |
| In-flight menu bar: ? (End mission, Exit to Windows) | 332 | OK | probe: `End` and `Exit`; shown as Exit to Desktop |
| Control menu: Keyboard | 332 | OK | opens the controls screen |
| Control menu: joystick types, rudder pedals, throttle stick, HAT | 332 | NOT IMPLEMENTED | probe; the controls screen binds any device instead |
| Pref: Graphics, Sound, Time (Paused, Slow-motion, 1x to 8x), HUD pitch ladder, Dim and Brighten HUD, Show cockpit, Large windows | 332 | OK | probe: each opens its screen or toggles |
| Pref: Accelerated time, Rear-view mirrors, Authentic radar CRT, IR/Laser targeting, Radio silence | 332, 333 | NOT IMPLEMENTED | probe prints "not implemented yet" (the mirrors are always on) |
| Pref: Show target info (Ctrl+T) | 12, 333 | OK | probe: the row toggles and says "Show target info: on"; `show_target_info_is_off_by_default_and_toggles_from_ctrl_t_and_the_pref_row`, `target_info` render test, `replay-script-friend-or-foe` (F2-C, 2026-10-05) |
| View menu: 11 views with F1 to F12 shortcuts | 333, 103 | OK | probe, `menus-window-view-*` |
| View menu: Ctrl and Alt use missile or target, View transitions | 333 | NOT IMPLEMENTED | probe (the Ctrl and Alt keys themselves work, see CONTROLS) |
| Window menu: envelope (Current), the nine windows, RCS | 333 | OK | probe, `menus-panel-*` |
| Window menu: envelope All and Compare | 333, 92 | NOT IMPLEMENTED | probe (the U, A and C buttons in the window draw) |
| Cheat menu: damage (Invulnerable, Normal, Realistic), unlimited ammo and fuel, easy aiming, no crashes, no spins, no turbulence, extra G, ignore weights, no whiteout, no redout or blackout, no shake, enemy AI, ignore midair, easy targeting, guns only | 333, 334 | OK | probe: all 14 toggle and each flag is read by the simulation (checked by code search) |
| Multi menu items and Position menu | 334 | NOT IMPLEMENTED | probe |
| HUD: heading tape, G meter, thrust percent and AFT, flight path marker, pitch ladder (solid up, dashed down), weapon and rounds | 78 to 80 | OK | flight captures, `menus-window-cockpit-*` |
| HUD: airspeed and altitude tapes with AGL bar and corner speed bar | 78, 79 | DIFFERS | boxed TAS and MSL values instead of tapes (features row, [HUD layout](../spec/hud-layout.md)) |
| HUD: GEAR, FLAP, BRAKE, HOOK in the upper right | 79 | OK | `--flight-devices 1,1,1,1,1` capture |
| HUD: BAY when the weapons bay is open (F-22) | 79 | FIXED | was never drawn; now the fifth label (`hud::draw`) |
| HUD: time compression rate in the upper right | 80 | FIXED | was a one-off message only; now `2X`, `4X`, `8X`, `1/2X` (`hud::time_label` test) |
| HUD: Weapons and Navigation modes (LCOS, NAV, ILS), N toggles | 77 | OK | launch line, ground start shows NAV |
| HUD: thrust vectoring VCTR, stability indicator, HSI | 81, 82 | NOT IMPLEMENTED | FLIGHT-CONTROLS ("thrust vectoring remains unavailable") |
| HUD: TD box, target range, closure, aspect angle, hit probability, weapon range scale, seeker diamond | 83, 84 | OK | `--hud-target-preview` capture, flight lane weapon scenarios |
| HUD: gun pipper at 1,000 ft with radar off, range arc | 86 | OK | `--validate-creator` (1,000 ft sight solution) |
| HUD: ILS glide slope, localizer, AGL and vertical speed | 87 | OK | `hud::draw` tests, AGL only on an active ILS |
| Windows: Front view (s2), Other view (s3, V sets it) | 88, 89 | OK | `menus-panel-*-page-2/3`, CONTROLS |
| Windows: Weapons status with + and -, System status (THR, TEMP, OIL, HYD), Nav (bearing, distance, ETA, + and -) | 89, 93 | OK | `menus-panel-*-page-6/7/8` |
| Windows: Envelope U, A, C buttons | 92 | OK | `menus-panel-*-page-1` |
| Windows: RWR with range (max 50), R and I indicators, JAM, comma and period range | 94 | FIXED | the range keys were reversed (see below); rest OK |
| Windows: RCS | 95 | OK | `menus-panel-*-page-0` |
| Radar window: RWS beyond tracking range, TWS within it, range number, squares with flags, Y history, M mode, +/- range | 96 to 100 | OK | captures at 150 and 5 nmi (RWS, TWS) |
| Radar range keys: comma increases, period decreases | 21, 97 | FIXED | were reversed in the game, the keyboard spec and the keyboard map |
| Ground radar (Ctrl-R), HARM (M), authentic radar CRT | 99, 100 | NOT IMPLEMENTED | features and probe |
| Target window: skill dots, tactical goal letter, activity, bearing, damage bar, range | 101 | OK | `target_window` tests; the bearing clock shows in the `--hud-target-preview` capture |
| In-flight map (Shift-M) with show toggles | 102, 201 | OK | `menus-window-preview-map`: aircraft, airfields, buildings, surface, emitters |
| View keys F1 to F12, pan with Shift and arrows, +/- zoom, Alt and Ctrl references | 103, 104 | OK | CONTROLS, `menus-window-view-*` |
| Cockpit toggle (Backspace) | 77 | OK | probe ("BS", `Show cockpit?`) |
| Wingman orders Alt-1 to Alt-9, E, R, P, D, B, T, C, H, V | 159 | OK | `docs/INPUT.md` table, `flight_ui` order key test |
| Wingman Alt-W (engage every target of the target's class) and Alt-F (attack on contact, IR targeting) | 159 | DIFFERS | Alt-W is attack on contact here and Alt-F reports "unavailable" ([INPUT.md](../INPUT.md), documented) |
| ? menu labelled Exit to Windows | 12 | DIFFERS | every menu shows Exit to Desktop (John, 2026-09-29): the main menu, creator, debrief, the flight and replay Esc menus and the shortcut help; only the imported row label behind them stays retail (`pause_menu::display_label`) |
| In-flight map: Shift-M toggles, +/- zoom, scroll | 102, 202 | OK | the manual's "A S W Z" and "W Z A S" are its typeface's arrow-key symbols (the same symbols name pitch, roll and Shift-panning, which the catalog binds to the arrows), so the arrows scroll |
| In-flight map: Show menu classes (planes, SAM, AAA, ships, airports, vehicles, other), SAM ranges, 5 nmi grid | 201, 328 | DIFFERS | category toggles are Aircraft, Airfields, Buildings, Surface, Emitters and Mission; no SAM ranges or grid yet ([map spec](../spec/flight-map.md)) |
| In-flight map pauses the flight | 334 | DIFFERS | the flight keeps running under the map (`map_shortcut_pan_and_escape_do_not_pause_or_switch_sensors`) |
| Keys: 1 to 8 throttle, A autopilot, B brakes, F flaps, G gear, H hook, O bay, [ ] weapons, R radar, I IR, T targets, Enter visual target, W and Shift-W waypoints, Insert and Delete countermeasures, Shift-1 to 0 windows, Ctrl-P pause, Shift-E twice | 60 to 104 | OK | `docs/CONTROLS.md` row by row, `flight_ui` key test |

**CONTROLS.md against the catalog and the manual.** The document is generated from
`input_catalog.rs` and the `controls_doc_matches_the_catalog` test fails when they differ,
so there is no mismatch between them. Every catalog action is dispatched: the 100 command
entries either have a match arm in the app's named-action table in `main.rs` or are switch actions handled in
`tore-input` (airbrake, bay, engine, flaps, gear, hook, jammer, waypoint autopilot), and
the one arm that returns nothing on purpose is the retired `master-arm`. Against the manual's key boxes
one key was wrong: comma and period, reversed. Documented differences that stay: Z and X
are extra rudder keys here (the manual uses them for vectored thrust nozzles); Shift and
arrows pan the view (manual p. 104: its "ASWZ" is the arrow-key symbols);
Ctrl and arrows (thrust vectoring), Ctrl-R, M for HARM and the wing sweep keys have no
binding, as FLIGHT-CONTROLS.md says.

**Round three summary.** Fixed from the audit: the scope range keys were reversed (comma
raises the range and period lowers it, manual pp. 21, 94, 97), the HUD had no BAY entry for
aircraft with a weapons bay, and the HUD did not show the time compression rate the manual
prints beside the clock (`1/2X`, `2X`, `4X`, `8X`). One attempted fix was wrong and was
reverted: the manual's "A S W Z" for scrolling the map are its typeface's arrow-key
symbols, so the arrows already do it, and A stays the autopilot. Also added: `TORE_CREATOR_STAGE=menu`
prints every retail in-flight menu leaf with its result (96 items, 35 not implemented, all
of which say "not implemented yet" when chosen). The merged menus lane and the windowed
flight subset were rerun on the merged tree; results are in the report to the parent
(no regressions from this lane's changes).

**Review fixes after round three.** The BAY label moved above GEAR (the time rate and BAY
now sit at y 118 and 129) so it no longer runs into the MSL caption; a test checks every
status label pair for overlap with a solid 10 px font, and an F-22 capture with the bay
open shows BAY and MSL apart. With the guns only cheat on and an empty gun, startup now
begins on NAV instead of arming a missile (`station_allowed` is public and used by the
fallback search). The weapons window truncates names by character, so accented names
keep their letters.

## Found and not fixed

- **`--combat-smoke`** failed for 13 of the 14 aircraft when this lane first ran. The flight
  lane found the probe stale, not the game, and fixed it; all 14 pass now.

## Round four (2026-09-29, John's decisions)

- **Theater list.** The creator offers the sixteen base theaters only. The 59 imported
  `~` layout variants (mostly one or two airports, incomplete) load only through
  `--theater ~CODE`, which adds that layout for the run; the matrix probe checks the
  list holds exactly the sixteen and still sweeps every layout, and `menus-snap-quick-devtheater-*`
  opens the creator on each variant. This retires the "ground start on nine layouts with
  no airports" note for players; those layouts are all variants.
- **Stations that ran dry.** A station emptied in flight cannot be selected (ring, buttons
  and `--weapon-slot` refuse it), hands the selection to the next loaded station or NAV
  (not while the trigger is held), and keeps a dim `0` row in the WEAPONS window. A station
  taken off on the Load Ordnance page has no row at all, as fixed in round one.
- **Exit to Desktop** everywhere it is shown, including the flight menu's shortcut help.
- **Damaged `input-v1.conf`** falls back to the default controls with a logged warning;
  `--input-profile` still fails loudly.
- **Recorded fuel** includes the external tanks (`fuel=internal and external tanks` in the
  header; older recordings unchanged).
- Closed by John, left as they are: the creator's Enter key and the rear-view mirrors
  menu row.

## Needs a decision

- **Su-35 wingtip station.** The imported Su-35 marks its wingtip AA-11B station as
  internal, so the Load Ordnance page prints it as `2 (max 2)` (no "loaded"), its weight
  is left out of the payload, and the Guns only rule treats it as a weapon to strip. The
  data says so; nobody has checked whether the retail game does the same.
- **Ground start on a variant layout with no airports** (developer option only). Nine
  `~` layouts have no imported runway; the creator refuses at Fly with "No imported
  runways are available in this theater. Choose Airborne."
- **Runways where a lone aircraft is obstructed.** Greece runway 13 refuses even a single
  player ("The runway start is obstructed"); about a dozen more (for example Greece 12, 15
  and 16) refuse only a wing of two or more. This is the documented rule (obstructed
  starts are refused), listed here in case the scenery is wrong rather than the check.
- **Creator rows that change nothing yet.** The situation row (neutral, friendly,
  hostile) and both nationality rows are presentation only; the mission does not use
  them ([creator spec](../spec/quick-mission-menu.md)).

## Needs a human eye or ear

- Night ground starts in desert theaters (Egypt, Iraq, Persian Gulf and others) show a
  brown ground clearly lighter than the sky; the image check passes it, but whether
  retail night ground was that lit is unknown.
- The HUD's new dark edge (round two) looks subtle in captures; a human should judge it
  against fog, cloud and a bright sky.
- Athens and Stanley airport pavement is pure white in daylight; fog and cloud hide the
  ground at the default 5,000 ft start (see round two).
- Portrait windows (640 by 900): the flight view fills the window while the paused menu
  and its bottom buttons sit in a centred 4 by 3 box in the middle. It looks
  deliberate but odd.
- The debug panels overlap the instrument windows and each other when all are open.
  They are a development feature.
- Whether the Load Ordnance page should print an empty station as NOTHING (the manual's
  wording) instead of an outlined empty box (the current, documented choice).
- Every screen's look against the retail game: the scenarios only find broken layouts,
  wrong sizes and blank pictures.
