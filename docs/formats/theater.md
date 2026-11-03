# Original theaters and atmosphere: first recovery pass

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

> **Research notes, research mode.** Recovered facts about the original
> game's data and code, kept as evidence. Requirements, gates and remaining
> work described here are research-mode scope; they are not acceptance gates
> for gameplay. Parity is measured by expression of feature, see
> [AGENTS.md](../../AGENTS.md). Player-visible behaviour is specified in
> [docs/spec/](../spec/).


Checkpoint: 2026-09-13, supplied Fighters Anthology installation. This is partial M1b recovery and an executable Ukraine preview, not accepted 1:1 environment parity. No USNF-ATF terrain, DEM, satellite imagery or engine code supplies this surface. The original executable was inspected as data and disassembly; it is never loaded or executed by the importer.

## Reproduce and identify the inputs

```sh
python3 tools/extract_assets.py --theater UKR --out .local/ukraine-import
mkdir -p .local/theater-research
objdump -d gameassets/fighters-anthology/FA.EXE > .local/theater-research/fa-disassembly.txt
```

Addresses below are virtual addresses in this exact `FA.EXE`, SHA-256 `e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c`. They are not portable to another edition. The extraction report records archive/output SHA-256, offsets and decoded sizes, plus structured terrain/mission metadata. Full inputs and derived captures remain ignored. Readers live in `crates/tore-formats/src/theater.rs`; runtime construction in `crates/tore-world/src/terrain.rs` (queries) and `crates/tore-app/src/scenery.rs` (art and mesh).

