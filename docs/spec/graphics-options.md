# Graphics options

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. John asked on 2026-09-22 for easier-to-see distant
aircraft and for rendering improvements that help with it: anti-aliasing, a
spotting outline, terrain distance filtering and render scale. Everything on
this page is **opinionated**. The features were requested by John; the methods,
constants and defaults are agent decisions, fitted by eye on captures. The
original game had none of these. All options off at 100% is the closest to its
look.

## The Graphics screen

Main menu Pref > Graphics... opens a screen in the same style as the controls
screen. Every change takes effect on its own and is saved: there is no Apply
button. **Defaults** puts the recommended settings back, applied and saved the
same way, and **Close** or Escape leaves the screen, which has nothing left to
undo. Up/Down move between rows, Left/Right change a value, Enter or Space
cycles it, Tab walks every control, and the mouse picks a choice directly. A
one-line description of the focused row is shown below the options. The top
right of the settings panel reads IN USE AND SAVED, APPLYING while a change is
settling, or IN USE, NOT SAVED when the file could not be written.

A change is applied 150 ms after the last one (*agent decision*, 2026-10-19).
Holding an arrow key or clicking quickly through choices therefore applies only
the value the player stops on, so anti-aliasing (which rebuilds the 3D
pipelines) and the vegetation and building settings (which resize or refill
their buffers) are rebuilt once, not once per step. A change still settling is
applied and saved when the screen closes or the game quits.

The rows sit under four headings in a list that scrolls: **Display**
(anti-aliasing, render scale, spotting aid, frame rate), **Terrain and scenery** (terrain
filtering, ground detail and its distance, building distance and detail,
shadows and shadow distance),
**Vegetation and grass** (vegetation distance, detail, density and cards,
grass, grass density and distance) and **Performance** (the frame-rate
counter). The scroll bar is the map's arrow bar, the one the multiplayer
lobby's settings pages use: the wheel scrolls a line a notch, the arrows a
line, the track a page, and the thumb drags. Moving the focus with the keys
scrolls the focused row into view with its heading. Under the render scale
row a line shows the window's size in physical pixels (for example
`Window 3840x2160`) and, under each percentage, the size the 3D view is drawn
at with it; the About panel spells out the chosen one, for example
`75%: the 3D view is drawn at 2880x1620 and upscaled to the 3840x2160 window.`
The sizes follow the window when it is resized. Requested by John on
2026-10-19 (auto-apply, Close, the scroll bar, the groups and the
resolutions); the group names and wording are agent decisions.

| Option | Choices | Default |
| --- | --- | --- |
| Anti-aliasing | Off, 2x, 4x, 8x | 4x |
| Render scale | 75%, 100%, 125%, 150%, 200% | 100% |
| Spotting aid | Off, Subtle, Strong | Subtle |
| Frame rate | 60, 100, 200, V-sync | V-sync |
| Terrain filtering | Off, On | On |
| Building distance (city fill) | 10 nm, 20 nm, 30 nm | 30 nm |
| Building detail (city fill) | 1 nm, 2 nm, 5 nm, 10 nm, 20 nm | 5 nm |
| Shadows | Off, Low, Med, High, Ultra | Med |
| Shadow distance | Trees, 1 nm, 2 nm, 4 nm, 6 nm, 8 nm | Trees |
| Ground detail | Off, On, Fine | On |
| Ground detail distance | 1 nm, 2 nm, 4 nm | 2 nm |
| Vegetation distance | 2 nm, 4 nm, 6 nm, 8 nm | 4 nm |
| Vegetation detail | 1 nm, 2 nm, 3 nm, 4 nm | 2 nm |
| Vegetation density | Low, Med, High, Ultra | High |
| Vegetation cards | Facing, Cross, Cross+top, Face+top | Face+top |
| Frame rate counter | Off, On | Off |

Choices are saved in `graphics-v1.conf` in the application data directory,
beside `preferences-v1.conf`. A missing file uses the defaults quietly; an unreadable or malformed one uses them and logs a warning saying why.
Smoke tests and captures ignore the saved file, like the other display
preferences. The frame-rate line is written only when the counter is on
(`frame-rate true`), so a default file is unchanged. The frame rate is
written only when it is not V-sync (`frame-limit 60`, `100` or `200`); a file
from before the choice existed reads as V-sync. The shadow quality is written
only when it is not Med (`shadows off`, `low`, `high` or `ultra`); a file
from before the choice existed reads as Med. Shadow distance is written only
for a fixed choice (`shadow-distance 1`, `2`, `4`, `6` or `8`); an omitted
line or `shadow-distance trees` follows the vegetation distance.

