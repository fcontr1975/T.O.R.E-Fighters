# Ground textures and map detail

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode, 2026-09-23. John selected retail textures, artwork, scenery
and map variants for all maps. Shaders and expanded landscapes are deferred.
Source detail and remaining fitted presentation rules are specified below;
retail visual comparison remains unavailable.
[Evidence](../baselines/retail-terrain-review.md),
[source contracts](../formats/terrain-materials.md), and
[delivery plan](../ROADMAP.md#retail-terrain-detail-review).

## Source landscape

The player flies over the original theater's hills, mountains, plains, islands
and coastline. The source terrain samples are 8,192 feet apart horizontally;
one height unit is 256 feet. The reviewed base maps have 200 to 256 samples
per axis and maximum sampled elevations from 4,096 to 7,936 feet. The
[theater census](../baselines/retail-terrain-review.md#base-theater-census)
gives the individual maps. These are source sample limits, not a claim about
the exact silhouette produced by retail's terrain interpolation.

Authored texture placements cover 32,768 feet per side and retain their source
location and quarter-turn orientation. A 256-square tile provides 128 feet
per texel; a 128-square tile provides 256 feet per texel at the same footprint.
Retain the original indexed artwork and resolve it through the selected
theater's live weather palette. A painted river, road or field is already
landscape detail and must not be discarded because it is not a separate object.

Weather choices retain the map's retail palette family. Egypt, Iraq, Persian
Gulf and Spain use the sand family by day and at night; North Vietnam uses its
MM's unsuffixed family. All sixteen families are listed in the
[weather contract](../formats/weather.md#theater-palette-families).
This applies to textured and bare ground, distance filtering, daylight colours
under night lights, city classification, and maps that use terrain colours as a
fallback. Briefing pictures keep their own embedded palettes. Provenance:
**spec-derived**, from the explicit retail MM layer references.

Kurile has 236 explicitly named texture placements, including coastline and
interior land artwork. They are part of its landscape even though the previous
host displayed only its cell colors. Both source image sizes use the same
placement footprint. Index 255 exposes water according to the existing
[shoreline behavior](terrain-shorelines.md). A land patch can sit on a cell
whose base color is water.

Provenance: source spacing, heights, placement geometry, dimensions and texture
identity are **native data**, to be implemented as **spec-derived** behavior.
The current fixed triangle surface, shared lighting normals, filtering and
cutout threshold are **fitted** or **opinionated** presentation, as recorded in
[surface lighting](surface-lighting.md) and [graphics options](graphics-options.md).
Changing them must not silently change the runway or contact surface.

## Placed scenery

The manual identifies permanent runways, roads, buildings and bridges on maps,
and lists rocks, crop fields and urban/industrial scenery in its object catalog.
The [airport specification](airports.md) applies to their source identity,
position, orientation and independent object behavior. Hills are not replaced
with enlarged rock objects. Reuse explicit source shape and picture references.

Availability in the catalog does not imply presence everywhere. A road can be
painted into a tile, a placed object, or mission-specific scenery. Do not scatter
extra rocks, trees or towns as if they were recovered placements. No autonomous
movement, targeting or ground-defense behavior is added by this visual review.

## Implemented source coverage

The host imports and offers all 75 reviewed MM layouts: sixteen base maps and
59 variants. Every placement with a main shape in those layouts projects into
the static scene. No-body controller records remain in the manifest. Referenced
scenery images retain their full dimensions and pixels through paged storage;
shape line records render as one-pixel strokes. Original flight adapters and
120 Hz simulation remain unchanged.

CHAP and SA2 use a **fitted static loaded pose**. Their reviewed shape records
provide the actual launcher and missile geometry; the host selects it without
calling the retail load-count callback. This is visual scenery only. Scenery
animation, live weapon inventory and autonomous operation are not added.

Destroyed scenery follows the [surface destruction rules](surface-defenses.md#destroyed-looks),
including the [bridge branches and corrected footing](bridges.md). Complete original
damage-state appearance, distance-dependent model selection and unresolved
runtime decal slots are not claimed by this static scenery expansion.

## Unknown behavior and next evidence

| Missing rule | Next bounded research step |
| --- | --- |
| Generic land texture projection, repeat scale and visibility | Review the identified land-plane callers, then specify player-visible scale or an explicitly fitted choice |
| Meaning of T2 classes for land appearance | Follow the class consumers; do not infer biomes from the class numbers |
| Retail hill interpolation and distance detail transitions | Review representative steep cells and the terrain builder; record silhouette/transition behavior rather than its control flow |
| Full scenery shape detail and destroyed appearance | Compare reviewed shape branches with the current projection, then follow explicit state and damage references |
| Live campaign landscape changes | Static variants use the explicit fitted selection rule below; recover progression and destruction persistence separately |
| Trees implied by two empty catalog definitions | Find explicit usable shape resources and placement evidence; the referenced SH files are absent from the two reviewed gameplay archives |

## Host presentation where the source rule is incomplete

The following are **fitted agent choices**, not recovered original rules:

- Bare non-water terrain uses the selected LAND/VLAND artwork over four cells,
  32,768 feet per repeat, matching the existing authored-patch footprint. Source
  heights, mask holes and live theater weather colors remain authoritative.
  The original land-plane projection/scale remains a research question. This
  supplies retail artwork, not added procedural material detail.
- A selected retail MM variant supplies the complete scenery list for that
  view. It is not appended to the base list: absent placements remain absent.
  The exact MM name remains visible and source aliases remain intact. Reviewed
  generated T2 names select their theater's base grid when no literal resource
  exists. This is a static layout presentation, not a campaign-state engine.
  Mission-dependent destruction and campaign progression are outside it.
- Imported image pixels remain intact. GPU paging and exact nearest replication
  of 128-square terrain art into 256-square storage are host storage choices.
  Neither operation adds detail or downsamples large scenery textures.

## Future enhancements

Optional surface shaders and expanded landscape geometry/vegetation are
[deferred roadmap items](../ROADMAP.md#future-terrain-enhancements), as John
requested on 2026-09-23. No new shader material noise, snowline, terrain
movement or invented scenery is included in this implementation. The
`exp/vegetation` branch is an exploratory exception (John, 2026-10-16):
close-range ground detail and non-collidable vegetation, set from the
Graphics screen
([vegetation](graphics-options.md#vegetation-texture-and-vegetation-objects)).
It is not part of the terrain-detail behaviour above.

## Exploratory vegetation and ground detail

`exp/vegetation` only. John's requested visual additions are **opinionated**:
vegetation on 2026-10-16 and a second pass the same day (retail trees, placement
from tile art, denser grass and a better desert). The placement and shading
constants are **fitted agent choices**, initially fitted by eye in passes 1 and
2. Runtime retail resources retain their source provenance; the code implements
this behaviour specification rather than original control flow. John reviewed passes 3 and 4. Passes 6 and 7, requested on 2026-10-17, await GPU visual and timing validation.
The new placement is not a claim about retail placement. Nothing is
collidable or part of the simulation, the network session, replays or
checkpoints.

### Sprites and cached placement

John requested sprites and caching, then denser ecological placement and a generated sprite option on 2026-10-16. The following
numbers are **opinionated agent decisions**, pending offscreen visual review.
The retail asset evidence and unresolved original distance rules are in
[objects and shapes](../formats/objects-and-shapes.md#vegetation-card-assets).

Trees, bushes, clumps, palms and rocks use indexed cutout cards. Four modes are
available in Graphics and through `TORE_VEGETATION_CARDS`: `billboard`,
`cross`, `cross-top`, and `billboard-top` (default, requested by John on
2026-10-17). A facing card has two triangles; a
fixed seeded cross has four. Top modes add two triangles for a horizontal card
at the visual crown centre, spanning the side silhouette's opaque crown width.
The centre is the opaque-pixel-weighted height of rows at least 45% as wide as
the broadest opaque row, excluding narrow trunks. Each original side has its own measured crown. Heights clamp to 5% through 95% of the grounded instance.
These are **fitted agent choices** for John's 2026-10-17 request.
Side and top use the same instance height and seeded width multiplier.
Original side and top cells fill a 124 by 124 usable area inside a two-pixel
gutter. Side aspect and top aspect are restored from their individual metadata.

Top coverage is zero through 35 degrees down from the horizon. Smoothstep of
the crown-to-eye ray's vertical component reaches full coverage at 65 degrees.
In top modes the side coverage is one minus the top coverage, so they never
both have full coverage. Their sample masks are complementary too: overlapping
opaque interiors cover every sample once, without holes at the midpoint. The 65-degree endpoint is a **fitted agent decision**,
pending the 30/35/40/45/55/70/89-degree GPU series. This angular representation
change is separate from distance opacity and identical in depth and colour.
Crosses retain their orientation as the camera moves. They may expose their X
at oblique angles and cost more covered pixels; no speed advantage is claimed.
Every side card starts at the base supplied through `tore_world::grounding`.
Top cards are part of that grounded instance, never separately terrain-snapped.

John requested original, tintable replacement art on 2026-10-21 (**opinionated**).
Only the shipped original pack is used. It has 54 individual plants and rocks,
three variants of 18 kinds, each with a matching side and vertical overhead view.
Broadleaf and birch variants also serve autumn trees and woodland groves;
conifers and palms serve their respective grove species. Groves remain individual
plants on the existing 80 ft woodland and 110 ft palm candidate grids.
The unchanged species height ranges and seeded width variation scale each
sprite's own measured aspect. All conifers now have matching top views and use
the common transition. There is no multi-tree card or procedural crown top.

The built-in image generator received text only, with no retail image or derivative
as input. Generic descriptions, original silhouettes and colour rules are
**fitted agent choices**, not recovered original art or scale. Exact prompts,
source hashes, packing rules and material IDs are in the
[asset README](../../crates/tore-app/assets/vegetation/README.md).
No retail vegetation shape, texture or local top override is loaded. New imports
stop selecting the extra retail tree resources. Old imports remain readable;
legacy tree resources remain excluded from the theater fingerprint. Explicitly
placed retail scenery rocks still belong to the static scene, not this vegetation
system. Runtime tile classification and placement still read the player's terrain.

One byte stores each pixel: material times 32 plus a shade from 0 to 31;
255 is transparent. Materials are foliage, bark, stone and straw. The shader
multiplies its material colour by `0.25 + 1.25 * shade / 31`. Colours are linear
RGB; nearest colour samples preserve coarse pixels, while the existing alpha
coverage, grounding, surface lighting, fog and shadow passes remain.
Transparent texels contribute coverage only, never a bright material colour.
Where edge smoothing covers a sample whose nearest texel is transparent, its
colour comes from the closest opaque contributor in the bilinear footprint.
This **fitted agent choice** removes the white fringe on thin dead branches
without changing silhouettes, dimensions or the authored opaque shading.
The conservative authoring defringe thresholds are documented in the asset README.

Foliage starts from sRGB temperate (78,103,44), desert (104,111,57), arctic
(69,89,65) or tropical (48,91,43). It mixes 28% local tile colour, with each
channel clamped to 35..150, then converts to linear RGB. Temperate tiles with
red at least green, green above 55 and green more than 20 above blue use a
golden (154,133,45) base. This colours suitable groves without changing placement.
Seeded brightness is
0.86..1.10, with opposite red/blue hue jitter up to 6%. Autumn species use
(167,125,40), with brightness 0.85..1.10. Dry shrubs use linear
(0.20,0.14,0.065). Existing stone colour still follows local ground, including
its seeded brightness and varnish. Bark has an independent theater colour:
(102,78,53), (116,91,60), (95,88,76), or (88,69,48), in the same climate order.
Distant canopy integrates the sprite's material shading with these same colours.
These constants are **fitted agent decisions** and remain open to art review.

`TORE_VEG_TINT=theater|spring|summer|autumn|winter-dry|desert` is a debug colour
preview read when the process starts. Theater is the default. Other choices
replace the foliage base with (103,143,56), (58,100,43), (176,110,35),
(119,110,83), or (134,126,67), respectively; seeded variation stays. They affect
neither species selection nor placement. Restart after changing the variable.
The tint mode participates in the local canopy cache key. This is a future
season/weather input hook, not a season or snow simulation. Future wetness or
snow should adjust material colours/shading at the material input, leaving
cutouts, world scale and placement intact. No weather values are active today.

The old `TORE_VEGETATION_SPRITES` art selector is retired; if explicitly supplied,
only `original` is accepted. The obsolete profile `vegetation-overrides` directory
is ignored, not deleted. `--vegetation-art OUT` exports all shipped side/top
pairs and fitted reference sizes without requiring retail vegetation media.

Each 2,048 ft patch has a fixed candidate set for its density and draw-distance
settings. A patch's quota is the object budget divided by the number of patches
in the guarded draw circle, rounded down. Where needed the lowest seeded ranks
fill that quota. The cap never changes with camera distance or local load.
This **fitted agent decision** can reduce the overall population of saturated
groves, but approaching cannot add instances inside the detail distance.
All species, including clumps and stones, retain that complete population at
full distance opacity throughout the selected detail distance. The former 0.4
multiplier, species-specific reaches, budget squeeze, rank ghosting and near
pixel/altitude fades are removed.

Only the band from detail to draw distance uses smoothstep distance coverage.
Horizontal ground distance controls it in both the object and distant-canopy
shaders. Canopy supplies the complementary coverage. The canopy's expected
crown-area estimate remains fitted and does not reproduce each capped patch.
Side/top angle coverage and silhouette edge antialiasing still apply.

The worker builds near patches first and prioritizes forward patches using a
five-second velocity projection, rounded on a 512 ft grid and capped at
4,096 ft. The ring extends two chunks
beyond draw distance, enough for about five seconds at 480 kt. Mission loading
builds and uploads the initial ring before starting flight. A replacement ring
publishes only when complete; unchanged patches keep their bytes. Normal uploads
remain capped at 256 KiB and 16 writes per frame. No incomplete detail ring is
made visible. Available outer range expands at 6,000 ft/s after a teleport;
a slow worker may shorten it. CPU checks do not prove every host keeps up.
Near plants cast sun-facing card shadows at High and Ultra shadow quality,
within the configured shadow distance, using the same cutout as supplied
sprites. See [shadows](graphics-options.md#shadows) and the
[streaming design](../ARCHITECTURE.md#exploratory-vegetation-rendering).

### Distant forest canopy

The terrain carries a world-anchored forest representation beyond sprite reach,
and fills the missing share in the outer fade band. On the loading worker each 512 ft map
cell samples four of the same placement sites, including slopes, tree lines,
fields, water and coast exclusions. Expected crown area comes from tree density,
selected species height/width and the sprite radius. Colour comes from the actual
selected sprites' visible-pixel means and local foliage tint, quantized into the
live theater palette. Derived pages use the local cache described above.

The shader blends those samples, adds band-limited 76 ft crown mottling, and uses
`1 - exp(-expected_area * density)` for coverage. It fills the complement of
`instance_distance_fade`, using the actual streaming radius and selected detail
distance. Its analytic crown density remains an approximation to capped patches. It receives surface lighting, flare lighting and
atmosphere on the terrain path. Ground-detail Off does not turn off the canopy;
`TORE_VEGETATION=0` and benchmark object suppression do. Desert ground shaders
and their dune/colour rules are unchanged and receive no forest canopy layer.
This is a **fitted agent representation**, not distant 3D trees. Its 512 ft map
can soften narrow habitat boundaries. Colour/coverage matching and the horizon
hand-off still need the lead's GPU captures at 1,000 and 3,000 ft.

### Where things grow

Placement still reads the theater's own palette, ground slope and water map.
The following base grids scale candidate count with the [density table](graphics-options.md#vegetation-texture-and-vegetation-objects). The base scale of 1.0 is now Ultra:

- Trees use a 42 ft base candidate grid, bushes 44 ft and small rocks 52 ft.
  Woodland probability is 0.70 plus 0.25 times the local woodland share.
  Meadow retains a 0.001 background plus 0.045 times woodland proximity and
  patchy copses, with probability up to 0.18 in the existing 1,400 ft noise.
  Hills add up to 0.48 and relief up to 0.18, below the tree line. Slopes fade
  from 32 to 48 degrees to keep cliffs bare.
- Field interiors stay clear. Procedural 1,024 ft square plots, independent of
  retail tile boundaries, carry selected rows at 32 ft spacing. A boundary
  qualifies when both sides are field, or one is field and the other meadow.
  Three quarters of row plants are narrow 42 to 60 ft trees, one quarter bushes.
  A 96 ft mapped-road probe selects rows on a regular 64 ft candidate grid at
  probability 0.80. Village margins carry orchard-like clumps at probability
  0.55 in the next 350 ft outside the existing exclusion. These are fitted
  approximations, not recovered roads or orchard maps.
- Water, streets, town centers and runway exclusions remain clear. Every sprite
  is grounded through `tore_world::grounding`, with scale applied from the base.
- Desert open sand has no trees and retains 0.012 dry-bush probability on the
  44 ft grid. Near water, tree probability is 0.72 on sand or 0.80 on green
  terrain, times squared water proximity. Proximity is full within 300 ft and
  zero at 2,600 ft. The 512 ft water grid accepts one eighth wet coverage so
  narrow mapped delta branches survive. The beach exclusion below overrides this belt and
  the water-distance approximation can still overstate small inland features; river identity is not geographically
  recovered. Wadi banks add 0.10 and beds 0.025 tree probability. Wet-bank
  bushes are green; remote shrubs remain brown and olive. Palms, acacias and
  broadleaf represent the belt; there is no dedicated reed or tamarisk asset.
- Tropical forest uses the 1.15 climate multiplier, capped at probability 1,
  taller broadleaf and palms near water and low coasts. Arctic uses 0.45.
  Tree-line full/zero heights are 4,500/7,000 ft for temperate broadleaf,
  3,800/6,000 ft for temperate conifers, 900/2,400 ft arctic, and 7,500/10,500 ft
  tropical. Bush cover increases near the tree line and fades to zero by
  700 ft above it. High cliffs are bare.
- Instance sizes use independent deterministic placement hashes. Two uniform
  samples make a triangular scale distribution: trees 0.6 to 1.4, shrubs and
  rocks 0.35 to 1.65, with most near 1.0. Woodland-edge trees multiply by 0.78,
  edge shrubs/rocks by 0.8, woodland-interior trees by 1.08 and wet-bank trees
  by 1.12. Width/height ratio varies independently from 0.88 to 1.12. Existing
  species base heights remain. These constants are agent choices implementing
  John's size-variation request of 2026-10-16.


Coast exclusion, **fitted agent rule** for John's 2026-10-17 request: a 512 ft
cell must be at least 75% actual water cutouts, and a 5 by 5 group of such cells
seeds broad water (2,560 ft across). Narrow rivers do not seed beaches. On the
128 ft art grid, light sand within 8,192 ft of that water starts the exclusion.
Sand uses reference-palette luma at least 105, red at least 92% of green, green
at least 90% of blue, and red no more than 8 below blue. The mask then propagates
only toward broad water through all colours. Green patches seaward of sand are
excluded; dark bands behind it remain eligible. Sand far inland is unchanged.
The mask is shared by trees, shrubs, rocks, grass and canopy site sampling.
These thresholds are colour/topology heuristics, not recovered coast geography.

`--vegetation-preview OUT overlay` writes classification pictures. These are
placement heuristics, not claims of original scenery or geographic river data.

Supplied conifer and pine-grove top disks are omitted in both top-card modes. Their
upright sides retain full angular coverage, and all distance/population rules
stay the same. The flat full-width disk through a tapered crown can appear
separated from its tree during the side/top transition. Omitting that disk is
an agent choice in response to John's 2026-10-17 green-cap report. Other species
retain their existing tops. A directly overhead pine therefore has a thinner
silhouette; this tradeoff needs GPU review.

The floating card John reported again on 2026-10-19 was not a top card. It was
part of the supplied pack pine's upright side cell: the source sheet's crop for
the conifer included the red bush's ground strip, which the slicer kept as a
substantial component, so a flat green-brown strip stood above every pack pine
(the conifer fallback when an import has no TREED.SH). At load, every supplied
side cell (v2 pack and Sahara sides, not top views) now drops opaque islands
lying wholly above or below its main body's rows, plus border specks under 1% of
the body. Separate stones, leaves and acacia canopy beside the body stay. This
cleans the conifer, banana and grey shrub strips and a few edge specks; the
rule is an agent choice. The test `side_sprites_have_no_detached_floating_cards`
covers every side cell. The cache key includes a hash of the source tree, so
cached cards rebuild.

### Tile seams

John requested a 500 ft transition centred on colour seams on 2026-10-18.
This is **opinionated presentation** on `exp/vegetation`, separate from the
[source import audit](../baselines/terrain-seams.md). The following mask,
colour threshold and tree-density choices are **fitted agent decisions**.

At an accepted tile edge the local linear-light means still follow a smoothstep
across exactly 500 ft, zero correction at 250 ft on either side. Both ground
textures also contribute inside that band. The neighbour is sampled at the
reflected distance into its own tile, with its actual uploaded page and quarter
rotation. Each sample's local mean is removed before mixing the texture detail;
the unperturbed mean ramp is then retained. World-space noise shifts the detail
boundary by at most 80 ft, tapering to zero at both outer endpoints. Its 512 ft
noise cells are stable in the world, precomputed at segment centres and
interpolated along the edge. These constants are fitted agent choices
for John's texture-blend request of 2026-10-18. Texture coverage and terrain
heights are unchanged. The same palette remaps and footprint filtering apply
to each texture. Adjacent accepted edge means interpolate along the edge.

Source pictures, including LAND, remain unchanged. The reviewed UKR camera
has mottled LAND.PIC south of the seam and a mostly uniform UKR23.PIC north of
it. Replacing LAND with a fit to the flatter named tiles removed existing
contrast. That global replacement is removed. Already mottled interiors stay
pixel-identical. John approved adding mottling inside flat named tiles on
2026-10-18, an **opinionated** extension. Original source pages remain intact.

Flatness is measured per imported picture in linear light, using green ground
blocks averaged at 256, 512, 1,024 and 2,048 ft. At least 60% of the picture must
use the natural green palette ramps; city art is excluded. A picture qualifies
only when its standard deviation at every scale is below half the reference's.
The reference is mottled LAND when available, otherwise the most varied eligible
picture. Each flat picture uses its most common adjacent mottled picture with
at least 70% of that reference contrast, falling back to the reference. These
thresholds and the following synthesis rules are **fitted agent decisions**.

A stable world-space noise field supplies broad light/dark modulation, with
no tile-local seed or tile-spacing repeat. Its scale and small-scale share are
fitted to the reference's four measured deviations. Candidate primary scales
are 512 to 8,192 ft; a second octave is four times smaller. The reference's
linear mean and RGB/luminance covariance set the light/dark colours. A cached
96-square correction patch per placement modulates the original fine texture;
it never replaces the source pixels. Existing painted features retain their
source colour. This is a statistical match, not a copy of neighbouring artwork.

Flat modulation fades from zero at the edge to full strength 250 ft inward,
alongside the existing 500 ft texture blend. Adjacent flat placements sharing
the same reference use one continuous world field with no artificial edge fade.
Water, roads, sand, rock, snow, brown fields and city art are protected. A green
field must have a coherent 3 by 3 texel core to be protected: Baltic fine grain
crosses the older shade-based Field threshold and is not a painted field.
A 512 ft guard protects that core and other source features from filtered
modulation. Coast checks extend 512 ft, airport bounds 860 ft and town circles
640 ft. Active city-fill tiles receive no modulation. These guards include the
345 ft interpolation support. Narrower-than-texel features remain unresolved.

The texture/colour response is precomputed into a sparse half-float atlas.
It stores a linear RGB correction and the source luminance at 128 ft positions
along accepted runs and nine depths from 0 to 250 ft on each side. Source pages
and rotations feed the same noisy 500 ft reference blend. A shader mask lookup
rejects excluded segments; one hardware-filtered response sample supplies the
correction. A hard distance test returns zero at and beyond 250 ft. Run endpoints
also fade inside their accepted segment, preventing filtered spill into a
protected feature. This seam response remains exactly zero outside the band;
only separately classified flat placements receive interior modulation. The
cache trades extra atlas memory for fewer shader fetches.

The atlas uses the theater's reference palette. Its correction scales with the
live material's luminance before surface lighting. Uniform brightness changes
are preserved; nonuniform weather palette remaps are a fitted approximation
requiring visual review. The atlas, mask and metadata are cached. There are no
per-frame uploads or new ordinary draw calls. A per-page vertex flag skips flat
modulation on mottled pages. Active flat fragments use one tile-record load and
one filtered response sample, without shader noise or neighbour-art sampling.
A diagnostic mask view and fixed unlit interior cameras are included in the
lead's offscreen harness. No GPU timing
claim is made by the CPU audit; the 4K High target is about 0.5 ms at 100 ft.

The source is the actual uploaded ground page, including LAND fallback and
neutral airfield ground. A maximum linear RGB local mean difference of at least
0.015, differing sampled texture detail on a different page/rotation, or an
accepted LAND boundary qualifies.
Matching means therefore no longer hide texture-only discontinuities. One shared mask samples both sides of every 128 ft edge
segment, at depths 64, 192 and 320 ft and at three positions along the edge.
Water, river, beach, road, rock, snow, sand, city tiles/sites/streets and airport
footprints reject the segment. Airport bounds expand by 348 ft; town circles
expand by 128 ft. Same-class grass, woodland or scrub can blend. Fields and
different authored land classes stay crisp. At a boundary with generic LAND
fill, matching T2 colour/class values and a green fallback palette ramp permit
blending into authored grass, forest or scrub. The fallback's light green can
classify as Field by shade alone; it is not an authored crop boundary. Authored
Field samples remain excluded. This is a fitted continuity safeguard for
John's 2026-10-18 review, not a new interpretation of T2 classes. These are
colour heuristics, not recovered land-use data. Narrower-than-texel features remain unresolved.

Accepted runs fade inside their first and last 128 ft segments. A 384 ft guard
at tile corners preserves junctions and prevents overlapping corrections.
Those short junctions intentionally retain their source colours. The old
brown-only fade toward T2 colour over 1,147 ft per side is removed.

A differing field/forest pair instead marks a tree belt, with no ground colour
mix. Within 250 ft of the boundary, tree probability rises to at least
`0.85 * belt`, where belt is one at the seam and smoothly reaches zero at the
outer edges. Existing tree-line, slope, grounding, exclusion and fixed-budget
rules remain. Field interiors and procedural rectangular field rows remain.
The distant canopy samples the same belt through ordinary placement queries.

Blades use the same source page, flat modulation and cached texture/colour
response as ground, before their existing species/root and tip tints. Fallback land now uses the same LAND page as the terrain; water
without a placed picture still has no blades. The seam metadata and masks are
cached with the placement/canopy/habitat inputs. `TORE_TERRAIN_SEAMS=0` disables
flat modulation, the texture/colour response and belt for comparisons. Ground-detail Off leaves
the correction active; `TORE_VEGETATION=0` disables this exploratory feature.
No simulation, wire, replay or checkpoint data changes.

### Grass

John requested GPU-generated blades on 2026-10-17. This is an **opinionated**
presentation addition. All geometry, density, habitat resolution and wind
constants below are **agent choices**, pending GPU review. Original grass
behaviour remains unknown. No third-party shader code is used.

`TORE_GRASS=blades|tufts|off` overrides the saved Graphics Grass row. Blades is
the branch default. Off removes grass geometry only, retaining ground detail,
trees and canopy. Tufts retains the earlier five-blade cutout sprites, patch
placement, ring density and uploads for direct comparisons.

Blades are opaque three-triangle strips with five generated vertices. There
are no alpha tests, CPU blade placement or grass chunk uploads. Each frame a
GPU pass decides every candidate cell once and lists the blades that grow; the
strips are drawn from that list (S13b, 2026-10-19; the same pixels as deciding
in every vertex, which remains the path for adapters without compute). A world-cell integer hash controls position jitter, height, width,
heading, lean and colour. At High, four grids have 1, 2, 4 and 8 ft spacing.
Every second cell in both axes belongs to the coarser grid, including inside
the fine region. Each world cell has one persistent owner, without duplicates
or relocation as the camera moves. Grids snap to their spacing. Guard cells put
entering and retiring rows beyond their visible disk. Density and distance are
independent Graphics rows, with budgets in the
[Graphics table](graphics-options.md#vegetation-texture-and-vegetation-objects).
These denser grids, 500 ft default and controls implement John's request of
2026-10-17. The particular lattice, size compensation and fade rules are agent
choices, not claims about original grass.

Each grid stays at full distance size to 70% of its horizontal guard radius,
then smoothly shortens and thins to zero. A separate distance from the eye to
the grounded root stays full to 70% of the selected distance, then fades to zero
at that distance. The smaller coverage wins. Thus 500 ft means 500 ft from the
eye, including altitude, and the finest grids do not vanish simply because the
eye climbed above their horizontal radius. Seeded ranks stagger shortening.
Width increases smoothly by up to 80%, height by up to 35%, between 40 ft and
the selected endpoint. This compensates slightly for small distant blades;
the outer fade still reaches zero. Ground detail continues underneath.

The loader caches two bits of habitat per 128 ft art texel: bare, meadow, crop,
or short. Water, river, road, sand, beach, rock, snow, pavement and town exclusions
remain bare. Art, coast and city masks are sampled at their aligned 128 ft texels.
Town circles and whole airport/runway footprints are rasterized only within
their bounds, with a 91 ft margin, the half-diagonal of a habitat texel. Airport
footprints also retain a conservative 220 ft pavement buffer. These are fitted art classes and
exclusions, not surveyed land use;
sub-texel exclusion features remain limited by that resolution. Desert grass
requires eligible green/crop/scrub land within 2,600 ft of mapped water. Sand stays
bare even beside water. Egypt's mostly sand-classified riverbanks can therefore
have no blades where the old tufts grew dry straw. The Egypt comparison must
show this difference, not silently invent an oasis.

Meadow/crop/short density is 1/0.9/0.35 of candidate cells. Existing canopy cover
further reduces density continuously toward 25%. Slope above about 42 degrees
is bare. Blade heights before seeded 0.75 to 1.25 variation are 2.1/1.6/1.2 ft;
tropical height multiplies by 1.5, desert/arctic by 0.65. Base width is 0.20 to 0.40 ft before distance compensation.
Roots use the shared `tore_world::grounding` bottom correction and interpolate
the same terrain triangles as `Terrain::height`. Both root corners follow that
plane. The ground's indexed picture and live theater palette tint each blade,
with straw tint for short/arid grass and upward-biased normals. Colour scales
from 0.55 at the root to 1.15 at the tip, and lighting occlusion from 0.6 to 1.

Wind uses simulation time, two world-space waves at roughly 570 to 700 ft
wavelengths and mission wind direction. Smooth 512 ft value noise, moving at
5.12 ft/s along that direction, modulates phase and scales amplitude by 0.65 to
1. Calm sway is at most 6% of blade height at the tip; amplitude adds 0.4% per ft/s of wind, capped at 60 ft/s. Seeded static lean
adds at most 22.5% per horizontal axis. Bending grows quadratically from the
root. Paused simulation pauses sway. This has no gameplay or collision effect.

### Desert ground

The desert texture is meant to read as sand at every height, not as
one repeating ripple:

- **Colour fields**, miles across: ochre, red and pale sands, grey
  gravel plains (serir), dark varnish on stony ground, pale silt in
  meandering wadi beds about 150 ft wide with darker banks.
- **Dunes**: in sand seas a few miles across, transverse ridges about
  1,150 and 1,750 ft apart and up to about 80 ft high, with a long lit
  windward slope, a sharp crest and a shaded slip face. Barchan
  crescents, a few hundred feet across, sit on the sea's edges. They
  are shading on the lighting, not geometry.
- **Close range**: wind streaks along the wind, and ripples 24 to 40
  ft apart in three directions, only on sand sheets and windward
  flanks, soft enough that the dune's own light still shows. Slip
  faces stay smooth. Soft grit, and pebbles about a foot across on
  gravel. Open sand carries sparse dry scrub. Legacy tufts retain dry straw on wadi
  banks, gravel and water belts; blade habitat follows the stricter rules above.

Each pattern fades out before it is smaller than a pixel. The fine
grain is full below about 2,200 ft and gone by about 7,500 ft; the
colour fields and dunes reach two to six times the texture distance
and fade out between about 9,000 and 26,000 ft.

### Lights

Flares, afterburners, fires and gun flashes add light to the ground
detail and never remove it. Measured on the Ukraine chase view (the
mean difference of each pixel from its 3 by 3 neighbourhood, lower half
of the picture), the ground detail is 1.001 times as strong with an
afterburner's light at 300 ft and 1.049 times at 40 ft, where the light
reaches the ground.