Runtime terrain adds [shared static foundations](objects-and-shapes.md#grounding-every-object-stands-on-the-bottom-of-its-shape)
after resolving the layout and objective. These fitted triangles serve both
contact and drawing; the imported BIT2 grid and its classification are unchanged.
Bridge support includes the fitted [bridge corridor](../spec/bridges.md), which
fills physical bank supports and grades outward terrain approaches on sloping
crossings. Flat crossings retain their existing level corridor.

## BIT2 / T2: confirmed packed layout

All 16 supplied T2 resources parse with these rules. Multi-byte numbers are little endian; names are null-terminated ASCII. Unknown header bytes remain in the extracted original file and are not assigned runtime meanings.

| File offset | Field / evidence |
| --- | --- |
| `0x00` | `BIT2` magic |
| `0x04`, 80 bytes | Theater display name |
| `0x54`, 16 bytes | Briefing map resource name |
| `0x64` through `0x78` | Legacy/unknown header fields; do not interpret as grid dimensions |
| `0x79` | u32 cells per coarse tile, 8 in this installation |
| `0x7d`, `0x81` | u32 coarse tile columns, rows |
| `0x85` | u32 coarse-grid file offset |
| `0x89`, `0x8d` | u32 fine-grid columns, rows |
| `0x91` | u32 fine-grid file offset, **149 / `0x95`** |
| Fine grid | Row-major three-byte cells: **palette color, class, elevation** |
| Coarse grid | Same cell representation; follows fine grid with no trailing byte |

The loader relocates `+0x85` at `0x4c5e85` and `+0x91` at `0x4c5e92–0x4c5e9a`. Lookup `0x4c6040` uses the fine grid below a step of `cells_per_tile`, otherwise divides coordinates by that value and uses the coarse grid. Rust implements both lookups; the initial renderer uses the full fine grid.

The terrain block builder at `0x4a9d00` copies the third byte of four corner cells into heights, and the first two bytes into material/class fields. Geometry construction at `0x4aa4f1–0x4aa56c`, world coordinate shifts around `0x4a9e32`, and elevation conversion at `0x42c091–0x42c0a4` establish **8,192 feet per horizontal cell and 256 feet per elevation unit**. Ukraine has 208 × 200 samples, 26 × 25 coarse tiles, and elevations 0–31 (0–7,936 feet). The preview uses X/east, Y/up, Z/north as a presentation convention; landmark orientation still needs native comparison.

**Lesson:** the reference reader's `0x94` starting offset shifts every triple by one byte, and its dimensions/elevation interpretation is not the retail layout. T2 contains actual height samples. A DEM is unnecessary to recover this terrain. This correction supersedes the earlier unresolved-elevation note in our progress tracker; it does not establish native tessellation or shoreline parity.

## Terrain textures and placements

`UKR.MM` contains 697 top-level `tmap col row texture rotation` lines at 695 unique coordinates and 29 `tdic 256` dictionaries. Coordinates (196,56) repeat texture 23 with rotations 1 then 3; (144,64) repeat textures 13 then 11, both rotation 3. The current Rust map/export retains the last entry; native duplicate precedence remains unverified, and all original lines remain in the extracted MM. The native parser at `0x482f69–0x482fb6` reads the four placement fields. Initialization at `0x4aa620` builds `%s%d.PIC` names, selecting `UKR0.PIC` through `UKR28.PIC` (256 × 256 each). Lookup at `0x4aa840` rounds cell coordinates down to multiples of four and searches eight-byte records by `(row << 16) + col`; texture and rotation are at `+4` and `+6`.

Each placement covers four cells per side, or 32,768 feet. Quarter-turn UV selection starts at `0x4aa9ac`; texture scanning around `0x4aa72d` reverses source rows. Rust applies these rotations and a V flip. Index 255 is tested as cutout/water coverage at `0x4aa739`. The runtime now treats those texels as holes exposing the shared ocean/horizon pass; untextured color-255 cells also leave that pass visible. The former land-color fill and palette-223 water fallback caused rectangular green strips beyond beach artwork and have been removed. Bilinear coverage uses a fitted 0.5 cutoff, not a recovered native raster threshold. See [shoreline behavior](../spec/terrain-shorelines.md) and [validation](../baselines/ukraine-viewer.md#shoreline-correction-2026-09-16).

Named tiles and fitted LAND/VLAND presentation are implemented. Source-aware terrain detail transitions and class-dependent material behavior remain open; the host uses pixel coverage directly rather than the original `tdic` cache. The [terrain material review](terrain-materials.md) establishes the dictionary contract and named Kurile artwork, while distinguishing the remaining land-plane questions. Exact retail shore/water geometry and lighting remain unverified; current lighting is described below. Fixed triangles currently join each four-sample quad; the height query uses those same triangles. `UKR.MM` contains 257 object placements, now imported into the static scene with
original shapes/textures, target identity and contact geometry. The sixteen base
theaters share this path. [Airport placement contract](airport-placements.md)
records unsupported shape and campaign boundaries.

The ocean now uses user-requested short ripples and distance/altitude filtering,
retaining the original textures and weather colors. Whitecaps are removed. [Source contract and limits](ocean.md),
[behavior](../spec/ocean.md), [acceptance](../baselines/ocean.md).

## City texture light areas (CF0 prototype)

Measured 2026-10-10 over all sixteen theaters (`tore-app --city-fill-survey`);
the reader is `tore_world::city::mask` and the fill that uses it is
`tore_world::city` (on by default since 2026-10-11; `TORE_CITY_FILL=0`
turns it off in single player). The terrain class and colour bytes do
not mark cities ([terrain materials](terrain-materials.md)); the city tile
pictures do.

- Fifteen theaters reuse one set of city pictures, the same indices under
  each theater's prefix: 1 (downtown), 2 (edge), 3 (corner), 4, 5, 6 and 26
  (coastal), 55 and 56 (towns sitting in fields) and 60 and 62 (Baltic). A
  full city is a 3 by 3 block of placements, about 16 nm across, with 1 in
  the middle; corner pictures alone make the smaller cities. North Vietnam
  has none: its retail city clusters sit on jungle pictures.
- The art paints built-up land in one ramp of palette indices, 198 to 207;
  192 to 197 and 208 to 215 are parks and fields (sand in Egypt), 224 a
  grey-green dot, 217 to 223 blue dots, 254 white, 255 the water cutout.
  Each theater's reference palette (the clear-day layer, condition 0, at
  noon) paints the ramp light to dark (Ukraine: 198 luma about 225, 207
  about 30; Egypt 219 to 28) in every theater but North Vietnam, where it is
  jungle green.
- Classes from that palette: light roofs are ramp indices with luma at
  least 115 (and 254); residential streets ramp indices with luma 100 to
  180 and the 224 dots; downtown streets ramp indices darker than 90 and
  224. A texel is a street when its run of either street class along a row,
  column or diagonal is at least 8 texels (1,024 ft) and at most 2 (256 ft)
  across.
- A picture is city art when at least one land texel in twenty is a light
  roof, at least one in fifty steps three or more ramp shades from its right
  neighbour (Kurile's snow and rock, smooth ramp art, stay under one in a
  hundred), and either at least half its land is ramp (city pictures) or it
  has no water, at least a quarter ramp and at least half of its non-green
  land ramp (the town pictures; shoreline pictures also carry ramp sand but
  have water).
- Result: city or town pictures in every theater but Kurile; North Vietnam
  fills each retail cluster's footprint at the retail shape scale (three
  times its drawn size) as a round village thinning from 60 to 10 percent
  of texels toward its edge.