## Frame rate

How often the window is drawn. The choices and the V-sync default were set by
John on 2026-10-19; the rest of this section is agent decisions, made the same
day. The simulation does not depend on it: every flight frame still turns the
wall time since the last frame into 120 Hz ticks (at most 0.25 s, every owed
tick run), and a multiplayer flight's ticks belong to the session.

| Choice | Presentation | Pacing |
| --- | --- | --- |
| V-sync (default) | `Fifo`: each frame waits for the display | One frame per display refresh |
| 60, 100, 200 | `Mailbox` (no tearing), else `Immediate`, else `Fifo` | A timer spaces frames evenly at that rate; the event loop sleeps until each frame is due, it does not spin |

V-sync uses `Fifo` rather than `Mailbox` plus a timer at the monitor's
refresh. A timer is not locked to the display: its drift against the refresh
shows as a doubled or dropped frame every so often, which is the uneven
pacing V-sync is meant to avoid. `Fifo` costs up to one refresh of input delay
over a non-waiting mode. With `Fifo` the window also asks Wayland for frame
callbacks, so a hidden window on Wayland gets no redraws rather than blocking
in its next present. Above the display's refresh, 100 and 200 lower the input
delay at the cost of drawing frames that are never shown. A late frame starts
the timer's count again rather than rushing to catch up. A change applies at
once, in flight too; the surface is configured again when the presentation
mode changes.

Throttles that need no setting:

- A single-player flight that is stopped (paused, its Escape menu or a screen
  opened from it) draws only when something happens once its picture has
  settled: a key, the mouse, a controller button, a resize, a notice expiring,
  or an animation on the screen over it. Mouse look-around while paused, the
  map and the [frame-rate counter](#frame-rate-counter) keep it drawing. The first running frame
  after a stop turns at most 1/30 s of wall time into ticks; every resume
  already restarts the flight clock, so this only guards a missed one.
- An unfocused window draws about ten frames a second. Events do not add
  frames to those: the pointer passing over a background window waits for
  the frame already due. Those slower frames are not counted as the game
  running late, so they never wake the extra simulation workers. Controllers
  are read at the menus' slower rate while the window is unfocused or hidden,
  since nothing acts on them there.
- A frame the graphics driver gives no picture for (the surface lost,
  outdated or timed out, as can happen while a window minimises or changes
  mode) is tried again after 16 ms, doubling up to a quarter of a second
  while it keeps failing, never in a tight loop.
- A hidden window draws nothing until it shows again: `Occluded` where the
  platform reports it (X11, macOS), or minimised to no size (Windows).
  Wayland reports neither; there V-sync's frame callbacks stop the drawing,
  and focus loss pauses a single-player flight.
- A minimised or hidden single-player flight pauses, as on focus loss.
  Multiplayer never pauses: the session takes its network turns between
  frames whenever its next tick is due, so its ticks, controls and keepalives
  continue while the window is unfocused, hidden or drawing slowly.

Scripted runs (`--input-script`), performance measurements (`TORE_PERF_FRAMES`),
captures and smoke tests are never throttled, and draw uncapped (`Immediate`)
unless `TORE_FRAME_LIMIT` is set: they keep the presentation they were written
against, and agent windows on a hidden workspace get no frame callbacks.

`TORE_FRAME_LIMIT` replaces the saved choice for one run without saving it:
`vsync`, `60`, `100`, `200`, or `off` for uncapped rendering (`Immediate`,
else `Mailbox`, else `Fifo`, no timer and no unfocused cap) for benchmarks.
Uncapped is not offered on the screen. Any other value is ignored with a
warning. While it is set, the About panel says so under the Frame rate row.

## Graphics API

**Opinionated.** John asked on 2026-11-02, after issue #8, for the player to pick
Vulkan or DirectX 12 on the import screen and in the Graphics settings. The default
rule, the DirectX 12 compiler flag and the wording are agent decisions.

Windows only. The player sees two values, **Vulkan** and **DirectX 12**, and the
game never changes the choice by itself. Linux and macOS show nothing and leave the
choice to wgpu, as they always have.

The value is saved as `graphics-api` in `graphics-v1.conf`, only once the player has
chosen, so a default file is unchanged. Until then the game uses Vulkan when a
hardware Vulkan adapter exists, otherwise DirectX 12. A software Vulkan driver does not
count, because it would win over a hardware DirectX 12 adapter and draw on the CPU. A value this build does not know counts
as not chosen, without costing the rest of the file.

`--graphics-api vulkan|dx12` chooses for one run and is never saved, not even by an
edit on the Graphics screen, unlike the other graphics flags. `--graphics-api auto`
asks for the default rule for one run, ignoring the saved value. Outside Windows
only `auto` is accepted, and the flag explains that for the other two.

The player can choose in two places:

- **The import screen** (the first-run [Locate Fighters Anthology](first-run-import.md)
  screen, also reached by Pref > Re-import media) shows the choice on its title line,
  usable before, during and after an import. It is saved at once and applies **with
  no restart**: every route from that screen into the game builds a new renderer and
  a new wgpu instance after it, in the same process, and each instance reads the
  choice in effect. The screen says so when the choice changes. The screen itself
  keeps drawing with the API it opened on.
- **Pref > Graphics**, the Graphics API row under Display. It is saved like the other
  rows but **does not apply live**: the running renderer keeps its API, and the
  message says to restart the game. While the row differs from the API in use, the
  note under it names both. A later Pref > Re-import media in the same process counts
  as a restart, since it builds a new renderer. Defaults leaves the row alone, because
  it may be what keeps the game running.

### When a launch did not finish graphics setup

A player whose game dies during GPU setup never reaches either screen: once media is
imported, every launch goes straight to the game window. So the game notices, and
asks. Nothing switches without the player.

A **marker** is one file per process in `<data>/gpu-setup/`, holding an OS file lock
for the life of the process, naming the API in use. It exists around launch-time
setup only: while looking for a Vulkan adapter for the default rule, while the
first-run canvas draws its first frame, and while the game renderer is built up to its
first presented frame. A long import is never covered, and nor are mission, replay and
theater loads, because a player who dies there has already reached Pref > Graphics.

A launch that can take a marker's lock knows its owner is gone, and removes it. Then,
before anything else, it shows a small screen on the same canvas path as the import
screen, with the choice, Continue and Quit, and one sentence saying the last launch
did not finish graphics setup. Continue saves the choice and the game goes on, with
no restart. The screen always draws on the API that did not die, never looks for a
Vulkan adapter first, and has a marker of its own: if it dies too, the next launch's
screen draws on the other API, so it can never get stuck retrying one. The saved value
only sets which option is highlighted. With nothing saved, the API that did not die is
highlighted, so that Continue tries something different.

What leaves a marker behind:

- **A native crash, a kill, or a Rust panic.** A kill during a slow compile counts,
  and costs one question.
- **A returned error during setup.** It has its own report and dialog too, but a
  player whose driver cannot start needs the same way out.

What does not:

- **A clean close**, including a quit during setup. The marker is removed.
- **A sibling game**, running several being allowed: its marker is locked and skipped.
- **Another profile**: markers live in the data directory, so an isolated
  `TORE_DATA_DIR` sees only its own.
- **A run with no player**: headless, capture, smoke test, validation, perf,
  scripted-input and diagnostic modes, or `TORE_NO_ERROR_DIALOG=1`, write no marker,
  read none and never ask. A launch with `--graphics-api vulkan` or `dx12` writes one, since
  it can die too, but it does not ask, because the flag has already answered.
- **A file system with no file locks**: without them a live sibling would look dead, so
  that launch writes no marker and logs why. A marker that cannot be opened or locked
  proves nothing about its owner either, so it is skipped and logged. One that cannot
  be removed is logged, and the next launch may ask again.

The first-run screen, the game, and the offline preview tools all use the same
choice, through `graphics_api::instance`. A Clippy rule rejects any other
`wgpu::Instance::new` outside tests. The offline tools that return before the launch
reads its options (`--reel-render`, for example) use the default rule and ignore the
saved value.

### DirectX 12 shader compilation

wgpu 27 compiles shaders on DirectX 12 with **FXC** by default: `d3dcompiler_47.dll`,
part of Windows, nothing to ship. It translates WGSL through naga to HLSL shader
model 5.1 when each pipeline is created. There is no disk cache, so this repeats
every launch, and it runs on the main thread, so the window stops responding. The
game retitles the window while it happens.

Every DirectX 12 instance passes `InstanceFlags::DEBUG`, which is wgpu's one way to
ask FXC to skip optimization. The GPU driver still optimizes what it receives. Other
backends keep the default flags.

Measured on 2026-11-02: every game shader, translated by naga 27.0.3 with wgpu's
options and compiled by the real `d3dcompiler_47.dll` 10.0.17134.12 under Wine on a
busy Linux machine, on a 1 MiB main-thread stack. These are compile times only.

| | Unique entry points | Failed | One launch | Slowest |
| --- | --- | --- | --- | --- |
| FXC default | 85 | 0 | about 530 s | `terrain_fragment` 342 s, `sky_fragment` 147 s |
| Skip optimization | 86 | 0 | about 42 s | `terrain_fragment` 23 s, `sky_fragment` 9 s |

On a Windows VM with no GPU (Basic Render Driver), `--graphics-api dx12 --smoke-test`
passed in 55 s, with the Shoreline terrain pipeline taking 24 to 38 s. With full FXC
that pipeline had not finished after 230 s, when the run was stopped. Turning off
naga's runtime checks made no real difference. How the picture and frame rate hold
up on real GPUs has not been measured.

DXC would be faster but is not used. It needs `dxcompiler.dll` shipped beside the
game (tens of MiB) or wgpu's `static-dxc` feature, which pulls a prebuilt static
library into the build.

## Anti-aliasing

Multisample anti-aliasing of the 3D view: aircraft, terrain, clouds, horizon,
vapor, tracers and smoke. The cockpit art and HUD are unchanged. 4x works on
every graphics card; 2x and 8x are offered only when the card supports them for
the display format, and unsupported levels are shown struck through. A saved
level the card cannot draw falls back to the next lower level. Changing the
level applies immediately, including in flight and replay. The sun's lens flare
and spotting outline remain usable when switching between Off and multisampling;
each camera keeps its existing sun-visibility fade through the change.

Cutout edges (water shorelines, airport markings) are decided once per pixel,
so anti-aliasing does not smooth them; render scale does.

## Render scale

The 3D view is drawn at the chosen percentage of the window's pixel size and
resampled to the window. Above 100% each window pixel averages up to 4 by 4
bilinear taps across its footprint (supersampling); below 100% the image is
bilinearly enlarged, trading sharpness for speed. The cockpit and HUD are always
drawn at window resolution.

## Spotting aid

Other aircraft get a thin, pixel-sharp contrasting outline so a small distant
contact can be found without turning it into a marker.

- **Who:** combat targets, dummy aircraft and AI wingmen and enemies. Not the
  player's own aircraft, missiles, tracers, flares, explosions or debris.
- **Where:** the main view and the rear mirrors. Not the instrument camera
  panels.
- **Shape:** drawn after the 3D view at window resolution, without
  anti-aliasing, so edges are always hard.
  - An aircraft 2 pixels or larger gets a 1-pixel border: its silhouette copied
    one window pixel in each of 8 directions, behind the aircraft itself.
  - Under 3 pixels, where there is no silhouette left to outline, it gets a
    solid square mark instead: 2 by 2 pixels under 1.5 pixels, 3 by 3 pixels up
    to 3 pixels.
- **Size:** each aircraft's largest dimension (56 feet for the F/A-18D)
  projected at its distance.
- **Strength:** the product of
  - full until the aircraft fills 1% of view height, fading to zero at 10%;
  - an eyesight limit, full above 4 arcminutes and zero below 1.5 arcminutes
    (about 8 and 21 nautical miles for a 56-foot aircraft);
  - the square root of how much of the aircraft's contrast survives haze and
    cloud, so a fully hazed or clouded aircraft gets nothing;
  - the setting: Subtle 0.5, Strong 0.85.
- **Colour:** strength is a step in brightness, never transparency.
  - Against sky the outline is dark: at full strength 0.2 times the estimated
    background brightness.
  - Against ground darker than 0.3 luminance it is light: 2.5 times the
    background brightness.
  - It keeps 30% of the background's hue.
  - The background is estimated from the horizon colour, or a hazed dark-ground
    value below the horizon; a dark outline can only darken a pixel and a light
    one can only brighten it.
- **Occlusion:** it reads the 3D view's depth, so terrain, cloud and nearer
  aircraft hide it, and it never covers the aircraft itself.

Known differences:

- The background is estimated, not read. Subtle can be faint against the darker
  sky-deck art.
- Hairline wings and tails at mid range do not get their own outline.
- Around 2 to 3 pixels the change from mark to outline can pop.
- Smoke, vapor and tracers write no depth, so the outline draws over them.

## Terrain filtering

Distant terrain texture is filtered by its on-screen footprint instead of
sampling single texels, which removes most of the shimmer and crawl on far
ground. The original palette colours are kept: coarse samples are resolved
through the live weather palette, fog and light rows before blending, never
averaged as indices.

- Each pixel's texture footprint is measured along its long and short axes.
  Up to 4 probes are spread along the long axis; each probe blends the two
  nearest coarse levels (bilinear within each), up to level 8.
- A coarse level L picks one fixed texel per 2^L by 2^L block, chosen by a
  hash of the block and level, so camera motion changes only blend weights and
  never swaps texels.
- Blocks stay inside their 256 by 256 tile. Cutout coverage (water) is averaged
  across the footprint and still cut at one half.
- Off, or a footprint of one texel or less, gives exactly the previous result.
- Terrain only. Airport art sits in padded layers and aircraft use a packed
  atlas, so both would bleed; ocean and sky decks and clouds are unchanged.

Measured on consecutive frames, the pixel change in distant terrain fell by 29%
to 57% depending on the view. Very distant high-contrast art keeps a fixed
grain, and thin features such as far roads fade.

## Building distance and building detail

Shown whenever the city fill is on, which it is by default (hidden only
under the developer switch `TORE_CITY_FILL=0`; see the
[feature matrix](../features.md)). John asked
on 2026-10-10 for both settings and their defaults; the steps are agent
choices. Building distance is how far city fill buildings are drawn.
Building detail is the distance inside which they are drawn with full
texture and detail; beyond it they are plain blocks in each building's
main colour. Over a band a quarter beyond the detail distance the block
fades out over its full detail building, so the change is gradual and the
edge does not show as a line. Buildings also fade in rather than appear:
over the last 15% of the building distance (4.5 nm of the default 30) and
while they grow from one to two pixels on screen (John, 2026-10-11, "let's
try phase in transitions"; the band widths are agent choices, fitted). The
fade is a fixed dither pattern spread over each pixel's anti-aliasing
samples, so with 4x a pixel blends in quarter steps and a still view never
shimmers; with anti-aliasing off it is a screen-door pattern. The target
display (target window, TV and gunsight views) draws every building it sees
at full detail whatever the setting, and fades them in at its own draw
distance. The values are saved
only when they differ from the defaults. Measured costs are in the city
fill notes; at 4K the 5 nm default adds about 2 to 5 ms over Kiev, 1 nm
about 1 ms. The fades cost about 0.1 to 0.4 ms more at 4K (2026-10-12).

## Shadows

Requested by John on 2026-10-20 ("maybe we add a shadow quality setting to
the graphics menu", with every ground object casting); the levels, what each
draws and the Med default are agent decisions. The row sets how detailed the
sun's [geometric shadows](surface-lighting.md#geometry-shadows) are and what
casts them. Every level but Off has the terrain, the aircraft and every
ground object cast: airports, buildings, bridges, SAM and AAA sites, parked
aircraft, vehicles, ships and men.

| Level | Shadow maps | Soft edge samples | City buildings cast | Trees cast |
| --- | --- | --- | --- | --- |
| Off | none | | no | no |
| Low | two maps, 1,024 texels each | 16 | no | no |
| Med | three maps, 2,048 texels each | 32 | yes | no |
| High | three maps, 2,048 texels each | 32 | yes | yes |
| Ultra | three maps, 4,096 texels each | 32 | yes | yes |

The separate **Shadow distance** row, next to Shadows, sets the outer map's
half-width and the maximum caster distance for city buildings and trees.
John requested a separate distance and a default following trees on 2026-10-21.
**Trees**, the default, follows the current Vegetation distance even when that
row changes live. At its default 4 nm this is 24,304.462 feet, using the same
6,076.11549 feet per nautical mile as vegetation. Fixed choices of **1, 2, 4,
6 and 8 nm** are agent decisions and do not move when vegetation changes.
Off ignores distance. Trees still need High or Ultra. Only loaded vegetation
on retained view pages and buildings drawn in the current view can cast.
Terrain and ordinary ground objects
are culled against each map's light-space volume. This is finite square
coverage projected along sunlight, not an exact horizontal circular cutoff;
altitude and sun angle affect the ground footprint.

Cascade spacing is an agent decision: keep the first map's 256 ft half-width
at every distance. With three maps the middle half-width is the smaller of
8,192 ft and half the chosen reach; the last map reaches the chosen distance.
Low uses 256 ft and the chosen reach. Thus longer reach never stretches the
near map, and short settings bring the middle map closer. At the default the
near and middle maps keep their previous dimensions and close-up look; the
outer map shrinks from 131,072 ft to the tree distance. The existing outer
15% map blending and tree-caster thinning over their last 30% remain.

A change applies automatically, including in flight. `--shadows
off|low|medium|high|ultra` and `TORE_SHADOWS` set quality for one run.
`--shadow-distance trees|1|2|4|6|8` and `TORE_SHADOW_DISTANCE` set distance
for one run. Environment overrides also work in offscreen tools and take
precedence over command-line/saved choices without themselves being saved.
The distance override uses the same accepted values as the row.

Distance does not allocate larger maps or add filter samples. Greater reach
includes more caster geometry and can shade more screen pixels, increasing
GPU cost, especially with tree shadows at High/Ultra. Distant shadows also
lose detail at a fixed map resolution. The older quality timings below
predate the separate distance control and used quality-specific reaches.
The current 1, 4 and 8 nm GPU costs, before/after pictures and live-application
checks are in the [shadow distance validation](../baselines/shadow-distance.md).

GPU time of a still offscreen frame at 3840 by 2160 on an RTX 4070
(2026-10-20, morning sun, `--vegetation-preview` cameras with the static
ground objects loaded, best of two rounds of seven eight-frame means, in
milliseconds; "before" is the build before this setting, where static ground
objects did not cast):

| Scene | Before | Off | Low | Med | High | Ultra |
| --- | --- | --- | --- | --- | --- | --- |
| Ukraine airfield | 11.3 | 7.0 | 10.8 | 11.4 | 12.0 | 13.2 |
| Ukraine forest, 300 ft | 7.4 | 5.7 | 7.0 | 7.4 | 9.2 | 9.5 |
| Ukraine open ground, 300 ft | 9.0 | 5.6 | 7.3 | 9.1 | 10.4 | 10.6 |
| Hanoi, 300 ft | 12.2 | 3.3 | 8.2 | 12.4 | 12.4 | 12.5 |
| Hanoi, 2,000 ft | 11.8 | 6.7 | 8.9 | 11.9 | 12.1 | 13.1 |

In that older measurement the ground objects' shadows cost under 0.2 ms;
tree shadows cost 0.3 ms over
a city and up to 1.9 ms over woods, mostly the soft filter on the newly
shaded ground. At 2560 by 1440 the same steps are about half as large.

## Vegetation texture and vegetation objects

Shown when the exploratory vegetation is on, which it is on this branch
(hidden only under the developer switch `TORE_VEGETATION=0`). John asked
on 2026-10-16 for the same kind of control the city buildings have. The
steps and the defaults are an agent choice the same day, aimed at an
RTX 4070 at 4K. Vegetation is presentation only: it is not collidable,
and it is not part of the simulation, the network session, replays or
checkpoints. What was drawn is seeded from the theater and the tile, so
every machine that uses the same settings shows the same scene.

**Ground detail** is the close-range ground detail: furrows on
fields, grass, a mottled forest floor, sand grain, ripples and dunes,
pebbles on gravel, and scree on rock. It is described in
[terrain detail](terrain-detail.md#exploratory-vegetation-and-ground-detail).
Off leaves the retail tile. On is the ordinary grain. Fine is a
tighter grain. **Ground detail distance** is how far that grain
reaches. It also fades with height, full below about 2,200 ft and gone
by about 7,500 ft, so a cruise view stays the retail tile; the desert's
broad colour fields and dune shading reach farther and fade out by
about 26,000 ft. A flare,
afterburner, fire or gun flash only adds light to it: the grain is the
same with and without one.

**Vegetation distance** is how far trees, bushes, palms and rocks are
drawn, the same idea as building distance. **Vegetation detail** sets
how far every configured instance stays at full distance opacity and density.
The setting is used directly for all species, including small stones and clumps.
Only the band from detail to draw distance fades, with matching distant-canopy
coverage. The trees, bushes, palms and rocks are the
original, tintable cutout sprites with matched side and top views. The shipped
pack is the only art set. `TORE_VEG_TINT` previews foliage colours; see the
[vegetation specification](terrain-detail.md#sprites-and-cached-placement).
**Vegetation cards** switches live between Facing, Cross, Cross+top and Face+top.
Face+top (camera-facing sides with angle-gated tops) is the default, requested by
John on 2026-10-17. Missing saved card settings take that default; explicit saved
choices remain selectable and round-trip. The original-presentation pixel-test
preset retains Facing.
`TORE_VEGETATION_CARDS=billboard|cross|cross-top|billboard-top` overrides that row
for a run and is not saved. Sprites have full distance opacity inside detail range. Distant forest coverage
continues on the terrain. Top modes use complementary side/top coverage from
35 to 65 degrees down, at the measured visual crown centre. All species, including conifers,
use their own matched top view and the common angular transition.


**Vegetation density** is Low, Med, High or Ultra. John requested a one-step
reduction on 2026-10-17: new Ultra equals previous High, High equals previous
Medium, and Medium equals previous Low. The new Low is an **agent choice** at
half the previous Low candidate density. Default remains High. Relative to the
previous High grid, the multipliers are 0.2, 0.4, 0.7 and 1.0.

| Tree density | Objects kept at most |
| --- | ---: |
| Low | 32,768 |
| Med | 65,536 |
| High | 131,072 |
| Ultra | 262,144 |

**Grass** selects Blades (branch default), Tufts or Off, live as it is chosen.
`TORE_GRASS=blades|tufts|off` overrides it for the run. **Grass density** is
Off, Low, Med, High or Ultra, independent of trees, default High. **Grass
distance** is 250, 500, 750 or 1,000 ft, default 500 ft. John requested these
controls and denser grass reaching about 500 ft on 2026-10-17. Off in either
grass row removes grass geometry while retaining ground detail, trees and canopy.

Blades use four world-snapped grids, doubling the base spacing at each level.
The following budgets are agent choices at the 500 ft default:

| Grass density | Base spacing | Grid side per level | Submitted blade slots | Maximum vertices | Legacy tufts per near patch |
| --- | ---: | ---: | ---: | ---: | ---: |
| Low | 2 ft | 128 | 65,536 | 327,680 | 24 |
| Med | 1.5 ft | 192 | 147,456 | 737,280 | 48 |
| High | 1 ft | 256 | 262,144 | 1,310,720 | 96 |
| Ultra | 0.75 ft | 320 | 409,600 | 2,048,000 | 160 |

Ultra grows to a 344-square grid at 1,000 ft, with a hard maximum of 2,366,720
vertices. Low grows to 136 at 1,000 ft; Medium and High retain their grid side. These limits include rejected
candidates in every direction; actual visible counts are lower. Since S13b
(2026-10-19) a GPU pass evaluates each submitted slot once and only grown
blades reach the vertex stage, five vertices each; the grown list costs 64
bytes per blade the circles can hold (since round 2 of the optimization, about
9.4 MB at High, 500 ft, down from 16.8 MB for every slot). Geometry stays
opaque, with no CPU placement. See [grass rules](terrain-detail.md#grass).
Grass-only GPU cost and final appearance require measurement on a GPU host.

Tufts retain their legacy horizontal footprint; Blades use distance from the eye.
In Tufts mode each tuft has five blades. Legacy grass thins in four rings: full within 14%
of its radius, then half, a quarter and an eighth. Object budgets are divided into fixed geographic patch quotas, independent
of the camera. Saturated patches retain the same seeded population throughout
an approach. This is a fitted agent choice; a saturated forest can be sparser
than its unconstrained candidate grid. Detail distance is never silently
shortened to meet the budget. Object opacity has no separate altitude or
projected-pixel fade inside detail range. Grass uses the selected mode's separate distance
and habitat rules.

The nine values are saved in `graphics-v1.conf` only when they differ
from the defaults (`vegetation-texture`, `vegetation-texture-distance`,
`vegetation-distance`, `vegetation-detail`, `vegetation-density`, `vegetation-cards`, `grass`, `grass-density`, `grass-distance`). An
older file that has none of them reads as the defaults. A change on the
screen puts them into effect at once, the same way as the building settings. Original
graphics turns the texture off and leaves the object distances at the
defaults, as it does for buildings.

## Frame rate counter

Requested by John on 2026-10-17. Off by default. On draws a single line in
the top right of the window, over the picture, in the bundled Noto face:
`62 fps  16.1 ms  cpu 4.2  gpu 11.8  low 48`. The rate and the milliseconds are the mean
frame over about the last half second. `low` is the slowest frame in the
last second, written as a rate. Slow rendering frames, including frames longer
than half a second, count. Disabling the counter resets its history. While enabled, every
screen redraws continuously at the [frame rate](#frame-rate) in effect (John,
2026-10-19: V-sync, or 60, 100 or 200 a second), including the main menu and
every menu screen, the lobby and the network screens, loading screens, the
paused flight's menu and the paused replay, so the line shows live, real
numbers and measures the actual render load. Turning it on or off applies at
once. Hidden, none of this applies: menus keep their low-power pacing (a menu
draws on input or news, a lobby also on a 10 Hz heartbeat, a loading screen at
20 Hz, a still paused flight on events only). With a capped rate, events such
as mouse movement do not add frames beyond the timer's, so the line never reads
above the cap; an unfocused window still draws about ten frames a second and
the line reads that. A gap of more than a second between frames (the window
was hidden, or the counter was off) restarts the history instead of counting
as one slow frame. CPU milliseconds are the main thread's wall-clock time
for frame preparation and encoding, excluding surface acquisition waits; they are
not the CPU seconds summed over worker threads. GPU milliseconds
are asynchronous timestamps over all of the frame's GPU work: each live camera
panel picture, the cockpit mirrors on the frames that draw them, and the window pass (scene, cockpit, interface
and the counter). Each submission is timed on its own and the figure is the sum, so
GPU idle time between submissions is not counted. Queued staging
uploads, CPU time and presentation waits are excluded. Offscreen captures and
screenshots are not timed. CPU/GPU values average up to 30 recent samples. Three
readback slots never wait on the GPU. Adapters without both timestamp features
show `gpu --`, never an estimate. GPU/CPU timings overlap and need not sum to the
frame interval. These measurement choices are agent decisions.

The line is drawn only when the window is presented. Offscreen renders,
previews, recordings and `P` screenshots copy the picture before that pass,
so they stay the same with the counter off or on. `TORE_SHOW_FPS=1` forces
the line on for one run, and `TORE_SHOW_FPS=0` forces it off. Neither value
is saved. Any other value is ignored.

Where it sits (agent decision, 2026-10-17), so it stays clear of the title
strip, the large instrument window, the HUD camera's tape counter and an
open right-hand debug panel:

| Screen | Place |
| --- | --- |
| Menus, lobby, Escape and pause screens | On a wide window, in the black bar to the right of the 4:3 canvas, 10 pixels from the top. On a 4:3 window, just under the title strip. |
| Loading screen | 10 pixels from the top and the right. The loading words are centred. |
| Flight | In the corner above the large instrument window when that margin holds the line (1080p and above). Otherwise just under that window, still on the right. If a right-hand debug panel covers that spot, the line moves to the left of the panel. |
| Replay viewer | In the corner. The mission timer is on the left, and the HUD camera counter starts 7% down, so the corner is above it. A right-hand panel starts lower and leaves the corner free. On a short window the line drops under the tape counter. |

## Command line

For one run, without saving: `--anti-aliasing off|2x|4x|8x`,
`--render-scale 75|100|125|150|200`, `--spotting-aid off|subtle|strong`,
`--terrain-filtering on|off`, `--building-distance 10|20|30`,
`--building-detail 1|2|5|10|20`, `--vegetation-texture off|on|fine`,
`--vegetation-texture-distance 1|2|4`, `--vegetation-distance 2|4|6|8`,
`--vegetation-detail 1|2|3|4`, `--vegetation-density low|medium|high|ultra`,
`--shadows off|low|medium|high|ultra`, and `--original-graphics` for everything off
at 100%. `--graphics-api` also lasts one run but follows [its own rules](#graphics-api). `TORE_SHOW_FPS=1` shows the frame-rate counter for that run without
writing it into the file, and `TORE_FRAME_LIMIT=off|vsync|60|100|200` sets the
[frame rate](#frame-rate) for that run. Flags apply in order, so `--original-graphics --spotting-aid strong`
turns on only the outline. Any change on the Graphics screen saves everything
the screen shows, including a flag's value, except `--graphics-api`.

## Cost

Measured in a release build at 1280 by 720 on an RTX 4070, median frame time;
see [flight performance](../baselines/flight-performance.md) for the method.

Free flight with view cycling (`TORE_PERF_FRAMES=630 TORE_PERF_ACTIVE=1
TORE_PERF_VIEWS=1 --free-flight`), three runs each, measured 2026-09-22:

| Settings | Median frame |
| --- | --- |
| `--original-graphics` (everything off) | 4.23 to 4.26 ms |
| Defaults with anti-aliasing off | 4.43 to 4.47 ms |
| Defaults (4x, 100%, Subtle, filtering on) | 4.65 to 4.68 ms |
| Defaults with 8x | 4.88 to 4.89 ms |
| Defaults at 150% render scale | 9.11 to 9.13 ms |
| Defaults at 200% render scale | 14.75 to 14.78 ms |

In a separate ten-aircraft formation run the spotting aid added about 0.04 ms.
Render scale is the only expensive option.

## Set aside

Sun glint (a brief sparkle when sunlight catches another aircraft) was tried and
set aside by John on 2026-09-22. The tried design was an extra pass over other
aircraft that lit a near-white flash when the reflected view ray came within 1
to 3 degrees of the sun, scaled by daylight, cloud, shadow and haze. A
`quality` uniform slot is reserved for it.
