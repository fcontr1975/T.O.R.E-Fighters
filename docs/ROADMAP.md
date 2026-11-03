# Fighters Anthology Rebuild: Roadmap

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

T.O.R.E-Fighters in the Repo - Tasteful Opinionated Reverse Engineered

## Contents

- [Current scope](#current-scope)
- [Principles](#principles)
- [What "1:1" means](#what-11-means)
- [Milestone 0: Spec and salvage](#milestone-0-spec-and-salvage)
- [Milestone 1: Faithful quick fight](#milestone-1-faithful-quick-fight)
  - [1a. Shell and menus](#1a-shell-and-menus)
  - [1b. Original terrain](#1b-original-terrain)
  - [1c. Free flight](#1c-free-flight)
  - [1d. Quick fight loop](#1d-quick-fight-loop)
  - [1e. AI](#1e-ai)
  - [1f. Sensors and weapons](#1f-sensors-and-weapons)
  - [1g. Installer and first-run import](#1g-installer-and-first-run-import)
  - [1h. Cheats](#1h-cheats)
  - [1i. Aircraft pass](#1i-aircraft-pass)
- [Milestone 2: Multiplayer](#milestone-2-multiplayer)
- [Milestone 3: Missions and campaigns](#milestone-3-missions-and-campaigns)
- [Milestone 4: Tools](#milestone-4-tools)
- [Milestone 5: Remaster layer](#milestone-5-remaster-layer)
- [Milestone 6: Persistent campaigns](#milestone-6-persistent-campaigns)
- [Milestone 7: Custom maps and community theaters](#milestone-7-custom-maps-and-community-theaters)
- [Importer coverage table](#importer-coverage-table)
- [Funding](#funding)
- [Open decisions](#open-decisions)

## Current scope

Development baseline: see [DEVELOPMENT.md](DEVELOPMENT.md), [ARCHITECTURE.md](ARCHITECTURE.md), and [recorded validation](baselines/environment.md). The first M1a [main-menu slice](baselines/main-menu.md) now imports original menu assets and runs natively. M0 research and the remaining M1a screens/audio work remain in progress.

This is the sequencing document for the ground-up rebuild in Rust. Current status
and the next feature are tracked on one page in [the parity plan](parity-plan.md).
The behaviour being rebuilt is described in prose in [docs/spec/](spec/); how
agents work from those specs is in [AGENTS.md](../AGENTS.md).

The existing TypeScript repo /USNF-ATF is the guide, not the gospel: its format docs, decoders, recovered geometry, audio recovery, and baselines are the reference material. Its engine is not being ported. Original terrain and environment systems are rebuilt from retail assets and from the game's observed behaviour; USNF-ATF's custom terrain system and DEM-based theaters are not being ported. Further menu screens are deferred until explicitly scheduled.

Current state (2026-09-17): original menus, the quick-mission creator and
ordnance screen, all 16 theaters, and twelve aircraft in free flight with
cockpit, HUD, instrument windows, weather and controller support. The
[ported roster](spec/ai-experience.md#currently-ported-aircraft) identifies all
twelve and their AI family bindings. A development
weapons range supports manual weapon testing. Ground contact and landing are
authored behaviour ([opinionated](behavior-provenance.md)); combat AI has
spec-derived components and a partial Quick Mission hookup. John requested aircraft and surface AI research
and planning on 2026-09-17; the 2026-09-22 M1 air-to-air plan now prioritizes
awareness, search and mission engagement. Current stages are in M1e below.
On 2026-10-10 John authorized surface AI and autonomous surface firing: the
Quick Mission's ground targets, their AAA, flak and SAM defenses and the base
layouts' air defenses, built that day in the
[surface objectives round](#surface-objectives-and-air-defenses-2026-10-10).
AI wings attacking ground targets stay unauthorized until the
[next rounds](#next-rounds-after-the-surface-round).

John scheduled the shoreline correction and ocean-motion trial on 2026-09-16.
See [ocean behavior and visual scope](spec/ocean.md). This bounded visual
work precedes the following order without authorizing additional systems work.

John requested initial F-14, A-4E and X-31 ports on 2026-09-16, then specified
Fighters Anthology sources throughout. F-14D, A-4E and X-31 EFM now have initial
ports through the [aircraft import workflow](aircraft-import.md). See
[acceptance and remaining limitations](baselines/aircraft-fa-expansion.md).
USNF-ATF is a research guide only. Continue maneuver audio/rumble, remaining
flight-response and weather work in the [parity plan](parity-plan.md).
No default adapter change or AI scope is included.

The preceding execution order (2026-09-14) required manual weapons, sensors and
damage acceptance for F/A-18D and Rafale C before AI work. John's 2026-09-17
request scheduled M1e research, followed by partial live aircraft hookup. The
2026-09-22 request prioritizes its air-to-air gaps; existing straight-flight
weapon fixtures remain available. See
[current systems evidence and remaining gates](baselines/weapons-systems.md).
The manual range covers both aircraft's ten PT-default weapon slots, partial
ECM/player-damage integration and controller feedback. This does not close the
M1d/M1f acceptance gates. The later aircraft ports are part of the current
twelve-aircraft scope; their individual acceptance limits remain documented.

## Principles

1. **Faithful first, opinionated second.**  Milestones 1 through 3 reproduce the retail game with tasteful opinions.  No feature will be left behind, and some opinionated features will have retail settings you can turn on to return back to that original experience.
2. **Bring your own copy.**  The repo ships no retail bytes.  The importer reads the user's own Fighters Anthology media at runtime and writes to app data.  A signature scan for EALIB, PIC, and other retail markers stays a release gate.
3. **Hand-rolled where it counts.**  External dependencies are kept to a minimum.  Formats, synth, terrain, and sim are ours.
4. **Importer grows with the game.**  There is no "import everything" phase.  Each step decodes exactly the formats the next playable piece needs.  Breadth is tracked in a coverage table, not a milestone.
5. **Cross-platform from day one.**  Linux, Windows, and macOS build and run at every milestone.  No platform is "deferred" this time.
6. **Baselines are recorded, not remembered.**  Every milestone writes its measurements and acceptance evidence to `docs/baselines/`.
7. **Deterministic and headless from the start.**  Massive battles and live campaigns are the reason for the rewrite.  The sim runs without a renderer and produces identical results from identical inputs from M1 onward.  This is a constraint, not a feature.

## What "1:1" means

Parity is measured **by expression of feature**: the player must experience what
they experience in Fighters Anthology. It is not a recreation of the original
program's code, control flow or internal structure. The test for whether
something belongs in a behaviour spec is "would a player notice if this were
different?"

In scope for parity:
- Aircraft flight envelopes from retail PT data
- Weapon, sensor, and object stats from retail OT, JT, and NT data
- AI behavior and pursuit tactics
- Mission logic, triggers, scoring, and campaign progression
- Menus, screen geometry, art, fonts, and audio
- Original theater terrain from retail T2 data

Out of scope for parity:
- Pixel-accurate rendering
- Original resolution, frame timing, or integer clock artifacts
- Bugs that are not load-bearing for gameplay
- The original program's control flow, call ordering, caches and RNG ordering

AI behavior is recreated from a behaviour spec, like every other feature; the
retail AI bytecode VM is not reimplemented. This follows from parity by
expression of feature and shapes M1e.

---

## Milestone 0: Spec and salvage

**Goal:** know what is being rebuilt before writing engine code.

Work:
- Inventory the TS repo and mark every artifact as *spec* (format docs, byte layouts, recovered DLG geometry, MUS scripts, PT field maps, baselines) or *implementation* (engine code, React shell, Three.js render).  Spec carries forward.  Implementation is reference only.
- Inventory the full Fighters Anthology disc layout: USNF '97, ATF Gold, NATO Fighters, Marine Fighters, and the Pro Mission Creator.  Produce a format-by-title census.
- Write the 1:1 definition above into `docs/` and get it settled. **Done:** see
  "What 1:1 means" above and [AGENTS.md](../AGENTS.md).
- Decide the AI VM question. **Settled 2026-09-15:** behaviors are recreated from
  a spec, not reimplemented from the recovered bytecode.
- Set up the Rust workspace, three-platform build, and the retail signature scan.

Deliverable: an app on all three platforms that opens a window, prints its renderer, and passes the scan.

Exit: census committed, 1:1 definition committed, AI VM decision recorded.

---

## Milestone 1: Faithful quick fight

This is the first playable milestone: original menus, original terrain, the
current twelve-aircraft roster with basic systems, and guns and missiles quick
fight against AI. The original four-aircraft target has expanded with the ports.

### 1a. Shell and menus

Work:
- Importer: ESA, EALIB, DCL, PAL, PIC, FNT, DLG, MNU, LAY.
- Main menu, quick fight setup, aircraft selection, loadout screen at recovered geometry with retail art, fonts, and title music.
- Pause bar, settings, volume mixer, briefing and debrief screens as shells.
- Original recorded PCM music with bounded MUS scheduling. User decision 2026-09-14: use FA recordings; MIDI conversion/synthesis is excluded from this slice.

Deliverable: every retail menu screen navigable with the correct art and audio.

Exit: side-by-side comparison against retail screenshots recorded in the baseline.  Title and menu music plays from recorded game data with no synth.

### 1b. Original terrain

Sixteen base theaters are selectable (59 retail variants load through the
developer option `--theater ~CODE`) with original T2
heights, named/numbered artwork, weather and visible static objects. The expansion
below replaces the initial two-theater preview target; dynamic source behavior
remains explicitly bounded. No real-elevation pipeline
or reference-checkout terrain engine is part of this milestone.

### Retail terrain detail review

**Implementation mode. John selected retail detail for all maps on
2026-09-23:** textures, artwork, scenery and map variants. Surface shaders and
expanded landscapes are future work under [M5](#future-terrain-enhancements).
No surface AI is authorized by this visual scope; John authorized it
separately on 2026-10-10 ([surface round](#surface-objectives-and-air-defenses-2026-10-10)).

[Measured findings](baselines/retail-terrain-review.md),
[behavior specification](spec/terrain-detail.md), and
[material contract](formats/terrain-materials.md) own the evidence. Complete
usable source coverage without adding invented scenery or new terrain geometry.

#### Delivery and remaining scope

Implemented in the separate retail-terrain worktree:

- All sixteen base theaters and 59 retail MM variants are imported; the picker
  offers the base theaters, the variants are a developer option (`--theater ~CODE`). Variant scenes
  preserve their source identity and use the documented fitted composition rule.
- Kurile's 236 named images, both source resolutions, numbered artwork and
  original land fallback pictures render with live weather palettes and water
  cutouts. The fallback's four-cell repeat scale is fitted.
- Main-shape geometry for every visible placement in the 75 layouts projects,
  including the formerly unsupported launcher poses. Large scenery images keep
  every source pixel through paging, and reviewed line primitives render.
- The shared importer and CLI follow layout dependencies. A cache-version marker
  requires re-import of older bundles. The headless map validator checks every
  imported layout without starting flight or autonomous behavior.

Remaining original-behavior research: exact generic land-plane scale/projection,
class-dependent visual rules, distance-dependent shape/terrain presentation,
dynamic and destroyed scenery appearances, runtime decals other than the
weapon and crash craters ([explosions](spec/explosions.md)), and live campaign
progression/destruction persistence. The selectable variants are static source
layouts, not a campaign engine. Missing TREE1/TREE2 resources do not authorize
invented vegetation. These boundaries are explicit in the behavior spec.

#### Acceptance

Use synthetic fixtures for named/numbered placement parsing, both image sizes,
border coordinates, masks, dependency failures, image paging and variant
selection. Retain source terrain heights, object positions, runway alignment,
weather palette changes and deterministic 120 Hz simulation. No retail-derived
fixtures are committed.

Construct and render every imported layout. Inspect matched close and distant
views of Kurile coast, Ukraine land/cities, Pakistan high ground and desert/river
scenes. Check the `--theater ~CODE` variant option and ground-start identity. Record frame-time
comparisons using the same camera/settings and state their scope. Run the
repository checks and display smoke after rebasing. [Measured validation](baselines/retail-terrain-review.md#implementation-validation)
records actual results and platform limits. Retail side-by-side comparison is
unavailable and is not an acceptance blocker or an implied parity claim.

The selected work is retail map detail. Shader enhancements and authored
landscape expansion remain future M5 items; flight adapters, controls and
compatibility modes stay available.

### Airport and ground-object expansion

**Implementation plan, requested by John on 2026-09-20.** The deliverable is a
playable airport world in TORE: source-positioned runways and surrounding objects
for every map, individually targetable ground structures, and player landing/tower
radio commands with landing guidance. Ukraine is the first complete slice.
Research serves implementation decisions. It is not a requirement to reconstruct
the original engine, scheduling or every callback before shipping.

[Behavior and fitted decisions](spec/airports.md),
[placement/resource contracts](formats/airport-placements.md),
[measured input coverage](baselines/ukraine-airports.md).
Current implementation: base-layout import, static rendering/targets, runway
surfaces, player tower commands, ILS and replay are connected. Independent review
fixed target reset, source classes/signatures, altitude reference and rendering.
The base-theater slice is implemented; campaign overlay/generated terrain handling,
unsupported shape programs, additional tower speech, speed brackets and target
camera imagery remain open. See the linked baseline for validation and limitations.
Ground start is now connected to the creator's player start and airport selectors;
other wings remain airborne and restart preserves
the accepted start. [Behavior](spec/quick-mission-menu.md#player-ground-start).

These linked documents own constants and evidence; this section owns execution.
All proposed type/file names below are agent design choices, not existing APIs.

#### What we can build on

| Existing TORE component | Reuse | Required extension |
| --- | --- | --- |
| `tore-formats::theater::Environment` | Bounded top-level M/MM environment parsing | Separate object-block reader; side tables and both nationality forms |
| `tore-formats::aircraft::Brf` and OBJECT schema | Inert OT token/field parsing | General static object definition, explicit resources and reviewed scalar fields |
| `tore-formats::strip`, SH contact boxes | Reviewed STRIP identity and runway anchors | Review all thirteen airport definitions and their actual shape contracts |
| `tore-import` (was `tore-app/src/assets.rs`), `tore-extract` | Archive provenance, runtime imports and caches | One shared transitive object dependency resolver; import report/cache version |
| `terrain::Terrain` and `Terrain::surface` | Source terrain, weather, grounding queries | Scene construction and runway surface query integration |
| `tore-sim::combat::live::Target` | Stable IDs, ground roles, hit points, sensors and damage classes | Per-object configuration and building/runway contact geometry |
| `combat.rs`, `sim_renderer.rs`, HUD/instruments | Current target display, geometry submission, palette and shadows | Static meshes, ground-object identity, ILS readout and airport selection |
| `audio.rs`, `tore-formats::radio` | Serial recorded speech playback and reviewed phrase mappings | Airport response events; only independently reviewed tower phrases |
| Pilot input and `combat_tape.rs` | Fixed-tick commands, replay and resource fingerprints | Airport selection/commands, scene identity and damage/clearance replay |

The aircraft renderer currently uses an aircraft-specific scale. Do not reuse it
for ground objects without measurement. Combat presentation also contains ID-to-
aircraft-index assumptions; remove those assumptions at the shared scene boundary
before adding ground targets. Current projectile tests use swept spheres; long
runways and buildings need dedicated geometry rather than inflated target radii.

#### Runtime shape and ownership

Create an immutable imported scene containing definitions, placements, dependency
identities and reviewed airport geometry. Keep GPU meshes/textures in the app.
Keep mutable target health in the existing combat state, with a stable mapping
from source placement to target ID. Keep airport selection, availability and player
clearance in a renderer-independent `tore-sim` airport service. Do not create two
independently writable health copies; derive airport availability from combat
changes. A destroyed building remains identifiable for mission goals and replay.

Use source layout identity plus record ordinal as the stable import key. Retain
signed source aliases separately; they are not globally unique object IDs. Allocate
runtime IDs deterministically through the existing ID owner, disjoint from player
and aircraft IDs. Ground objects do not consume aircraft roster indices.

Per fixed tick: apply recorded player commands, advance existing flight/combat
with shared world-contact inputs, deliver damage/contact events to the airport
service, and publish a readout plus radio events. Rendering interpolates poses
and reads that snapshot. Audio playback never changes clearance timing. Build a
new scene completely before switching theaters; failed loads leave the old scene
intact. Flight reset reconstructs target and airport state from the same scene.

#### Slice A: bounded importer and Kiev visible world

**Prerequisites:** existing Ukraine T2/MM and reviewed initial assets are available.

- Add object parsing alongside Environment rather than mixing mutable world state
  into terrain metadata. Preserve position, orientation, name delimiters, flags,
  speed, aliases, side tables, source order and unknown fields. Enforce existing
  mission size limits plus explicit per-record/token bounds. Report malformed
  records with resource and line context. Never execute mission scripts.
- Add a general static OT reader using the existing schema. Expose explicit main,
  damaged/shadow references only where their meanings are established. Unknown
  callback names remain inert metadata. Keep the narrow STRIP diagnostic intact.
- Share dependency discovery between CLI and app import. Traverse explicit OT/SH
  references with cycle detection and bounds; retain exact archive provenance.
  Missing resources name both the missing resource and referring object. Reimport
  old caches when needed, with a clear diagnostic rather than invisible buildings.
- Implement reusable static shape mesh/texture loading without aircraft rigs.
  Apply one reviewed scale and placement transform to visual and contact geometry.
  Batch shared type geometry and cull by camera bounds; avoid duplicating a mesh
  for every instance. Reuse source palettes, lighting, cutouts and shadows.
- Render Kiev's runway, four hangars and tower at their source coordinates. Inspect
  low-altitude views from both runway ends and an overhead view. Compare numerical
  transforms to the source, not modern geography or a nearby terrain-texture image.

**Exit:** six expected instances, correct names/types/transforms, no unresolved
required visual resources, no runway buried in terrain, no ID coupling to aircraft.
Synthetic tests cover truncation, signed angles/aliases, unknown tokens, duplicate
fields, resource cycles and transform consistency. This is a visible scene slice;
landing and target operation arrive in subsequent slices.

#### Slice B: complete Ukraine placement and every-map import

**Depends on A.** Expand Ukraine to all fourteen runway records and the 99 objects
in airport-labeled sections. Preserve the other 144 source objects in the scene
manifest even when outside this airport-focused rendering scope. Keep association
as metadata; it must not cause collective damage or ownership changes.

The census already extracted all 75 MM layouts and thirteen airport definitions.
Review each definition's shape geometry, scale, collision records and dependencies.
Treat STRIP3A/5A/6A/7A as independent placed instances, not damaged versions or
additional airports inferred from their names. Review their relationship to the
other runway pieces before exposing airport groups in the UI.

Implement `sides`/`sides2` and `nationality`/`nationality2` using their documented
conversion. Inventory airport-associated objects in every base layout. Prefer
explicit relations, then source authoring groups; ambiguous records remain
unassigned and appear in the audit. Retain/render ambiguous static placements in
the full scene rather than silently excluding a possible airport building. If
that requires non-airport static types, process them through the same loader.
Mobile/armed objects may have static placement/presentation, with no new behavior.

Run a coverage pass over all sixteen base theaters, including all thirteen runway
types. Resolve mission overlays separately: prove whether a source supplies a full
layout or patches it, how signed aliases replace/delete records, and how generated
`~` terrain names resolve. Never append two full layouts or redirect a campaign
alias to a base terrain without evidence. Unsupported overlays get explicit errors.
Campaign resolution remains in the final every-map scope, even if base maps ship
first; do not call base-only support complete.

**Exit:** every expected airport/associated object accounted for by source key;
per-layout counts and dependency manifests reconcile; no unresolved required
placements on accepted maps. Inspect every Ukraine airport and one view per other
base theater plus each runway type. Numerically validate every placement even
when visual checks are representative. Repeated import is unchanged and missing
resources/ambiguous overlay operations are actionable errors.

#### Slice C: runway surfaces and individual ground targets

**Depends on A; finish Ukraine first, extend through B's shared definitions.**

Add an airport surface query using reviewed extents/anchors and the fitted support
policy in the spec. Wire it through all applicable Terrain surface consumers so
flight, target grounding and presentation agree. Building contact uses a separate
solid-object query, not roof height returned as terrain. Verify both runway ends,
edge transitions, taxi exits, gear contact and off-runway terrain. Preserve legacy,
hybrid-default and restricted native-table adapter contracts and report which
surface interactions each supports.

Decode each OT's health, class and sensor-relevant fields. Construct zero-velocity
ground targets with their own configuration. Use the current designation/sensor/
weapon eligibility pipeline and target window. A control tower, hangar and runway
must be distinct targets. Keep target identity after vector compaction/removal.

Extend projectile contact with segment-versus-oriented-box or reviewed contact
volumes, preserving earliest impact against terrain/objects and distinct fuze
radius handling. Do not change aircraft hit volumes in this slice. Reuse damage
classes, hit records and effects; connect destruction to airport availability by
one event. Add a target-destroyed notification usable by future mission goals,
without implementing the mission scripting engine here.

**Exit:** designated tower is the object hit; adjacent structures remain unchanged;
long runway geometry cannot intercept shots far outside its footprint; one
threshold-crossing destruction event is emitted; destroyed objects do not respawn
on camera changes. Synthetic deterministic tests cover high-speed tunneling,
nearest impact, collision/terrain ties, repeated hits, reset, IDs and replay.
Conduct a manual ground-attack and runway-contact pass on Ukraine.

#### Slice D: player landing guidance and tower radio

**Depends on C for runway availability; UI can develop against synthetic scenes.**

Implement typed `SelectAirport`, `RequestLanding`, `RepeatReply` and
`CancelApproach` actions, with one player clearance record and explicit response
reasons. Resolve input bindings against `tore-input`/app shortcuts before assigning
keys; expose configurable controls without taking an existing binding silently.
Use the existing HUD/font/menu pieces for airport choice and radio text.

Follow the spec's command and ILS rules. John's requested activation height is
4,000 feet above airport ground level, including the threshold. Recovered retail
evidence can refine other fitted rules without overriding this user choice. Requesting clearance sets guidance, not autopilot. Existing
manual flight and autopilot controls retain their behavior. Land completion comes
from actual contact and the fitted completion condition, not proximity alone.
The service invalidates clearance if its runway becomes unusable. Changing airport,
repeating a request, canceling and starting a new flight have explicit transitions.

Publish threshold/end, bearing/range, clearance status, localizer/glide deviation
and optional aircraft-specific speed brackets in a HUD readout. Test the published
values independently of pixel rendering. Feed typed replies to existing serial
radio playback and subtitles. Extend phrase extraction only for proven text/sample
pairs. Do not repurpose wing formation phrases as tower dialogue.

**Exit:** select an airport, request and receive an appropriate reply, fly an
indicated approach, touch down and taxi manually. Repeat/cancel/reset and runway
loss behave deterministically. Tests exercise ILS distance/altitude boundaries,
gear/NAV gates, both ends, off-axis/behind-threshold cases and zero-distance math.
No audio device or missing sample prevents the same clearance result. Replay
reproduces selection, replies, target damage, guidance and landing completion.

#### Research tasks bounded by implementation decisions

Do these within the owning slice. Stop once a prose rule can be written. If the
consumer remains unresolved, ship the documented fitted rule where available and
keep the evidence gap visible.

| Decision | Retail evidence to inspect | Implementation action |
| --- | --- | --- |
| Shape scale and runway extents | SH transform/header consumers and contact records for all thirteen types | Measure per type; inspect rendered/contact overlap before acceptance |
| Compound airport layouts | MM records, explicit aliases and STRIP variant geometry | Preserve all records; group only confirmed relationships |
| Allegiance/overlay semantics | Mission parser branches and actual base/campaign layouts | Typed conversion/patch rules; reject unresolved operations rather than guess |
| ILS activation and symbols | HUD consumers near airport selection; manual pages 67/87 disagree | Keep John's 4,000-ft airport-relative choice; research remaining gates/symbols; test the boundary |
| Radio request/reply repertoire | Player menu/input producers, APCommentProc, speech tables and sample mappings | Use the opinionated minimal command set with text fallback |
| Targeting and damage | Relevant OT fields, sensor eligibility and damage-class consumers | Reuse current combat model with per-object inputs and labeled contact approximation |
| Runway/tower destruction effects | Airport availability queries after object damage | Use the spec's independent service policy until stronger evidence exists |

The nine slots in the earlier STRIP template are evidence for that template, not
a universal capacity requirement. NPC traffic, holding patterns, autonomous taxi,
automatic player landing, capture, repair/rearm/refuel and a mission editor are
not required to deliver this plan. Existing AI must keep working alongside ground
objects, but this plan adds no autonomous behavior.

#### Release and verification

Ship coherent increments A, Ukraine B/C, D, then complete B's every-map expansion
and rerun C/D against all airport types. The every-map task remains open until the
coverage report closes base and supported retail mission/campaign layouts.
Do not label unsupported layouts as empty or successful. Record visual, interactive
and headless acceptance separately. Keep all retail-derived meshes/captures local.

Run the repository's required formatting, Clippy, locked tests/build, Python tests,
asset guards and documentation checks for each completed implementation change.
Rendering changes also require the real display smoke test and inspected captures.
Exercise Linux now; record Windows/macOS runtime validation as performed or pending.
Measure startup time, static mesh memory and frame time with airport scenes before
and after on the same host; optimize measured regressions without reducing object
coverage silently. Do not impose a speculative performance budget in advance.

Update affected feature-matrix rows and guides for extraction, objects/shapes,
theater, architecture, input and flight controls as each behavior lands. New replay
records get an explicit version and compatible old-tape handling. Milestones are
reported in the session. No commit, push or default-adapter change is authorized
by creation of this plan.

### 1c. Free flight

Aircraft: all twelve in the [ported roster](spec/ai-experience.md#currently-ported-aircraft).
F-14D, A-4E, X-31 EFM and F/A-18D were the original target; Rafale C,
MiG-29, Su-27, MiG-21, Su-25, MiG-23, Su-35 and F-22A are also in the port.

Work:
- Importer: PT, SH, and the cockpit and HUD assets for every ported aircraft.
- Fixed-rate sim loop, decoupled from render, headless-capable.
- Flight model from PT.  Engine, gear, flaps, hook, brakes, throttle, afterburner.
- Cockpit view, HUD, external and chase cameras, control surface animation.
  The [retail view suite](spec/flight-views.md), reference modifiers and Other View
  are implemented with fitted placement; [validation](baselines/flight-views.md).
- Navigation display and waypoints.
- Keyboard, gamepad, and joystick input.
- Engine, actuator, stall, and environment audio from retail samples.
- Maneuver harness: level flight, sustained turn, loop, stall, runnable headless in one command.

Deliverable: take off, fly, and land any of the twelve ported aircraft on any imported theater.

Exit: harness green on all three platforms with numbers recorded.  Someone who has played the original says it feels right.

### 1d. Quick fight loop

Work:
- Quick fight setup wired end to end: aircraft, loadout, altitude, range, encounter orientation, airborne and runway starts.
- Bandits fly fixed profiles.  No AI yet.
- Visual target acquisition, target cycling, guns, hit detection, airframe damage, destruction, debris, explosions.
- Combat audio and situation music from recovered MUS scripts.
- Debrief with results.

Deliverable: a complete quick fight from menu to debrief against scripted opponents.

Exit: the loop can be run a hundred times headless with a fixed seed and produce identical results.  This is the harness for 1e and 1f.

### Ejection and pilot survival

Implemented player escape and requested AI recovery decisions. [Behaviour and fitted thresholds](spec/ejection.md),
[source evidence](formats/ejection.md), [validation](baselines/ejection.md).
Remaining: campaign rescue/capture, additional crew and original speaker routing.

### Situation audio

Implemented 2026-09-23: [radio chatter](spec/radio-chatter.md), the
[cockpit crew voice](spec/cockpit-voice.md) and [situation music](spec/flight-music.md),
from the imported retail recordings. Open items for a later pass:

- **Doubled ejection call.** When a wingman ejects, "Punching out!" can play
  twice: once from the [ejection](spec/ejection.md#audio-and-art) sound and
  once from the new death call.
- **Not yet voiced, because TORE has nothing to trigger them.** Waypoint calls
  (no routes), carrier deck and catapult calls (no carrier model), and "You're
  the Wingleader now" and the AWACS report (the original's triggers are
  unknown).
- **Vietnamese voice set.** The second (`#`) set of recordings is imported but
  only matters once Vietnam missions exist; see
  [the second voice set](spec/cockpit-voice.md#the-second-voice-set).
- **Agent choices open to tuning.** Radio traffic is on at the start of a
  session (radio silence off), and the Alt-S setting is not saved between
  sessions. The other approximations, such as contact report thresholds, fuel
  call estimates and takeoff detection, are listed in the "Implementation in
  TORE" sections of radio chatter and cockpit voice, and the "Current TORE
  state" section of flight music.

### 1e. AI

**M1 scope: air-to-air awareness and engagement.** The
[development specification](spec/ai-awareness.md) covers visual cones,
skill-scaled memory, search, missile defense, shared AI/RWR threat information
and mission rules. Observation/memory and search/Target-view activity are
implemented. Missile defense and shared RWR information are implemented;
mission rules and six per-group objective/survival selectors are implemented. Surface AI
was built in its own round on 2026-10-10 (AI-4 below); additional behavior
families remain in the broader backlog, outside this scope.

The [main AI behavior specification](spec/ai.md) now covers established fighter
choices, other family differences, surface boundaries and proposed API inputs.
The [experience specification](spec/ai-experience.md), [source map](formats/ai.md)
and [research baseline](baselines/ai-research.md) provide its companion evidence.
AI-0 has an installed-archive census; loose overrides and mission bindings remain.
AI-1 is closed: Quick Mission skill is uniform per wing, saved per-object skill
is final, the enemy-skill flight-menu override and the human-only G exemption
are specified. AI-2 covers fighter decisions, timing, pursuit, targeting,
weapon-service cadence, steering with performance and terrain rules, seeker
gates, ammunition, wing orders with formation geometry, threat warnings,
countermeasures, reason priority, routes and fuel. AI-3 has a partial runtime integration: the components are joined by a per-actor
`ai::controller` that sequences them, with `ai::fitted` supplying one named
fitted rule per unresolved branch so a live actor cannot stall. AI-5 is
partially integrated for aircraft: `ai::mission` gives every actor its own sensors,
stores, flight model and decision state, debits ammunition before emitting a
launch event, and does not duplicate missile physics. AI-6 is partially integrated for
Quick Mission: `ai::launch` carries side, wing, member, type and resolved
experience. Quick Mission enables the live hookup by default, with separate
wing groups and idle delta-formation following. `--fixture-wings` keeps the
straight-flight setup. Reviewed runtime defects are covered by the
[repair baseline](baselines/ai-research.md); AI seeker lifecycle and broader
mission integration remain partial. AI-4, surface behavior, is built for the
air defenses (2026-10-10, [surface round](#surface-objectives-and-air-defenses-2026-10-10)).
The delivery sequence below prioritizes this scope; the older backlog retains
remaining research and integration work.

#### M1 air-to-air awareness delivery

Behavior, provenance, initial tuning constants and acceptance cases have one home in
[air-to-air awareness](spec/ai-awareness.md). Deliver these slices in order:

| Slice | Concrete work | Exit evidence |
| --- | --- | --- |
| A: Observations and memory, implemented | Add actor-owned timestamped observation records between sensors and decisions; separate live observations, memories and bearing-only warnings; skill-filter visual acquisition | Cone/range boundaries, expiry, hidden-turn and Novice kill/reacquisition tests pass; no hidden world pose refresh |
| B: Search and Target view, implemented for current mission context | Connect remembered-position investigation, acquiring and return/rejoin to steering; expose real activity through the existing Target window | Lost-contact scenario visibly searches and reacquires or returns; deterministic headless transitions and display smoke |
| C: Missile awareness and defense, implemented for reviewed profiles | Connect actual A pitbull, S supported launch and I/E visual sightings through shared actor-owned RWR threat records; show known missiles in player RWR with incoming threats blinking; add skill-based time-to-defend assessment, jink/notch/dive selection, timed inventory-backed bursts and re-engagement; specify missing missile notch response | Silent midcourse and unseen passive shots provoke no response; immediate supported-launch warning; matching AI/RWR knowledge, receiver-specific blinking, safe maneuvers, effective sensor/support coupling, bounded device use and no hidden launcher knowledge |
| D: Mission roles and stances, implemented for M1 group assignments | Explicit protect/destroy/escort assignments and stance inputs, narrow protected-aircraft threat reports, priority selection and leash; connect minimal Quick Mission assignments | Escort protects its charge instead of chasing unrelated enemies; hostile escorts follow symmetric rules; objective label uses assignments |
| E: Integrated combat acceptance | Finish required AI seeker acquisition/activation/pitbull hookup; run role, sensor, weapon and survival scenarios together | Twelve aircraft by four skills, mixed roles, Novice multi-kill limits, repeated-seed replay and measured 30-aircraft fixture; full repository checks and display smoke |

Implementation should extend `tore-sim::ai::mission`, `controller`, `targeting`
and threat services, with the shared sensor component supplying observations.
Keep simulation state independent of `tore-app`; the app passes assignments and
renders activity. Existing tactics and weapon services remain reusable. Each
slice needs its own specified behavior and tests before being described as
complete. No new runtime dependency or flight-adapter default change is planned.
Search-orbit constants and current route/visibility limits are documented in the
[specification](spec/ai-awareness.md); [stage validation](baselines/ai-awareness.md).
Slice C depends on real missile lifecycle events, so bring the required seeker
activation/support adapters forward from slice E. Specify missing missile notch
rejection before accepting defensive effectiveness; attempted maneuvers alone
do not close that gap.
Report slice milestones in the session for Jeeves until the PM file is restored.

#### Visual contact and incoming-fire repair

Research completed on 2026-09-28 for John's F-22/Su-27 report and requested
all-variant check. The [baseline](baselines/visual-awareness-under-fire.md)
separates visual acquisition, neutral engagement permission and missing gunfire
self-defense. The [behavior contract](spec/visual-awareness-under-fire.md)
records agent-authored tuning and the explicit policy changes. Implementation
is complete in the isolated `fix/visual-awareness-under-fire` branch; the
[implementation baseline](baselines/incoming-fire-implementation.md) records
acceptance. Radar stealth, exact identities and normal flight/weapon defaults
are preserved.

| Order | Work | Exit evidence |
| --- | --- | --- |
| 1. Reproducible scenarios and explanations, implemented | Extend the headless probe with explicit opponent, geometry, weapon and adapter selection. Record actual sensor observations and rejection reasons separately from target choice. Add controlled anonymous air/ground projectile and damage fixtures. | The current Su-27 failures reproduce without a private harness; recordings distinguish unseen, seen-but-held, anonymous danger and rejected evidence. |
| 2. Defense without an offensive target, implemented | Give gun trajectories, close passes and weapon hits a shared incoming-fire cue in `tore-sim`; the app supplies observations/damage. Separate shooter identification from self-preservation and remove the dependency on a selected target. | Front/rear/anonymous hits and near misses request defense; harmless fire, terrain masking, stale cues and burst spam do not manufacture attackers. |
| 3. Safe jink and threat arbitration, implemented | Reuse current flight-limited jink/dive behavior; combine missile and gun danger into one response, keep terrain/energy protection and return-to-duty behavior. | Measured controls and trajectories show a safe break, not just a new label. Gun-only cues consume no countermeasures. Controlled AAA and SAM cases exercise the victim without requiring a new ground AI. |
| 4. Lookout and leader decisions, implemented | Implement the proposed side/rear visual lookout and mission-aware contact response. Update the current neutral-leader contract explicitly; retain human wing orders, hold, recall and mission restrictions. | All aircraft can notice visible F-22s and take the response their duty permits. No radar-signature workaround, automatic revenge target or hidden tracking. |
| 5. Integrated acceptance, measured | Repeat the full matchup matrix at all four skills, front/side/rear and boundary geometries, both legacy/researched paths, repeated seeds, terrain/energy limits and mixed air/ground fire. Keep native-table research separate. | Replay reconstruction and deterministic comparisons, repository checks and display captures pass; outcome/flight safety limits are reported separately from perception. |

The probe also now records real AI bank and pitch, with a direct flight-to-drawn
attitude check before replay reconstruction. Surface firing AI, full environmental
visibility, aircraft-specific cockpit blind sectors and original-game timing
remain separate work. Milestones are reported in the session for Jeeves.

#### AI gun employment and timeout recovery

Implemented on 2026-09-28. The [investigation](baselines/ai-gun-employment.md)
identified sparse groups, terminal timeout and target-directed launches. The
[correction](spec/ai-gun-employment.md) now provides bounded expiry retry,
physical burst accounting, fixed-barrel lead and replay explanations.
[Validation](baselines/ai-gun-implementation.md) records component checks and
sustained imported fights. Original gun timing and skill accuracy remain unknown;
hard-turn tracking is still fitted. Surface firing AI remains separate work.

#### AI response to system damage

Implemented after integrating main's Realistic NPC damage. Pilots react to live
faults, stop offensive engagement, use their existing recovery destination and
protect engines where flight safety permits. The existing physical model still
owns jammed controls, progressive failures and death. Dummies retain fixed-course
behavior and damage capacities are unchanged. [Authored contract](spec/systems-damage.md#ai-pilot-response-to-faults),
[validation](baselines/ai-damage-response.md). Original fault-response tactics,
unreachable-field diversion and component repair remain outside this delivery.

#### AI backlog (2026-09-17)

Established rules with tests are listed in the
[implementation status table](spec/ai.md#implementation-status). Everything
else falls into one of four kinds.

Missing evidence (research, in priority order):

1. Remaining signature branches and seeker state producers beyond the reviewed
   B45 launch envelopes, target-class selection and support rules.
2. Remaining tactics: last-ditch candidate suitability, the random-tactic menu
   contents, engagement-pitch rule, jink and circle shapes, what a script
   restart does to a maneuver in flight (B11, B12, B13, B47).
3. Surface classes (B30): SAM, AAA, flak, ship and armed vehicle engagement
   and route movement are specified and built from the retail records
   ([surface defenses](spec/surface-defenses.md)); still open are the event
   mask meanings, the surface target ranking terms, the hit-chance rule, the
   unready flag's producer and the hydrofoil and carrier programs
   ([unknowns](spec/surface-defenses.md#unknowns)).
4. Wing remainder: approach steering point, mode 9 negative-band entry,
   loose-versus-medium self-engagement, 20 second target deadline expiry,
   reply voicing (B43, B46).
5. Recovery and survival: damage-triggered disengagement and attack-state
   producers (B48); vertical, STOVL and carrier airfield variants. Takeoff and
   landing sequences and leader and singleton return to base are implemented
   ([AI airfield sequences](spec/ai-airfield.md)); retail timing comparison is
   still unmeasured.
6. Small units: B05 thrust-to-weight scale, pursuit offset signs, lead speed
   estimator, minimum-speed exemption producer, bank bound second term, burst
   policy after a shot, decoyed-missile time shortening, template ground skill
   values, prefs persistence of the enemy-skill override.

Implementation work (spec established, not yet coded): family variants for
F-117, helicopter, bomber, AC-130, large and MOTH behaviors (B20), which
`Controller::new` currently rejects rather than serving fighter behavior; the
hydrofoil program (B30). Surface actors run their own controller
(`tore_sim::ai::surface`) since 2026-10-10; `ai::targeting` still does not
rank them for aircraft.

Fitted rules now standing in for unresolved branches, each named in
`ai::fitted` and listed in [behavior provenance](behavior-provenance.md):
engagement pitch, base pitch rate, zero-duration completion axis, last-ditch
candidate suitability, random-tactic menu contents, remaining tactics,
lead speed estimator, burst and reload pacing, store hit chance, and leader
and singleton return to base. Each becomes spec-derived when its research item
above closes; none is a claim about retail behavior.

Validation work: synthetic scenarios per aircraft and experience level (48
combinations) that run the components together headless with a fixed seed;
determinism and restart tests for the live controller; a review of the
fitted steering curves against any flight-model turn data already measured.

Integration work remaining after the 2026-09-17 hookup: mission routes and orders beyond the fuel and waypoint rules
already wired; full AI seeker acquisition, activation and pitbull beyond imported launch
envelopes; and the wing-approach value producer, without
which the B12 wing-split branch stays untried rather than being fed ordinary
target distance.

Initial AI delivery covers every aircraft in the
[ported roster](spec/ai-experience.md#currently-ported-aircraft). All twelve bind
to the fighter/strike family in FA, so the first family implementation must
serve all twelve using their own capabilities. Acceptance covers 48 combinations
of aircraft and experience level, plus mixed-aircraft encounters. Wider retail
families remain in the research plan without delaying this roster behind new
aircraft imports.

| Stage | Work and deliverable | Exit evidence |
| --- | --- | --- |
| AI-0: FA inventory | Extend the initial FA_2.LIB census to archive precedence and mission bindings; retain exact aircraft identities and separate static scenery from autonomous objects | Every referenced behavior family has a build/source identity, evidence category and explicit gap list; no unnamed fallback controller |
| AI-1: Experience | Trace the six wing selections, four side/domain assignment channels, per-object mission values and all type-appropriate skill consumers | Specify Quick Mission distributions, saved-skill precedence, tactical percentages, G exemption and device reactions; synthetic boundary cases for 0..3 and invalid input |
| AI-2: Aircraft behavior specifications | Complete fighter/strike and defensive behavior first, then F-117, helicopters, bombers/AC-130, transports/airliners and special families; include formation, orders, navigation, fuel and damage responses | For each maneuver and decision, prose gives trigger, target geometry, units, limits, duration/completion and interruption rules; source/BI disagreements and unsupported aircraft motion are explicit |
| AI-3: Isolated Rust components (runtime gaps remain) | Implement specified behavior slices in renderer-independent `tore-sim::ai`; add only needed bounded data readers to dependency-free `tore-formats` | Deterministic headless scenarios exercise each family's decisions and maneuvers at all applicable experience levels; known approximations have named rules/constants and provenance |
| AI-4: Surface behavior (built for air defenses, 2026-10-10) | Trace and specify static defenses, SAM, AAA, mobile ground units, ordinary ships, hydrofoil and carrier behavior separately, then implement isolated components | Built: SAM, battery, AAA, flak, barrage, ship and armed vehicle controllers with detection, fire control, experience and route movement, each with fixture tests and battery scenarios ([surface defenses](spec/surface-defenses.md), [acceptance](baselines/surface-defenses.md)); scenery never fights. Open: hydrofoil and carrier programs, surface against surface |
| AI-5: Simulation service adapters (partial for aircraft; surface units since 2026-10-10) | Generalize actor ownership for sensors, weapons, missile support, damage, fuel and movement; feed isolated controllers through those services | Multiple actors own independent contacts, stores and targets; no free ammunition, omniscient targeting by accident, duplicated missile physics or player-state contamination |
| AI-6: Later game hookup (partial for Quick Mission) | Replace the lossy dummy-wing launch payload with side, wing, member, type, loadout, experience source and resolved level; connect mission routes/orders and activity display | Six mixed-skill wings retain identity end to end; replay and headless/live results agree; player and straight-flight fixture paths remain available |

Research can advance by family and spec section; completing all executable
routines is not a prerequisite to implementing a specified slice. Surface
research can proceed once shared actor/weapon contracts are understood, without
waiting for every aircraft family. Surface firing implementation depends on
surface sensing/designation and launcher ownership, which are not supplied by
the current aircraft range. Helicopter, bomber and ship decisions can be tested
in isolation before their required movement/import coverage exists; that does
not count as flown acceptance.

Proposed interface: a controller receives its mission/order context, own state,
equipment, resolved experience, permitted observations and threat events. It
returns desired motion, sensor/target requests, weapon/device requests and a
player-readable activity. It does not directly move bodies or apply damage.
Aircraft motion requests go through a steering adapter and the selected flight
model; surface motion uses its own bounded rules. Retail awareness shortcuts,
if established, must be explicit spec-derived inputs rather than hidden access
to all entity positions. No default flight adapter changes are included.

Keep behavior family, mission role, equipment and experience as independent
inputs. A fighter family does not imply that a particular aircraft carries a
radar missile; Ace does not imply better radar hardware. Use one controller
implementation with experience-dependent rules where supported. Keep actors'
decision state and deterministic random streams in simulation state. This is a
proposed host design; the original RNG ordering and command buffers are not
acceptance targets. Fixed 120 Hz simulation stays independent of rendering.

Acceptance scenarios must include:

- Head-on merge, pursuit, overshoot, defensive break and simultaneous threats at
  each level, with maneuver envelopes and durations checked against the spec.
- Conditional tactical probabilities tested independently of encounter win
  rates; explicit draws at each threshold, plus aggregate checks with stated
  sampling tolerances. Higher experience need not win every random fight.
- Lost/occluded contacts, wrong-side and destroyed targets, missile-support
  loss, empty stores, low fuel, damage and terrain avoidance. Fitted rules are
  tested as fitted rules, not claimed as measured retail behavior.
- Wing orders, formation split/rejoin and interrupted route resumption, then
  SAM/AAA engagement, a moving vehicle and a ship attack in separate fixtures.
- Same seed and inputs across repeated runs and render schedules; stable actor
  identity through removal and restart. Check a 30-aircraft fixture matching
  current creator capacity and record cost, without inventing a retail timing
  requirement or a performance promise before measurement.

Final deliverable: aircraft and supported surface opponents with behavior tied
to the correct experience input, integrated only after isolated acceptance.
Liveness means an eligible combatant can act and fire when appropriate, not that
unarmed transports or scenery must engage. Retail recordings may add evidence
if available later; their absence is not a blocker and passing our tests is not
a claim of demonstrated retail parity.

### 1f. Sensors and weapons

Current authorized slice: shared aircraft radar, requested 2026-09-16, independent
of the AI milestone. [Behaviour and roster stats](spec/radar.md),
[standard component guide](radar.md), [validation](baselines/radar.md).
Research established the imported radar profiles and the normal range-mode rule;
the component then shipped on the same day. Implementation milestones:

1. **Done.** Shared radar/IR A2A profiles and simulation-owned observations with
   one selected target and at most one fire-control track, using PT radar/IR
   signatures and the authored look-down, era, notch and jammer model.
   Destroyed-aircraft combat state is separate from physical sensor presence.
2. **Done.** Scope range/mode correction, directional jammer noise, the RCS
   exposure panel, persistent mouse selection and Y history using stable target
   IDs. Aircraft orientation feeds the same effective signature used by the RCS
   contour and detection. Radar/jammer generation matchups are explicit profiles.
3. **Done.** Radar-guided launch and maintained-support transitions with
   deterministic tests, including version-3 combat tapes.

Remaining in this slice: the tuning pass. All twelve aircraft produce the
capability summary and pass their combat smokes, but the twelve results have not
been reviewed side by side, so the presets and jammer matchups are unplayed
against each other. See the
[delivery table](radar.md#delivery-and-acceptance) for what each stage delivered.

Mode scope is TWS/RWS and installed infrared air-to-air, with single-target
tracking only. Gamified IFF stays in the target view. The air-to-ground (A-G)
page, the IR page's ground objects and the HARM page arrived with the
air-to-ground round (2026-10-10, [spec](spec/air-to-ground.md#the-air-to-ground-radar),
[evidence](baselines/air-to-ground.md)): the retail ground lists and symbols,
no ground map and no moving-target picture. No realistic IFF, multi-track or AI
work is authorized.
Unresolved retail details use documented fitted rules; complete source-code
closure is not a prerequisite.

The [missile update plan](missile-update-plan.md) records the completed current-store
implementation: four guidance types, pitbull activation, launch velocity and range,
motor burn, guidance lifetime, uncued seeker search and HUD/tone feedback, with
delivery stages, [measured acceptance and remaining limits](baselines/missiles.md).

#### Missile flight overhaul (review corrections validated, study stopped)

Authorized by John on 2026-10-25. The shared finite-energy kernel, physical
decoy pursuit, bounded predictions, public/class physical data and measured A2A
envelopes are implemented in new worlds. Linux compatibility and regression
checks passed for round 3. John's subsequent decision removes only the geometry
drag multiplier, retaining the newer-Sparrow refit and turning-player advice.
Regenerated range measurements, Linux workspace checks, all 35 battery
scenarios and sequential cost lanes validate the restored drag.
Phase 3 remains stopped pending lead review;
its earlier results remain invalidated. [Behaviour](spec/missile-energy.md),
[data provenance](formats/missile-energy-data.md) and
[measured baseline](baselines/missile-energy.md) own the details.

The findings below describe the preserved earlier Spec adapter. Its exact
cost shortcuts and old-tape resimulation remain available.

Current findings, checked after the cost commit `8d883e95`:

- **Acceleration can end before the motor burns out.** `Motion::step` adds
  `acceleration * DT` only until `gain` exhausts the budget
  ([missiles.rs:609](../crates/tore-sim/src/combat/missiles.rs#L609)). `phase()`
  remains Powered until launch age `fuel_t * 30` ticks
  ([missiles.rs:355](../crates/tore-sim/src/combat/missiles.rs#L355)). Smoke,
  flame, powered turn rate and the HUD's BURN label follow that phase, not
  remaining thrust: [smoke, live.rs:5405](../crates/tore-sim/src/combat/live.rs#L5405),
  [flame flag, combat.rs:1177](../crates/tore-world/src/combat.rs#L1177),
  [flame drawing, render_snapshot.rs:434](../crates/tore-app/src/render_snapshot.rs#L434),
  [turn rate, missiles.rs:715](../crates/tore-sim/src/combat/missiles.rs#L715),
  [BURN, weapon_hud.rs:767](../crates/tore-app/src/weapon_hud.rs#L767).
  The [AIM-120 record](formats/fa-weapons.csv#L25) burns for 66 seconds. Once
  the budget is spent, [turn losses](../crates/tore-sim/src/combat/missiles.rs#L590)
  are never recovered while lit. Existing [FA evidence](formats/missiles.md#compatibility-and-current-launch-adapters)
  describes acceleration toward an absolute maximum, holding it and recovering
  toward it after turns ([combat.rs:104](../crates/tore-sim/src/combat.rs#L104),
  [combat.rs:119](../crates/tore-sim/src/combat.rs#L119)).
- **There is no absolute speed cap.** The budget is
  `max(0, altitude-adjusted maximum_speed - initial_speed)`, fixed at launch
  altitude. Straight,
  aligned flight can reach inherited launcher speed plus that budget, exceeding
  `maximum_speed`; later altitude changes do not recalculate it
  ([missiles.rs:533](../crates/tore-sim/src/combat/missiles.rs#L533)).
- **There is no aerodynamic drag law.** After burnout, speed falls by fixed
  deceleration toward the record's final speed. The separate fitted turn loss
  remains ([coast, missiles.rs:619](../crates/tore-sim/src/combat/missiles.rs#L619)).

Some range limits stay fixed even when launch speed changes:

| Path | Current rule |
| --- | --- |
| IR, semi-active radar and emitter maximum-range estimate | The speed-aware search is capped at zone 0 seeker range. Active radar has no such cap. [missiles.rs:1085](../crates/tore-sim/src/combat/missiles.rs#L1085). |
| Compatibility weapon rules | Readiness uses the fixed zone 1 range. [live.rs:2689](../crates/tore-sim/src/combat/live.rs#L2689), [live.rs:2798](../crates/tore-sim/src/combat/live.rs#L2798). |
| Powered air-to-ground release bars | `reach_uncached` returns the geometric zone when `!glide`; it does not solve speed-dependent reach. [guided.rs:322](../crates/tore-sim/src/combat/guided.rs#L322). |
| AI launch gate | Store setup copies zone 1 maximum into `spec.maximum_range_ft`; the controller checks that range and the fixed employment zone. [ai_wings.rs:607](../crates/tore-world/src/ai_wings.rs#L607), [controller.rs:417](../crates/tore-sim/src/ai/controller.rs#L417). Released AI missiles still inherit launch velocity and use the shared motion. [ai_wings.rs:3230](../crates/tore-world/src/ai_wings.rs#L3230), [ai_wings.rs:3314](../crates/tore-world/src/ai_wings.rs#L3314). |

The release code also computes the compatibility `launch_speed` scalar, but a
Spec missile's `Motion::launch` receives `launcher.velocity`. Live flight then
sets the displayed projectile speed from `motion.velocity`. Changing the scalar
helper alone will not change that flight model. See
[cockpit.rs:845](../crates/tore-sim/src/combat/live/cockpit.rs#L845),
[cockpit.rs:914](../crates/tore-sim/src/combat/live/cockpit.rs#L914),
[cockpit.rs:917](../crates/tore-sim/src/combat/live/cockpit.rs#L917) and
[live.rs:5578](../crates/tore-sim/src/combat/live.rs#L5578).

Decisions for John before implementation: keep the FA accelerate-and-hold shape,
or choose boost then coast with drag; clamp at `maximum_speed`, or retain the
additive launch speed requested on 2026-09-17; retain or change the seeker-range
cap; and decide whether AI launch gates should use the speed-aware estimate.
These are deferred decisions, not changes authorized by the cost optimization.

A new model changes simulation results. Update the behaviour spec first, then
review guard references and golden fingerprints, including [CI on `macos-14`](../.github/workflows/ci.yml#L23).
Plan a protocol bump from the current **54**
([wire/mod.rs:255](../crates/tore-session/src/wire/mod.rs#L255)), and review the
[missile checkpoint layout, missiles_checkpoint.rs:43](../crates/tore-sim/src/combat/missiles_checkpoint.rs#L43)
and combat tape **VERSION 8** ([tape_file.rs:28](../crates/tore-app/src/tape_file.rs#L28)). Re-run the [missile reach cases](baselines/missiles.md)
and `--missile-acceptance`. The TTI shortcut
[`straight_seconds`, guided.rs:102](../crates/tore-sim/src/combat/guided.rs#L102)
and the [conservative fly-out bounds](ARCHITECTURE.md#the-firing-estimate-and-its-exact-shortcuts)
must follow the chosen model as well as live flight. No protocol, checkpoint
or tape change is part of this backlog note.

#### Weapon catalog update passes

Requested by John on 2026-09-23: Load Ordnance hides weapons without connected
flight behaviour. Normal loading shows supported weapons that fit the selected
aircraft; Cheat shows all imported supported weapons. The
[catalog specification](spec/ordnance-presentation.md#catalog-availability)
defines visibility. The current flight support list contains 72 weapons (2026-10-11,
after the air-to-ground round) and the Cheat-only Boulder and Moose;
importing the wider [JT inventory](formats/fa-weapons.csv) does not enable the rest.

The agent-selected passes below plan the remaining weapons. Each starts with a
player-facing behaviour spec and records unknowns with a next research step.
Existing [missile inventory classifications](spec/missiles.md#first-pass-inventory-matrix)
are provisional where marked; a filename or a shared guidance type is not enough
to accept a new weapon. These are future implementation passes, not weapons
enabled by the catalog fix.

| Pass | Remaining scope and examples | Acceptance before catalog exposure |
| --- | --- | --- |
| W1: Conventional guns, rockets and bombs | **Bombs and rockets delivered 2026-10-11 (air-to-ground round, [evidence](baselines/air-to-ground.md)):** MK84, FAB-250, FAB-500, FAB-1000, the Mk 82 pack and retarded variants and the LAU-10 are loadable; bombs fall on the retail equation, packs release together and pods hold their real rockets. **Remaining:** the aircraft cannon pods ADEN, BK27, GAU-13 and GSh-6-23 (GAU8 is supported already), which nothing loads. Original scope: remaining aircraft cannon and pods such as ADEN, BK27 and GAU8; LAU10 rockets; MK84 and FAB-series bombs | Specify ammunition/pod counts, release cadence, trajectories, impact/fuze behaviour and damage. Exercise manual release, depletion, mass, compatible stations and accepted-load restart. |
| W2: Remaining air-to-air missiles | AIM7/AIM7E, AIM9B, AA10/AA6/AA9, PL7/PL10 | Resolve each guidance profile, launch limits, support loss, seeker acquisition, motor/lifetime and HUD/audio feedback. Validate against spec numbers and deterministic manual target fixtures. |
| W3: Surface and anti-ship missiles | **Delivered 2026-10-11 (air-to-ground round, [evidence](baselines/air-to-ground.md)):** AGM65A, AGM84A/AGM84E, AM39, AS15/AS16 and AGM45/AGM88 fly from the ground designations, the A-G track, the HARM boresight cue and the HARM page; online since protocol 23 (slice NA), the HARM cue and the ground pages included. Original scope: AGM65A, AGM84A/AGM84E, AM39, AS15/AS16; AGM45/AGM88 passive-emitter weapons | Connect the required ground/ship targets, designation and emission state first. Validate valid and invalid targets, lost emissions/support, flight, impact and damage without granting hidden target knowledge. |
| W4: Designator and unresolved guidance | **Delivered 2026-10-11 (air-to-ground round, [evidence](baselines/air-to-ground.md)):** the laser bombs and laser missiles home on the aircraft's own laser spot or a same-side buddy's (John's addition) and go blind when the spot ends; the AT-2 is command guided. Remaining: a JDAM "release at the bar" mode on the level T and a viewer overlay for laser spots. Original scope: GBU10/GBU10A, GBU28, PAVEWAY/PAVEWA3; AS14, AS30, AT12 and AT2 | Specify the actual guidance and designation requirements before selecting a model. Connect acquisition, release, loss of designation and terminal impact; do not substitute a radar or heat seeker for an unresolved contract. |
| W5: Special payloads and delivery variants | **Delivered 2026-10-11 except AEMP1 (air-to-ground round, [evidence](baselines/air-to-ground.md)):** the cluster bombs (retail wide splash and presentation-only flashes), the Mk 82 variants and the JDAMs (John's choice: fly to the point designated at release, where retail homes on the object). **Remaining:** AEMP1, an air-to-air EM pulse that nothing loads. Original scope: CBU87/CBU89, MK20, RBK250/RBK500, MK82 variants, GBU29/GBU29P/GBU30 and AEMP1 | Resolve each record's payload and release behaviour. Implement the applicable submunition, retarding, penetration or special damage effect with numerical tests; keep unresolved and internal novelty records hidden. |
| W6: Ground and naval weapons | Remaining surface gun rounds, SAM/ship missiles, ASROC and other held surface roles in the inventory | Built for air defense on 2026-10-10 with John's authorization: the surface gun rounds with the AAA tuning table, flak, the 13 radar and 6 infrared SAM and ship missile profiles, fired by surface units only ([surface defenses](spec/surface-defenses.md)). Held: ASROC and the SS-N-9 anti-ship missile (surface against surface is out of scope). |

Every pass updates the shared flight support list only for weapons that have
working behaviour, plus the feature matrix and one feature baseline. Check both
catalog modes, normal compatibility, Cheat transitions, launch/restart and
deterministic headless execution. Record fitted or opinionated rules explicitly;
retail comparison remains unavailable. Auxiliary tanks and equipment need their
own loadout/fuel work and are not enabled by these weapon passes.

Work:
- Radar modes, RWR, IFF, and the retail sensor model.
- Missiles, bombs, and gun stats from retail data.  Stores affect weight and flight.
- Per-system damage.
- SAM sites and ships as targets and threats. Built 2026-10-10 (surface round,
  [Milestone 2](#surface-objectives-and-air-defenses-2026-10-10)): Quick
  Mission ground targets in all 16 theaters and the base layouts' defenses.
  Player ground designation and A2G weapons are the next workstream.
- Extend the decoded station compatibility to auxiliary stores and remaining
  stock/year/airbase rules; supported weapon catalogs already apply it.

Deliverable: full quick fight with retail weapons against air and ground threats.

Exit: **Milestone 1 tagged.**  A stranger can install it on any of the three platforms, import their own disc, and fly a quick fight.

---

### 1g. Installer and first-run import

Planned 2026-09-21 at John's request. A player installs T.O.R.E on Windows,
macOS or Linux, points it at their own Fighters Anthology, and reaches the main
menu without a terminal. Behaviour: [first-run import](spec/first-run-import.md).
Container research: [SETUP.ESA notes](formats/esa-installer.md).

What the research settled:
- The importer needs five files (`FA_1.LIB`, `FA_2.LIB`, `FA.EXE`, optional
  `FA_4B.LIB`, `FA_4D.LIB`). On the retail discs they live inside
  `disc1/SETUP.ESA`, a flat container whose compressed entries use the DCL mode
  the LIB reader already decodes. Disc 2 is not needed.
- The disc's 1.0 executable carries the same creator, cloud, flare and radio
  tables as the reviewed 1.02F build at shifted addresses; content is identical.
  The patch changes no gameplay resource the app reads.

Slices, in order:

1. **Media sources.** `tore-formats::esa` reader with synthetic tests. A
   `MediaSource` in `tore-app` that yields the five files from an installed
   folder or a disc folder, detected by content. Second executable fingerprint
   with its address set in the four table readers. Import report names the
   build. CLI: `--import` accepts either kind; `tore-extract` gains
   `--source` support for a disc folder.
2. **First-run screen.** Locate screen in original menu art, path field,
   drag-and-drop through winit's file-drop event, automatic detection of
   volumes and conventional folders, progress, error text, Pref re-import,
   remembered source. No new runtime dependency; a native file dialog is
   deferred until drag-and-drop has been tried by players.
3. **Packages.** CI job producing MSI, DMG and AppImage plus tar.gz from one
   tag, unsigned, each scanned by `tools/check_assets.py` before upload.
   Signing is a later, separate change.

Decisions recorded 2026-09-21 (John): mounted disc or copied folder, no raw
ISO reader; proper installers rather than portable archives; unsigned first
builds.

#### Implementation plan (2026-09-22)

Execution plan for the three slices, written after a code survey. Behaviour
stays in the [spec](spec/first-run-import.md); this section owns sequencing,
ownership and the agent decisions that the survey forced. Every type and file
name below is an agent design choice, not an existing API.

What the survey established:

- The importer's only file lookups are one case-insensitive `archive()` helper
  and two inline `FA.EXE` scans in `tore-app/src/assets.rs`. One `MediaSource`
  replaces all three.
- `Archive` opens a whole file from offset zero. The four LIB archives are
  stored uncompressed inside `SETUP.ESA`, so a base-offset constructor lets the
  existing EALIB reader serve a disc without copying 108 MB.
- Three table readers gate on the 1.02F hash; `radio::phrases` has no gate and
  would read wrong addresses from the 1.0 build. All four move to one build
  table with two address sets.
- Every game object in `tore-app/src/main.rs` is built from imported assets
  before the window opens, and the renderer needs a terrain world. A first-run
  screen therefore cannot live inside the game `App`.
- There is no text field, no file-drop handler, no clipboard and no release or
  packaging job anywhere in the repository.

Agent decisions, recorded as agent decisions:

- **Pre-game shell.** The locate screen is a separate winit application
  handler run with `run_app_on_demand` before the game app on the same event
  loop, presenting a 640 × 480 CPU canvas through a minimal blit-only wgpu
  path. Re-import from Pref ends the game app with a re-import outcome and the
  shell runs again. `EventLoop::new` is never called twice.
- **Art before import.** Before any import exists there is no retail art on
  disk, so the locate screen uses the bundled menu font and flat panels from
  `menu.rs`. When a pack already exists (re-import, or a stale cache) the same
  screen draws over the retail Choose Activity background. The spec's "original
  menu art" is amended to say this.
- **Text entry without a clipboard.** The path field accepts typed characters,
  Backspace, Delete, Home, End and Enter. There is no paste; drag-and-drop and
  automatic detection cover the common cases. A clipboard dependency is not
  added.
- **Import progress.** `Assets::import` gains a progress callback and runs on a
  worker thread; the shell polls a channel and redraws. The import result and
  report text are unchanged apart from the build line.
- **Build identity.** `tore-formats::executable::identify` maps a hash to a
  `Build` with per-build addresses. Unknown hashes are refused before any
  archive is read. The report line becomes `FA.EXE: 1.02F` or `FA.EXE: 1.0
  (disc)` followed by the hash.
- **Packages.** A tag-triggered release workflow builds `--release` on the four
  CI images, stages a bundle directory per platform, runs
  `tools/check_assets.py` on the staged directory and on the finished package,
  then uploads unsigned MSI (WiX from the runner image), DMG (`hdiutil`),
  AppImage (`appimagetool` pinned by hash) and tar.gz. No Rust dependency and no
  signing.

Work packages and file ownership:

| Package | Owns | Depends on |
| --- | --- | --- |
| A: ESA reader and CLI | `tore-formats/src/esa.rs`, `Archive` base-offset constructor in `tore-formats/src/lib.rs`, `tore-extract/src/main.rs`, `docs/EXTRACTION.md`, `docs/formats/coverage.md` | none |
| B: executable builds | `tore-formats/src/executable.rs`, `ui/creator.rs`, `ui/fingerprint.rs`, `weather/clouds.rs`, `weather/flare.rs`, `radio.rs`, `docs/formats/esa-installer.md` implementation notes | none |
| C: media source and importer | `tore-app/src/media_source.rs`, `tore-app/src/assets.rs`, `--import` handling in `main.rs` startup only | A, B |
| D: packaging | `.github/workflows/release.yml`, `tools/package/*`, `docs/DEVELOPMENT.md` packaging section | none |
| E: locate screen state and drawing | `tore-app/src/locate.rs` (state, key handling, layout, headless snapshot test), Pref entry in `tore-app/src/menu.rs` | none, integrates in F |
| F: shell integration | `main.rs` (pre-game shell, file drop, re-import outcome, remembered source), `tore-app/src/canvas_present.rs`, `docs/spec/first-run-import.md`, `docs/features.md`, `docs/ARCHITECTURE.md` | C, E |

A, B and D run together; C and E follow; F closes. Each package runs the
repository checks on its own files before handing back; F runs the full set
plus the display smoke test and a real first run against the local disc 1
folder and the installed folder, with `TORE_DATA_DIR` pointed at an empty
directory. Windows and macOS runtime validation is recorded as pending until a
package is installed on each.

Current (2026-09-22): slices 1 to 3 are implemented. The media source, the
ESA container reader, both executable builds, the locate screen, the pre-game
shell with drag-and-drop and Pref re-import, and the four release packages are
in the tree and validated on Linux. The release workflow has built the MSI,
both DMGs, the AppImage and the tar.gz on a `release-test` branch, and every
package passed the asset guard. Installing and running them on Windows and
macOS is pending.

Deliverable: on each platform, install, choose a mounted disc 1 or an
installed folder, and fly the README free-flight check without using a
terminal.

#### Startup diagnostics

John requested built-in startup logging and visible failures on 2026-09-23.
Implemented as an opinionated host feature: bounded cross-platform session and
panic logs, native Windows/macOS failure UI, Windows MSI event registration,
embedded event messages and media-free package validation. The
[contract](spec/startup-diagnostics.md) owns behavior and limits; the
[validation pass](baselines/startup-diagnostics.md) owns measured results.
Installed Windows/macOS desktop and Event Viewer acceptance remain pending.

### 1h. Cheats

Planned 2026-09-23 at John's request. Every entry in the in-flight Cheat menu
and the loadout screen's Cheat button works as described in the
[cheats specification](spec/cheats.md). Two already work: No turbulence and No
sun whiteout.

Slices, in order:

1. **Cheat settings and the simple cheats.** One cheat settings object shared
   by the flight menu, the simulation and restart. Unlimited fuel, Unlimited
   ammo, No spins, Pull extra G, Ignore weapon weights, Damage Invulnerable and
   Normal, and the loadout Cheat button.
2. **G effects.** Blackout, redout and the high-G screen shake, plus the No
   redout or blackout and No screen-shaking cheats.
3. **Weapon hits.** The missile hit jolt for every aircraft, No crashes
   ricochet, and Easy aiming.
4. **Midair collisions.** Always fatal to every aircraft involved, plus Ignore
   midair collisions. Touches AI files.
5. **Targeting.** Separate Enter (visual) and T (radar) selection, and a
   target drops when radar loses it. Then Easy targeting.
6. **AI cheats.** Live Enemy AI skill and Air combat guns only for AI aircraft.
   Touches AI files.

Current (2026-09-23): slices 1 to 6 are implemented on the `cheats` branch.
Slices 4 and 6 touch AI files another agent is working in, so merging needs a
check against that work. Damage Normal (hit points only) and Realistic (hit
points plus system faults) were split on 2026-09-28, and AI aircraft now always
take Realistic damage. The fitted numbers in the
specification await John's review.

Deliverable: each cheat toggles mid-flight from the menu, shows On or Off, and
survives Restart.

### 1i. Aircraft pass

Planned 2026-09-30 at John's request, from what the
[overnight battery](baselines/battery-2026-09-28.md) left open. It is built on
the multiplayer branch after its stage C and before stage D. The measured
options it chose from are in the battery report's section 4 and the pass notes.

Decisions (John, 2026-09-29 and 2026-09-30):

1. **A fitted stall-speed reference for each aircraft.** The weight-scaled
   stall speed keeps its rule, and each aircraft gets its own reference weight,
   fitted to John's liftoff and approach figures (unsourced, used as a
   plausibility range). Measured on a scratch build: every approach within 10
   percent of his figures (8 of 11 inside the range), liftoff for the Su-27,
   Su-25 and F-22 family within 6 percent, and combat-speed G at the retail value
   for all 14 aircraft. The cost: the F-22 family needs about 200 kt to hold level
   flight clean, against 150 kt. The X-31, with no figure, keeps today's
   reference. `fitted`.
2. **The Su-27 in the middle.** John's liftoff (135 to 150 kt) and approach (120
   to 135 kt) figures cannot both be met in this model, which puts its approach
   at about 1.1 times its liftoff. The fitted value gives about 128 kt and 142 kt.
3. **No short strips for a ground start.** The 22 airstrips of about 1,074 ft (in
   Cuba, the Falklands, Pakistan, Panama and the Persian Gulf) are no longer
   offered in the Quick Mission takeoff choice, so neither the player nor an AI
   wingman starts on one. This settles the strip questions for the player, the
   wingmen and wings of two or more. Changed 2026-10-11 (John): they are
   offered again, labelled STOL ([STOL runways](spec/airports.md#stol-runways)).
4. **No short strips in flight.** They also leave the in-flight airport list:
   navigation, landing guidance and the wing's land order.
5. **The gear stays down on the ground.** As in retail, the gear cannot be
   raised while the aircraft has weight on its wheels, and the gear key does
   nothing. T.O.R.E adds a message, "Ground sensor preventing gear retraction"
   (`opinionated`). This replaces the belly slide that the gear key started on
   the runway (battery report, section 3.9, item 3).
6. **The wingman who takes over carries on.** When an AI wingman takes the lead
   from a lost human, it continues with a mission of opportunity: it searches by
   eye around the enemy's last known positions, and if it finds nothing it
   returns to base (John, 2026-09-30). Before, it flew straight on until the
   world edge removed it.

Also in the pass: the AI probe's scripted pilot survives its takeoff and
climb-out from every ground start the battery uses (slice P5: it holds the
runway line, respects the aircraft's minimum speed and clears the terrain
ahead), so the landing scenarios test a living player. See
[the scripted pilot](testing/lane-ai.md).

Current (2026-09-30): built on the multiplayer branch in five slices, each
checked against the single-player baseline. The stall table
(`STALL_REFERENCE_FRACTIONS`) gives the numbers above. A short strip is an
airport whose longest runway is under 2,000 ft (`fitted`: the 22 strips are
1,074 ft and every other airport is 4,060 ft or longer); every theater keeps at
least three ground-start airports. The ground sensor replaced the belly slide,
whose code went with it. The new lead's waypoint step works, but no mission has
waypoints yet (Quick Mission imports no route), so in play it searches and then
goes home. The probe pilot now holds the runway against the wind, climbs over
the terrain ahead and survives every start, and a probe option
(`--probe-lose-player`) loses the player on purpose where a scenario tests the
new lead.

Acceptance: liftoff and approach speeds for every aircraft recorded against
John's figures, with the flight lane's liftoff and approach checks re-recorded;
combat-speed G unchanged for all 14 aircraft; no short strip offered for a ground
start or in flight, with every theater keeping at least one airport; the gear key
on the ground shows the message and the gear stays down; an AI that takes the
lead from a lost human searches and then returns to base; the whole battery
passing, its checks updated for these decisions; the single-player baseline
differences explained as planned changes; the specs and lane pages updated.

### Aircraft variety import

Requested by John on 2026-10-05. The `import-variety` branch started from
multiplayer at `7d25975b`. The [23-aircraft matrix](spec/aircraft-variety.md)
covers transports, a gunship, airborne radar aircraft, VTOL jets, a tiltrotor,
helicopters, fighters including all four retail F-4s, and two airliners. Exact
variant choices are recorded as agent choices; 20 was not a hard limit.

Initial ports are implemented: reviewed source imports, aircraft-owned hybrid
configurations, powered-lift flight, playable controls, cockpits and
initial device rigs. AC-130 selected-target tracking and linked gun groups are
connected to simulation, rendering and recordings. The
[flight baseline](baselines/variety-flight.md) distinguishes tested behavior
from the remaining animation and wider systems acceptance. No new AI pilot
behavior or new-family AI acceptance is claimed.

[Selectable external tanks](spec/external-fuel-tanks.md) now separate installed
quantities, shell mass and fuel. The F-14 source probe validates zero, one and
two tanks through accepted-load restoration. The F-4J gun pod likewise keeps
installed hardware separate from its ammunition. Source-specific capacity and
ammunition fits are documented rather than presented as recovered retail laws.

The [import guide](aircraft-import.md#worked-variety-import-examples) now has
worked transport, F-4 family and powered-lift/system examples. Next: human
handling review, remaining moving surfaces and cockpit mirrors, fuller AWACS
sharing/operator behavior, tank visuals/jettison and wider platform/controller
acceptance. Keep per-aircraft status and evidence in the matrix.

**VTOL and helicopter overhaul.** Designed with John's twelve decisions of
2026-10-08, after the fitted attitude-hold law proved unflyable (an AH-64 topped
out at 34 kt). Player
flight is built: the six powered-lift aircraft fly a rigid body on rotors, nozzles
with puffer jets, nacelles and an angle-of-attack wing; the stability levels,
trim, the nozzle and collective keys, the HUD cluster, hover hold, the Easy flight
physics cheat, trimmed airborne starts and the moving parts follow. It is
specified in [powered-lift flight](spec/powered-lift-flight.md) and measured in
the [overhaul baseline](baselines/vtol-overhaul.md). It awaits John's flight test
and the windowed battery. The second project, AI wingmen flying these aircraft
through the same inputs (decision 7), has not started; the AI still does not fly
them.

## Milestone 2: Multiplayer

**Goal:** players create and fly Quick Missions together through the existing
Quick Mission creator.

**Status:** delivered in [v0.2.0](release/v0.2.0.md) on 2026-10-23. Milestone 3,
single missions and campaigns, is in development.

Sequencing requested by John on 2026-09-23. Multiplayer starts here, then
expands in this order as the underlying modes are implemented:

1. **Quick Mission creator, M2.** Online lobby, shared mission setup and launch,
   and multiplayer quick fights.
2. **Campaigns, M3.** Extend multiplayer to campaign missions, progression and
   saves when the campaign systems are implemented.
3. **Single missions, M3.** Extend multiplayer to standalone missions after
   campaign multiplayer, using the implemented mission loader, objectives and
   results.
4. **Persistent campaigns, M6.** Carry multiplayer into the persistent campaign
   world when that milestone is implemented.

John wrote the M2 feature spec on 2026-09-28: co-op on the Quick Mission
creator, humans in open wing slots, AI in the rest, a server core that runs
dedicated or inside a player's game, a server browser with NAT traversal and
relay, host migration, rejoin and a flight data link. The design is in the
[multiplayer guide](MULTIPLAYER.md). The delivery stages, code findings and
risks are in the [multiplayer plan](multiplayer-plan.md), planning pass only.
Retail Fighters Anthology's own multiplayer is recorded in the
[retail multiplayer spec](spec/multiplayer.md).

Work, in the plan's stage order:
- A, B: one mission core outside the app, and seats so several humans fly in one
  mission and aircraft pass between AI and humans.
- C: the bug bash merged in (built 2026-09-29): the multiplayer commits rebuilt
  on main, with the bug bash's player rules applying to every human-flown plane.
- D, E: network foundation, dedicated server and player-hosted games. Then
  F-lite, the first playable checkpoint: two players flying co-op on a LAN.
  Stage D's design (reviewed by John on 2026-09-30) is in the
  [architecture guide](ARCHITECTURE.md#network-sessions).
- F: lobby through the Quick Mission creator.
- G: flight data link and radio backing, also in single player.
- H: exact checkpoints of the whole mission, which John chose on 2026-09-28 so
  host migration is seamless.
- I, J: master server, server browser and connectivity.
- K, L: host selection, migration, rejoin and compatibility hardening.
- Keep single player deterministic in headless simulation at 120 Hz. Networked
  play is host-authoritative, because results are not bit-identical across
  platforms.

Deliverable: players can join a session, configure a Quick Mission and fly it
together.

Exit: a multiplayer Quick Mission completed across separate clients, with
validation evidence recorded. Later mode extensions do not block M2 acceptance.

### Surface objectives and air defenses (2026-10-10)

John asked on 2026-10-10 for the next multiplayer round to cover the Quick
Mission Creator's surface objectives by theater, the import of the surface
objects and surface AI for AAA, flak and SAMs. That request is the explicit
authorization AGENTS.md requires for surface AI and autonomous surface firing;
his answers set the rules (manned slots, jittered and relocated sites, real
shells, batteries around a search radar, supply trucks, simulated parked
aircraft, one combined objective, friendly fire, Redfor defends in PvP). Built
on the `surface-ai` branch in slices R1 to A1:

- The ground target, AAA and SAM fields are live in all 16 theaters; the
  retail templates resolve with the retail rolls and equipment lists, nudged
  and (when not tied to a theater feature) relocated, with both sides' starts
  following the target ([quick mission menu](spec/quick-mission-menu.md),
  [surface defenses](spec/surface-defenses.md)).
- SAM sites, SAM batteries, AAA, flak, barrage zones, armed ships and armed
  vehicles fight with their retail records; the base layouts' 321 air-defense
  units fight on both sides; supply trucks rearm; columns and ships move;
  parked aircraft are simulated targets; destroyed units keep their wreck.
- The import carries the templates and every unit they name (`TORE_SURFACE_V1`).
- Splash damage for every weapon and a shockwave on large ground explosions.
- Objectives and the debrief count the ground target and SAM and AAA fire.
- Online under protocol 22 (host-authoritative, digest-checked, migration
  carries it); recordings under replay format 3.
- Real-size buildings and units (a third of the retail shape scale);
  runways, bridges and roads keep the map scale.
- Airports take their runway's side; only own-side fields are offered.

Evidence: the per-theater acceptance in
[surface defenses baseline](baselines/surface-defenses.md). The full battery
and John's in-game check close the round.

### Next rounds after the surface round

In John's order where he gave one; each is its own planned round.

1. **Air-to-ground player weapons.** Ground designation (A-G radar, FLIR,
   visual), CCIP and level-release bomb sights, laser designation and guided
   bombs, HARM boresight, Mavericks and anti-ship missiles, the bomb motion
   from the original, for every A2G store the imported aircraft carry. Planned
   (2026-10-10), not started; AI strike is excluded from it.
2. **AI strike and the ground weapons pass.** AI wings attacking ground
   targets on purpose (target assignment from the ground objective, release
   solutions, buddy lasing, SEAD with HARMs, the Alt+F wingman order) combined
   with the remaining ground weapon systems. John placed it after this round
   (2026-10-10); it needs his go-ahead to start (AGENTS.md).
3. **Real-scale airfields.** Built and the default (mp-fixes slice FX-A,
   2026-10-11, protocol 24): every airport redrawn at real size from its
   runway type's plan with the theater's own textures, markings, runway,
   approach and PAPI lights and about 12 extra targets per airport, in single
   player, the Quick Mission, missions, online and on the server; recordings
   keep the airfield scene so old ones replay on the retail airfields.
   `TORE_REDRAWN_AIRPORTS=0` keeps the retail airfields in single player for
   developers. What remains: [redrawn airports, limits](formats/redrawn-airports.md#limits).
4. **City fill.** Real-size scenery buildings filling the light areas of the
   city textures, on by default everywhere since the surface close-out
   (protocol 25; `TORE_CITY_FILL=0` turns it off in single player):
   up to 500,000 buildings drawn to 30 nm with separate graphics settings for
   building distance and detail; scenery, not targets.

#### Real-scale airfields and runway width

Runway pavement keeps the retail width, about twice a real runway's: John
deferred narrowing it on 2026-10-10. What a narrower runway would still need:
the strip mesh width, the runway's landable contact surface, the taxi paths,
ground-start slot spacing, the AI landing's lateral tolerance and the ILS
localizer width ([placed object scale](formats/objects-and-shapes.md#placed-object-scale-2026-10-10)).
The redrawn airports of item 3 (the default since protocol 24) replace the
pavement with real-width runways, so this applies only to the retail
airfields kept behind `TORE_REDRAWN_AIRPORTS=0`.

Deferred, with no round yet:

- **Redfor ground start online.** A Redfor human in a networked game starts
  airborne by the target; a ground start on Red's own airfield needs a
  mission flag the host sets in PvP, an enemy departure in the airfields, the
  parking of Enemy Wing 1 and Red's AI takeoff. The single-player creator's
  side-aware picker exists.
- **Dirt strip lengthening.** A future option John chose not to take
  (2026-10-10): the 22 dirt strips stay 1,074 ft short strips. Lengthening
  them to 3,500 or 4,000 ft needs no terrain work but three would cross water
  and one would cover a Roland site, and it needs a plan length for the runway
  and its ILS, the Quick Mission's strip list, the vertical-pad rule and a pad
  plan (findings in [redrawn airports](formats/redrawn-airports.md#short-strips-under-the-redraw)).

---

## Milestone 3: Missions and campaigns

This is the base game.  Tagged as 1.0.

Work:
- Importer: M, MT, and campaign structures across all Anthology titles, plus Pro Mission Creator files.
- Mission loader, triggers, objectives, scoring, and events.
- Briefing, map, and debrief at retail fidelity.
- Campaign progression, pilot record, saves.
- Extend the M2 multiplayer foundation to campaigns, then single missions, in
  the [multiplayer rollout order](#milestone-2-multiplayer).
- Carrier operations: launch, recovery, deck.
- Remaining aircraft imported and validated in batches.  Coverage table shows what is flyable per title.
- Encyclopedia and video playback where media is complete.

Deliverable: every retail campaign playable start to finish, with multiplayer
support for campaigns and single missions.

Exit: a full campaign completed per title, evidence recorded. Multiplayer
campaign progression, save/resume and single-mission completion validated across
separate clients. Importer coverage table shows no red cells for gameplay-critical formats.

---

## Milestone 4: Tools

Work:
- Aircraft and asset tools: import, inspect, validate, and export flight profiles, shapes, and textures.
- Mission editor as the successor to Pro Mission Creator, reading and writing its format.
- Audio composer plugin: users choose instrument banks and sound sets per category, audition, and save as a profile.
- Mod loader with manifests, load order, and a "classic" profile that disables everything.

Deliverable: a user can build and share a mission and a sound profile without touching code.

---

## Cannon cadence correction

Requested by John on 2026-09-21 and implemented with individual physical rounds
at the fitted source-derived host rate, plus one luminous tracer every three
bullets. Ammunition, target damage budgets, trigger boundaries and rendering are
validated together. Actor-owned release mechanics use the same shot spacing
without changing autonomous decisions. [Rules and known damage-rounding difference](spec/damage-smoke.md#individual-cannon-rounds);
[validation](baselines/damage-smoke.md).

## Milestone 5: Remaster layer

### Future terrain enhancements

Deferred by John on 2026-09-23 while retail map detail is completed:

- Optional close-range surface shaders for rock, soil and sand, with documented
  material rules, strengths and distance filtering. Keep source art, colors and
  an enhancement-off comparison. No shader trial is part of the current scope.
- Expanded landscape geometry, vegetation and additional scenery. These are
  authored additions needing a separate behavior/contact specification and
  approval of their visual direction. They are not recovered retail detail.


Everything here ships as toggleable layers over classic.

Work:
- Menus: modern layout option while keeping retail geometry available.
- Rebaked maps and atmosphere: real-elevation terrain pipeline returns here as an optional theater source.
- Cockpits and instruments: higher fidelity while keeping retail cockpit art selectable.
- Ambient cockpit and instrument lighting (John, 2026-09-23, deferred): cockpit art, instrument frames and screens follow ambient light, darkening in low light and brightening when the sun is behind the pilot and falls into the cockpit. Today the frames only follow the palette shared with the cockpit art, and no night or weather shift has been observed.
- Systems expansion: HARM and other rudimentary systems brought up to plausible depth, gameplay stats otherwise unchanged.
- Lighting, shadows, particles, vegetation, and basic cities, all at a light flight-sim feel.

Exit: classic mode still matches the M3 baseline with every layer off.

---

## Milestone 6: Persistent campaigns

Extend the multiplayer foundation from M2 and M3 to persistent campaigns, the
final stage of the [multiplayer rollout](#milestone-2-multiplayer).

Work:
- Authoritative server for a persistent combined-arms dynamic campaign with daily role selection: SAR, resupply, CAS, CAP, interdiction, strike, and escort.
- Ground-to-ground warfare as a campaign layer.
- Massive battle scaling, measured and recorded.

Entry condition: the sim has been deterministic and headless since M1.  If that is not true, this milestone becomes a second rewrite.

---

## Milestone 7: Custom maps and community theaters

Work:
- Terrain pipeline productized so users can build theaters from public elevation sources.
- Theater manifests, sharing, and validation.
- Asset expansion: community aircraft and objects through the M4 tools.

---

## Importer coverage table

Maintained in `docs/formats/coverage.md` and updated every milestone.  Rows are formats.  Columns are titles.  Cells are: not started, partial, decoded, validated in game.

Formats: ESA, LIB, DCL, PAL, PIC, FNT, DLG, MNU, LAY, XMI, MUS, 5K, 11K, T2, PT, SH, OT, JT, NT, HUD, M, MT, CB8, VDO, FBC, INF, campaign and save structures, Pro Mission Creator.

---

## Funding

- Donations only, for development tools and hosting.  Hard cap of $600 per month.
- Do not open until Milestone 1c is flyable by a stranger.
- Publish a ledger so the cap reads as discipline.
- GitHub Sponsors first, Patreon if the audience asks for it.  Money stays attached to the engine, never to the EA property.

## Open decisions

| Decision | Where it is made |
| --- | --- |
| ~~Retail AI VM reimplemented or behaviors recreated~~ | Settled 2026-09-15: behaviors recreated from a spec |
| Working title | Whenever, before M1 tag |
| TS repo stays runnable as reference or is archived at M0 | M0 |
| Multiplayer sequencing | Settled 2026-09-23 by John: starts in M2, see [rollout](#milestone-2-multiplayer) |
| Initial aircraft scope | Expanded to the twelve ported aircraft listed in M1c |
| Save format and mod manifest schema | M4 |
| Installer scope: mounted disc or folder, proper installers, unsigned first builds | Settled 2026-09-21, see M1g |