- At 128 ft per texel the downtown blocks are about 2,000 ft and the painted
  roofs 500 to 1,300 ft: the art is drawn at about the retail 3x object
  scale.
- World to picture: texel `(tu, tv)` east and north inside the placement
  maps to picture column and row `(tu, 255-tv)`, `(255-tv, 255-tu)`,
  `(255-tu, tv)` or `(tv, tu)` for rotations 0 to 3, as the terrain mesh
  draws it (picture row 0 is the north edge at rotation 0). A quarter turn
  swaps the two diagonals.

The development aid `tore-app --city-fill-dump OUT [THEATER]` writes each
city's mask and fill images and prints the counts; its render, sequence,
sweep and bench options are in [development](../DEVELOPMENT.md).

## Mission environment and weather modules

Follow-up: [native clock and turbulence research](weather.md) confirms continuous
time and LAY time-window selection. The dynamic palette implementation and its
remaining approximations are documented in [the full weather review](../baselines/weather-review.md);
it supersedes the historical midday-only runtime descriptions below.

The bounded `textFormat` reader exports top-level map, layer, layer parameter, clouds, wind, time and texture placements. The separate bounded mission reader now parses the indented object fields for
static scene construction. `UKR.MM` specifies `UKR.T2`, `DAY2.LAY 0`, clouds 0 and time 12:00; wind is absent and remains null. For example, `UKR01.M` specifies layer parameter 4, wind `160 7` and time 17:40. Wind units and the layer parameter's full semantics remain unverified.

Campaign missions reference names such as `~UKR6.T2`. Preserve these names; do not silently redirect them to `UKR.T2`. The parser accepts `~` and `$` resource-name characters. Resolving generated campaign terrain aliases is future work.

LAY files here are **PL modules containing weather data and native imports**, not menu layouts. The reader resolves data RVAs inside the bounded CODE section without executing code. The CODE root has the base 768-byte six-bit palette RVA at `+0x70`, and the weather-record table RVA at `+0x74`. Records are 352 bytes (`0x160`); flags bit 0 marks the sentinel.

Native palette copying at `0x4b4592–0x4b45c5` and palette writes at `0x4b3631–0x4b3661` establish:

| Record bytes | Destination palette entries |
| --- | --- |
| `+0x3e .. +0x9b` (93 bytes) | 224–254 (31 RGB colors) |
| `+0x9b .. +0xfb` (96 bytes) | 192–223 (32 RGB colors) |

Rust expands six-bit RGB to eight-bit and uses **explicit DAY2 keyframe 2** for a midday preview. Native time/altitude keyframe selection and fixed-point interpolation (`0x4b3820`, helpers `0x4b3b60`, `0x4b3b80`) remain to port. The two palette ramps are not a single contiguous destination range: reversing them produces incorrect terrain and sky colors.

DAY modules import `_T_HorizonProc` from `main.dll`; this build's export at `0x4aace0` is a return stub. FOG also imports `_WRFogLayerUpdate`; `0x4b4320` adjusts a field at `+0xfe` by a random -25…25 and clamps it to 217…235. This callback, its caller timing and seeded weather evolution are not implemented.

## Text character set

Retail text is 8-bit DOS text in code page 437. The evidence is the original
fonts: `WIN`, `HUD` and their variants draw exactly the CP437 text characters
above 0x7F (accented letters at 0x80 to 0xA5, the inverted marks at 0xA8 and
0xAD, sharp s at 0xE1, micro at 0xE6, degree at 0xF8) and no box-drawing or
Greek cells. Across every imported text resource the only byte above 0x7F is
0x89 in `KURILE.MM`, the airport name `Ber\x89zovka`: an e with a diaeresis
(Berëzovka). The app decodes mission layout text as CP437
(`tore_formats::text`), holds it as Unicode, and draws it by putting each
character back on its CP437 font cell. The bundled menu font atlases carry the
same cells. `--validate-text` scans every imported string for U+FFFD and for
characters the original fonts cannot draw.

## Sky, sun, moon, stars and clouds

The shared extraction profile includes all of the following, preserving archive boundaries:

| Resources | Recovered dependency / current use |
| --- | --- |
| `SKY0.PIC` … `SKY8.PIC` | Nine 256 × 256 indexed sky textures; SKY0 is rendered |
| `SUN.SH` | Original sun shape, no literal PIC reference found |
| `MOON.SH`, `_MOON.PIC` | Shape references `_moon.PIC`; both preserved |
| `STARS.SH` | Original star shape, no literal PIC reference found |
| `CLOUD1.SH`, `_CLOUD1.PIC` | Shape references `_cloud1.PIC`; both preserved |
| `CLOUDS.SH`, `CLOUDS.PIC` | Shape references `clouds.PIC`; both preserved |
| All `.LAY`, `PALETTE.PAL`, `GRND*.PIC`, LAND/VLAND variants | Preserved for weather, palette and terrain follow-up |

SUN/MOON/STARS resource-name references occur in FA.EXE at `0x50c42c`, `0x50c434`, `0x50c43c`. Sun and stars appear to use shape commands rather than separately named PICs; this is an inference from literal references, not complete SH interpretation.

The renderer now selects source sky/ocean decks and uses world-anchored plane projection. Original sun circles/glow remap, moon billboard and point stars render with live weather palettes and source clock/angle gates. Cloud geometry and special horizon/ray-fog behavior remain under implementation; see [weather foundation](../baselines/weather-foundation.md) for current evidence and limitations.

## Next recovery steps

The [retail terrain detail plan](../ROADMAP.md#retail-terrain-detail-review)
owns current terrain and scenery sequencing. The named-tile and coverage
contracts are in [terrain materials](terrain-materials.md). Weather and
celestial work is tracked in the linked weather guides. All 75 static layouts are selectable. Live campaign composition and further
state-dependent source-detail behavior remain open. Retail visual comparison is unavailable.

## Broader extraction checkpoint

All 16 named profiles now share one definition table; `--theater all` selects their union. The supplied disc additions introduce no extra matching terrain resources, but supply reference media and audio/video archives. See [extraction commands and scope](../EXTRACTION.md#all-defined-theaters-and-the-retail-discs).

All 75 MM layouts parse. PGU.MM contains border placements (-4,244), (-4,248), (-4,252); SPA.MM contains (48,-4), (52,-4), (56,-4), repeated in their campaign layouts. Texture placement coordinates therefore use signed integers. The initial Ukraine renderer still validates its own placements against its grid; parsing the other theaters does not assert their rendering parity.

## Runtime expansion to the 16 base theaters

The viewer uses the selected base or variant MM, its referenced terrain and theater-specific weather palette, and paged indexed artwork. The numbered texture naming convention uses the first three code characters: TVIET therefore needs TVI0–41.PIC. The shared profile now includes TVI. Kurile has no numbered tmap placements; its 236 named placements now render with their original artwork. [Named-tile contract](terrain-materials.md#named-texture-placements). Other theaters have 29–68 numbered texture layers in this preview.

Signed out-of-grid border placements remain preserved; the mesh only queries patches intersecting actual fine-grid quads. Camera starts and fixed triangles are authored investigation behavior. All 16 passed Metal startup/render checks; native landmark/shoreline/atmosphere parity remains open. See [runtime validation](../baselines/all-theaters.md#runtime-and-typography-follow-up).

## Climate (not in the retail data)

The retail theater files carry no humidity or climate. TORE gives each of the
16 base theaters a fixed, fitted humidity for the look of a large blast
(condensation cloud when humid, lingering tan dust when arid), pushed toward
humid by a fog or overcast weather choice; the table and rules live in the
explosions spec's [climate](../spec/explosions.md#climate) section and in
`tore_world::climate`. It is presentation only.

## Diagnostic native contact geometry, 2026-09-15

The vertical cell diagnostic now preserves the native diagonal, word normals,
integer intersection and imported square-root table rounding. It does not change
the preview renderer or its height sampling. Native fine lookup returns a zero-
elevation/class-1 fallback for out-of-grid corners; it does not clamp to the last
sample. [Contract](native-land-contact.md#vertical-terrain-arithmetic-ne-001b),
[measured decoder/replay coverage](../baselines/native-land-geometry.md).

Smooth presentation adds continuous sunlight and geometric shadows to the
existing terrain triangles, with shared area-weighted vertex normals for
continuous lighting across their edges. Shadow positions and height queries
still use the source triangles. This is an authored renderer, not newly recovered
terrain behavior. See the [shared surface spec](../spec/surface-lighting.md).

The in-flight map fits the original briefing image to the T2 grid extents,
with positive world Z at image north. T2 cell colors provide a fallback. This
is fitted cartography; the exact source projection has not been recovered. See the [map specification](../spec/flight-map.md).


Airport terrain recesses are a fitted rendering layer, not a change to T2
samples. The renderer splits triangles along each oriented airport footprint,
retains the original texture coordinates, and lowers only the inside surface
beneath the fixed runway plane. See [airport integration](../spec/airports.md#integration-choices)
and [the visibility regression](../baselines/airfield-radio.md#ground-visibility).

## Merged-airfield ground pictures

The scene-3 paired-airport replacement marks `tmap` cells intersecting the
source strip footprints as neutral airfield ground. Scenery retains the tile's
own ground texels, using the existing terrain green ramps (192 to 197 and
208 to 215). Other opaque texels inherit the nearest original ground texel
by Manhattan distance on the wrapping 256-square tile. A multi-source queue
starts in row order, expands left, right, up, down, and resolves ties by first
arrival. Original ground texels and index-255 or masked water stay unchanged.
A tile without a ground donor is an explicit error. Rotation stays unchanged.

This is an agent-selected **fitted** cleanup rule, not a recovered airfield-art
classifier. It also removes non-ground road or built-up markings within the
selected source tiles. It avoids importing the unrelated LAND plane's palette
ramp and large-scale blotches into otherwise continuous ground. The merged
headless audit prints means, population variances and dominant palette indices
for each repaired tile and adjacent untouched tiles, and checks exact ground
and water preservation. Those statistics describe indexed source pixels, not
lit GPU colors. No protocol, checkpoint or contact surface changes accompany
this presentation repair.
The imported T2 and MM bytes remain unchanged. Ground-picture water queries
retain that same source mask, so neutralization does not fill coastal sea.
See [merged airports](redrawn-airports.md#merged-pairs-airfield-scene-3).

## Exploratory vegetation classification

The `exp/vegetation` renderer samples this theater data and its reference
palette to place presentation-only sprites and classify GPU grass habitat. It does not add terrain records
or establish original vegetation placement. Cached patch, field, road, village
edge, desert, broad-water beach exclusion and canopy sampling rules are specified in
[terrain detail](../spec/terrain-detail.md#where-things-grow); retail card
records and unresolved original distance rules are in
[objects and shapes](objects-and-shapes.md#vegetation-card-assets).

GPU blade grass additionally caches a two-bit habitat per 128 ft source-art
texel and a terrain-vertex lookup grid. This is disposable presentation data,
not a new retail format. See [grass habitat](../spec/terrain-detail.md#grass) and
[renderer storage](../ARCHITECTURE.md#exploratory-vegetation-rendering).

The exploratory vegetation habitat uses a device-sized atlas of 256-square
logical pages, independent of theater rows. The shared canopy/card atlas and
all vegetation buffers are checked by the CPU-only
[theater limits audit](../testing/README.md#vegetation-theater-limits).

## Temperate tile colour audit

The 2026-10-18 UKR/FRA/BAL audit compares imported layout, T2, picture and weather
payloads with the source archives, recomputes `tdic` coverage, and checks the
render mesh's texture pages and quarter-turn UVs. No new per-tile palette or
shade mapping is established. The source pictures differ in colour, including
the fitted LAND fallback. The reviewed UKR seam has mottled LAND.PIC south of
the edge and flat UKR23.PIC north of it. The renderer retains both source pages
and applies a cached texture response inside accepted 500 ft bands. Separately,
statistically flat natural pictures receive fitted world-space modulation,
approved by John on 2026-10-18. Mottled interiors and protected features retain
their source colour. Imported resources and T2 cells stay unchanged. Retail runtime appearance and exact fallback-land
projection remain unverified. [Measured evidence](../baselines/terrain-seams.md)
and [exploratory seam presentation](../spec/terrain-detail.md#tile-seams).
