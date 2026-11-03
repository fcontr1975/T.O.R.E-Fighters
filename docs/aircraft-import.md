# Complete aircraft import and acceptance guide

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Powered guided store masses now resolve through the authored public/class
[missile data table](formats/missile-energy-data.md#loaded-aircraft-mass-changes).
Decoded retail weight remains available for inspection, compatibility effects
and radar-signature proxies. Carried mass and loadout totals use the resolved
mass; importing does not rewrite the user's media or alias aircraft variants.

The opinionated **F/A-XX** concept is also selectable as `--aircraft faxx` or
from Quick Mission. Since 2026-09-22 it uses the validated F-22N import
(`--aircraft f22n`, the stock carrier Raptor with its own hook) without extra
media. See the [variant contract](spec/fa-xx.md) for split flaps, the stowed
hook and handling limits.

Updated 2026-10-05. Start here when adding an aircraft. This guide joins the
existing extraction, format, simulation, presentation and systems contracts;
linked research remains authoritative for byte layouts and native behavior.
“Imported” is not synonymous with “fully implemented” or “retail validated.”

The [23-aircraft variety matrix](spec/aircraft-variety.md) now tracks the initial
ports of transports, gunship, airborne radar aircraft, powered-lift aircraft,
helicopters, fighters including all four F-4s, and airliners. There are 36 retail
import identities plus F/A-XX. Source roots are [reviewed](formats/aircraft-variety.md);
[flight validation](baselines/variety-flight.md) separates source configuration,
fitted handling and remaining visual/systems work. Milestones live in the
[roadmap](ROADMAP.md#aircraft-variety-import).

## Scheduled aircraft and execution order

John requested the F-14, A-4E and X-31 ports on 2026-09-16, using Fighters
Anthology sources throughout. The registered identities are now **F-14D
(F14.PT)**, **A-4E (A4E.PT)** and **X-31 EFM (F31.PT)**, alongside F/A-18D and
Rafale C. The initial ports support extraction, headless/rendered flight,
cockpits, fitted animation, source audio references and partial manual systems.
See the [behavior spec](spec/additional-aircraft.md) and
[acceptance record](baselines/aircraft-fa-expansion.md) for limits and checks.
The F-14 base model receives scoped runtime repairs for wing/tail attachment,
exhaust symmetry and six body seams. Its collapsed hook is replaced by the
loaded F-22N hook geometry and texture, fitted to the F-14 attachment. Both donor
resources are already present in the full-roster runtime import. These are intentional visual corrections
using the imported geometry and textures; source media and extracted files are
unchanged. [Repair contract](spec/additional-aircraft.md#fitted-exterior-behavior).

USNF-ATF supplies research guidance only. Its mixed-edition profiles and toolkit
SWPATCH F-14 exterior are not used. Exact wing-sweep flight effects, X-31 thrust
vectoring, damage/LOD/shadow shapes and complete systems parity remain open.
Quick Mission AI uses these exact imported profiles and their PT default weapon
and ECM records. Existing player flight adapter defaults remain unchanged.

The next requested batch adds **MiG-29 Fulcrum-C, Su-27 Flanker-B, MiG-21
Fishbed, Su-25 Frogfoot-A, MiG-23 Flogger-B, Su-35 and F-22A Raptor** as initial
player ports; the **F-22N Raptor** joined on 2026-09-22 as the F/A-XX donor.
All thirteen identities are selectable through Quick Mission and
`--aircraft`. [Roster spec](spec/roster-aircraft.md) and
[acceptance](baselines/aircraft-roster-expansion.md) document shared source
cockpits, missing PTS companions and fitted devices. The
[animation contract](spec/aircraft-animation.md) covers the subsequent surface,
airbrake, rigid-gear and bay pass; [engine materials](spec/engine-material.md)
list the reviewed round outlets.

## Current coverage and authoritative references

| Area | Implemented for F18 / Rafale C | Remaining work / reference |
| --- | --- | --- |
| Identity and extraction | Reviewed F18.PT = F/A-18D and RAFALE.PT = Rafale C; shared app/CLI dependency closure, source hashes, bounded readers | Other variants/layouts need review. [Extraction](EXTRACTION.md), [aircraft formats](formats/aircraft.md), [format coverage](formats/coverage.md) |
| Flight configuration | Separate models own validated typed configuration; source mass, engines, envelopes, controls/equipment and departure fields resolve at construction | Unresolved source fields stay explicit; no per-tick raw PT access. [Model contract](FLIGHT-MODEL.md) |
| Flight dynamics | Fixed 120 Hz shared integration, independent attitude/velocity, fuel/devices, legacy and selectable hybrid paths; hybrid departure/spin/contact helpers | Response and coupling components are fitted and acceptable as shipped; whole-tick native reconstruction is research, not an acceptance bar. [Native research](formats/native-flight.md) |
| G / roll / rudder / departure | Supported response/telemetry and hybrid stall attenuation/spin recovery validated for both identities/adapters | Restricted airborne native coupling is tested. Ground, terrain and object contact is opinionated authored behaviour, not a pending native producer; lifecycle work is sequenced in [the parity plan](parity-plan.md). [Flight-response plan](research/flight-response-plan.md) |
| Exterior and animation | Both original shapes/cockpits; separate aircraft animation mappings and fitted device travel | General native SH execution, exact hinges/schedules, LOD/shadow/damage and store-placement acceptance remain incomplete. [Shapes](formats/objects-and-shapes.md) |
| Cockpit / HUD / instruments | Original art/fonts, aircraft-specific glass clipping with cockpit art over HUD symbols, responsive overlays, rear mirror and asynchronous camera windows; supported live instrument channels | Additional cockpit images need glass-boundary review; exact retail composition and unmodeled system readings remain open. [Aircraft formats](formats/aircraft.md), [controls](FLIGHT-CONTROLS.md) |
| Environment / air data | Shared resolved wind, terrain/standard-atmosphere AirData, camera-local weather, physical turbulence and corrected own-shape vapor attachments | Native atmosphere, wake/contact/coupling, broader vapor and weather acceptance remain partial. [Wind/turbulence evidence](baselines/wind-turbulence-vapor.md) |
| Audio / feedback | Original engine/device audio, weapon/damage feedback, afterburner rumble, strong environmental turbulence rumble | Maneuver buffet and verified original dispatch are next; audibility/hardware acceptance is distinct from event tests. [Flight-response plan](research/flight-response-plan.md) |
| Weapons / sensors / damage | PT-default manual range, supported weapon/ECM/damage behavior; source stations and partial custom-load creator flow. One shared sensor component reads each aircraft's own SEE and ECM records, so a port reviews data rather than adding radar code | Current missile stores use four guidance profiles, inherited velocity, BORESIGHT and seeker feedback. Not full JT catalog or aircraft-system parity. [Missile validation](baselines/missiles.md). Sensor detection tuning is opinionated, not recovered. [Systems](baselines/weapons-systems.md), [sensors](baselines/radar.md), [creator/loadout](baselines/creator-ordnance.md) |
| Validation | Same aircraft flight suite, deterministic same-host probes, rendering and documented Linux checks | Retail trajectory, complete systems and unavailable platform checks remain separate gates. [Flight baseline](baselines/shared-flight-model.md), [progress](research/progress.md) |

Older first-pass sections in individual docs describe historical limitations.
Use their dated follow-ups and the current evidence above when assessing runtime
support. Missing telemetry must remain unavailable, not fabricated to fill a gauge.

## Per-aircraft implementation gates

### A. Identify the source and dependency closure

- [ ] Inventory the user's FA media; record exact displayed identity, PT name,
  schema/type size, archive/provider, source hashes and source-build distinctions.
- [ ] Review PT/object fields, G polygons, mass/fuel/thrust, equipment, default
  stores, hardpoints and referenced SH/HUD/SEE/ECM/JT/GAS/audio resources.
- [ ] Add the reviewed identity/profile to the shared resolver used by app and CLI.
  Preserve archive boundaries, safe paths, limits, conflicts and missing-reference
  reason chains; required unknowns fail rather than silently using another plane.
- [ ] Extend bounded readers only for reviewed layouts, using synthetic malformed
  and boundary fixtures. Update `formats/coverage.md` for actual reader coverage.
- [ ] Verify runtime cache refresh and CLI extraction independently. The app
  imports from user media into application data; CLI outputs are research files,
  not an alternate runtime asset bundle. Never execute imported native modules.

**Gate:** reproducible complete selected dependency report, including explicit
optional/unresolved references. Extraction success alone does not enable flight.

### B. Build the aircraft's own model

- [ ] Create a separate `tore-sim::models` implementation and complete typed
  configuration from that aircraft's reviewed source fields. Validate before
  replacement; no raw profile/string cache in update ticks and no guessed zeros.
- [ ] Review speed/G envelopes, loading, positive/negative G, roll/rudder authority,
  engine count/thrust/fuel, devices, departure/spin and contact limits.
- [ ] Apply the completed flight-response contracts to this identity and verify
  its own behavior. Shared helper code is reusable; aircraft calibration and
  configuration remain independently owned, each component carrying its own
  provenance label (spec-derived, native, fitted or opinionated).
- [ ] Review special control/propulsion modes from source. For F-14, investigate
  variable wing geometry and its flight/attachment consequences. For X-31,
  use the [reviewed low-speed auxiliary controls](spec/additional-aircraft.md)
  and departure behavior. Source paddle faces now follow the fitted plume demand; original animation
  schedules remain a research target.
- [ ] Preserve 120 Hz deterministic state, explicit wind/atmosphere/contact and
  clock/RNG inputs, independent movement/body attitude and both adapter boundaries.

**Gate:** the same headless suite runs for the new identity; differences are
explained by its reviewed configuration or documented fitted laws. No aircraft
is accepted by merely reusing F18's numbers until it happens to fly.

### C. Recover exterior, devices and attachments

- [ ] Project the aircraft's own SH with the bounded reader; review branches,
  materials, normals, UV/palette rules, scale and coordinate conventions.
- [ ] Build its own device/control-surface rig, using verified source structure.
  Never apply F18 or Rafale code offsets to a new shape. Document fitted hinges,
  schedules and unsupported topology instead of claiming native animation parity.
- [ ] Check neutral and deployed gear/flaps/brakes/hook and applicable aircraft-
  specific moving parts, plus engine-off/on and afterburner where supported.
- [ ] Validate CE vapor, gun/muzzle, hardpoint/store and other effect origins
  against that aircraft's own records, mesh and component transforms. Mesh uses
  right/forward/up; reviewed CE uses right/up/forward. Do not assume every record
  shares either convention. Check identity, yaw, pitch, bank and moving geometry.
- [ ] Record unimplemented LOD, shadow, damage and material behavior. Preserve
  original art and raw data; no retail derivatives in committed assets.

**Gate:** neutral/device and attachment captures plus numerical transform checks.
Run the actual drawing-path animation probe across signed controls, intermediate
and near-stowed gear, and combined controls. Validate complete wheel assemblies,
painted attachment points, thick forward edges and exact source endpoint
materials/triangle interiors. Transparent image corners are not mechanical
roots. Register the aircraft in the focused animation battery only after its
source and pose sheets are individually reviewed.
A correctly attached vapor trail does not establish emission/material parity or
validate store placement; each consumer needs its own evidence.

### D. Integrate cockpit, controls and telemetry

- [ ] Resolve the aircraft's own HUD/cockpit variants, fonts, mirror artwork,
  instrument window frame (`~<cockpit>_P.PIC`, which the HUD must name),
  instrument dependencies and device capabilities from source.
- [ ] Preserve cover-fit forward art, full-canvas world/HUD, independent instrument
  overlays, body-coordinate head-look and renderer-consistent projection.
- [ ] Consume actual typed telemetry and supported sensor/system state. Keep
  IAS/CAS, pressure/indicated altitude and unmodeled engine/system readings absent
  until their producers exist. No invented contacts or healthy-system values.
- [ ] Preserve source versus provisional input labels, modifier/release isolation,
  unsupported-device handling, pause/resume and aircraft-switch resets.

**Gate:** cockpit/exterior/mirror/panel and wide/tall captures, control release and
restart checks. Original cockpit art does not imply complete instrument parity.

### E. Connect effects, audio, stores and systems

- [ ] Connect the completed maneuver-feedback contract to this aircraft's actual
  G/rates/rudder/departure state and its own source gates; verify sustained rumble,
  release and device/mute/pause lifecycle without changing physics or RNG.
- [ ] Verify engine/start/stop/actuator and maneuver audio through original resource
  references and dispatch. Record any authored haptic mapping separately.
- [ ] Resolve weapons, ammo, stations, compatibility and supported damage behavior.
  Test actual launch/muzzle/attachment transforms and supported loadout mass/drag
  effects; extracted definitions do not enable unsupported weapons.
- [ ] Review `--sensor-summary` for this aircraft: its radar, infrared, visual and
  ECM records, volumes, look-down coefficient and assigned preset and jammer
  generation. An unreviewed radar or ECM record fails the import on purpose;
  assign it explicitly in the [component guide](radar.md) rather than borrowing
  another aircraft's. Do not write aircraft-specific sensor code.
- [ ] Keep ordinary free flight externally clean. Validate deliberate range and
  creator loadouts separately, including fuel/weight, restart and replay boundaries.
- [ ] Add creator selection only with honest capability validation and reviewed
  aircraft metadata/art; avoid exposing a selectable but incorrectly configured plane.

**Gate:** supported equipment and default-load scenarios, original audio checks,
controller checks where hardware exists, and explicit unavailable systems.

### F. Acceptance record and support status

For each aircraft, create `docs/baselines/aircraft-<reviewed-id>.md` with:

- Exact identity/resources/hashes, import commands and dependency report location.
- Implemented versus translated/fitted/unavailable subsystems and open decisions.
- Same flight-suite results plus G/roll/rudder/departure/buffet probes; level,
  pull/push/turn/loop, roll/rudder release, stall/spin/recovery, wind, fuel and
  explicit-runway/contact cases. Never infer safe runways from arbitrary T2 height.
- Aircraft-specific controls/devices/equipment tests and matched retail evidence
  when available, kept distinct from host regression tests.
- Visual captures, sound/controller observations, platform/driver/hardware and
  bounded performance results for flight-performance changes.
- Formatting, Clippy with warnings denied, tests/build `--locked`, Python and asset
  guard results; creator/viewer and flight/camera GPU checks for rendering changes.
- Deterministic headless replay and feedback/camera independence, pause/restart,
  aircraft-switch state isolation and failures without stale mutable state.

Use the terms **source reviewed**, **extraction supported**, **headless flight
supported**, **rendered flight supported** and **systems partially supported**
independently. Add **retail validated for named cases** only where that evidence
exists; retail comparison is currently unavailable. These describe support, not
origin: a spec-derived, fitted or opinionated component can be fully supported.
Record unavailable checks; do not collapse these into a single “complete”
checkbox. Update this guide,
`formats/coverage.md`, `FLIGHT-MODEL.md` and `parity-plan.md` with actual results.

## Worked variety import examples

These examples describe the implemented initial ports. Use the linked specs
and validation, not real-aircraft assumptions, when importing the next type.

### C-130 and other unarmed transports

1. Review the exact PT identity and schema, source quantities and full
   dependency closure. C130.PT names AC130.HUD and C130.SH; its missing PTS
   and cockpit overlays are explicit exceptions in
   [the source inventory](formats/aircraft-variety.md).
2. Register the identity in `tore-formats::aircraft::AircraftId`, including
   exact PT/HUD/shape/cockpit names and installed gun/radar lists. An empty
   gun list and absent radar are supported states. Do not add a placeholder
   weapon or radar to satisfy old callers.
3. Add its independently owned configuration through
   `tore-sim::models::variety`. Reuse the shared integration and response laws,
   retain the aircraft's own source fields, and document every tuning choice
   in the [flight contract](spec/variety-flight.md). Derive ground clearance
   from its deployed geometry at the host scale. Compare the decoded 1 G top
   speed and the simulated level speed (`tore-sim` example `envelope_probe`)
   with published figures; a clearly wrong edge gets a fitted correction in
   `variety::fitted_envelopes`, recorded with its source under
   [top speeds](spec/variety-flight.md#top-speeds).
4. Review the shape layout and device branches with the bounded
   `shape_inspect` example. The app's `variety_animation` descriptor checks
   those measurements before animating. Propeller/rotor phase selection and
   fitted hinges live in `variety_rotors`; do not copy another shape's offsets.
   [Animation contract](spec/variety-animation.md).
5. Check source HUD and panel names, transparency, equipment indications and
   exterior/deployed captures. A shared cockpit supplies art, not another
   aircraft's sensors. No-radar instruments must report absent equipment.
6. Run the source-backed flight probe, application headless flight, source
   capability summary and wrapped rendering checks. Add the identity to the
   appropriate battery family and record the evidence.

For an already reviewed and extracted C-130, from the repository root:

```sh
cargo run --locked -p tore-sim --example variety_flight -- .local/aircraft-variety/FA_2.LIB/C130.PT
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --aircraft c130 --headless-flight 1200 --no-audio --no-controllers
TORE_DATA_DIR=.local/dev-profile tools/agent-run.sh cargo run --locked -p tore-app -- --aircraft c130 --free-flight --flight-view 2 --flight-devices 1,1,1,0,0 --capture-flight .local/c130.ppm --no-audio
```

The C-130 capture sets the hook fraction to zero because it has no hook.
The application profile must already be imported; CLI extraction remains
research data, not an alternate runtime pack. The
[headless setup guide](DEVELOPMENT.md#headless-development) explains isolated
profiles and runtime import.

### F-4 variants and stores

Keep all four exact identities. B/J can share reviewed shape and handling
algorithms, while E/G retain their own shape and differing configuration.
Compare values as in the [F-4 source review](formats/aircraft.md#f-4-variant-comparison),
then test each profile. Identical G envelopes do not justify replacing mass,
fuel, rudder, departure, radar or weapon data with the newest variant's values.

Treat installed hardware and ammunition separately. The F-4J's source SUU16
is a gun pod, not an internal gun or a single expendable bullet; the F-4B's
centreline row can take the same record as an option
([the Phantoms' gun pod](spec/stores-on-pylons.md#the-phantoms-gun-pod-2026-10-16)). The same
distinction governs [external tanks](spec/external-fuel-tanks.md): selected
count, empty equipment mass and fuel/ammunition are different quantities.
Accepted empty selections must survive mission creation, restart and network
loadout serialization.

### Powered lift and special systems

The [powered-lift flight spec](spec/powered-lift-flight.md) supplies the VTOL,
tiltrotor and helicopter laws and all fitted constants. Use source limits where
reviewed and fit missing values explicitly. Airborne starts are trimmed forward
flight (a hover only for the probes), computed from the final mission altitude,
fuel and payload; hover hold is an autopilot mode the pilot chooses. The source
probes (`variety_flight`, `powered_probe`, `jet_probe`) exercise hover, landing,
lift-off, conversion and the transition.

New controls must be available in the in-game editor, saved profiles, input
tapes and network input. Put actual device positions into snapshots and
recordings so remote and replay animation follows the same state. Update the
[master controls](CONTROLS.md) and [keyboard map](tore-keyboard-map.html)
only from implemented defaults.

For a new system family, follow the [AC-130 contract](spec/ac130-linked-guns.md):
source gun records, explicit fitted aiming limits, individual ammunition and
cadence, shared simulation/render muzzle transforms, and tests for blocked
fire as well as successful firing. Importing references alone does not
implement a special capability such as AWACS contact sharing.

### Individual surface acceptance

John requested aircraft-by-aircraft headless animation review on 2026-10-05.
Before broad creator or flight validation, run the selected aircraft through
`--animation-probe` and inspect its actual surface poses. Successful imports,
finite flight and branch counts do not establish animation completeness.
Require signed pitch/roll/yaw response, opposite roll surfaces, shared hinges,
coherent upper/lower skins and intermediate device travel. Review gear wheel
separation, roots and stow placement densely, including a near-zero pose before
gear disappears. Reject new polygon crossings and verify that closing a hook
raises its blade. Preserve source neutral geometry;
a pre-deflected branch is not a neutral pose. Record fitted control assignments
and motion constants in the behavior spec, and keep unknown mechanisms explicit.

Inspect the original atlas before assigning gear joints. Many wheel/strut
assemblies are transparent panels; their rectangle corners are not separate
mechanical attachment points. Preserve painted wheel dimensions, check the
actual painted root, and compare stowed geometry against both sides of the
neutral fuselage envelope. An out-of-plane bend that merely defeats a planar
intersection check does not establish a coherent mechanism. Reports labeled
`motion-survey` have not passed aircraft-specific attachment acceptance.
Check rotor spin in the original panel plane, combined cyclic/phase poses and
tandem separation. Audit always-present gear as well as switched additions,
both for retraction ownership and the ground-contact low point.

The [audit](baselines/aircraft-animations.md) and
[A-7 worked contract](spec/variety-animation.md#a-7-surface-repair-and-acceptance)
show the first per-aircraft repair and its limitations.

The historical `variety_animation` module now also registers individually
reviewed older profiles. `additional_animation` delegates those profiles once,
preserving their existing source scale. Preserve both face preparation and
per-face animation dispatch: bay recesses and concept split leaves may produce
multiple faces before animation. Already posed split leaves must not rotate a
second time. Check existing bay geometry at every opening fraction when moving
a profile into this registry. Do not apply the older generic surface
transform again. Aircraft-specific devices run before shared propeller/rotor
and player-directed gun overlays. Flight adapters remain a separate concern.

## Commands and implementation entry points

For an aircraft with selectable external tanks, review its exact GAS record and
the original hardpoint's compatibility/count fields. The shared loadout keeps
tank identity and installed quantity separate from weapon ammunition. Review
internal fuel, external fuel, empty shell mass and total mass independently.
Use the tank cards at the top of the air-to-surface catalog to remove or reload
a tank. Both catalogs share the same
source-numbered station rows, so a weapon and tank on a shared mount cannot
appear as separate available positions. New accepted loads record an
explicit empty tank list when all tanks are removed; missing tank lists in older
saved missions preserve source defaults. `cargo run --locked -p tore-app --
--validate-tanks --aircraft f14 --no-audio` is a headless worked regression for
zero, one and two F250 tanks and accepted-load restoration. See the
[tank contract](spec/external-fuel-tanks.md) and
[validation](baselines/external-fuel-tanks.md) for source values and limits.

Selective aircraft/combat extraction includes the original `$<GAS stem>.PIC`
thumbnail when it exists in any supplied archive and records a
`generated-tank-thumbnail` edge. An absent optional icon remains an explicit
unavailable edge. Broad runtime menu imports can already carry these icons;
this dependency rule fills the selective CLI closure and does not infer tank
SH resources or physical aircraft attachments.

Audit every source hardpoint, including empty and default-GAS slots, against
implemented weapons. A default `.JT` is not the criterion for an editable weapon
mount. The [ordnance source matrix](formats/aircraft-ordnance.md) distinguishes
logical station groups, installed quantities and unsupported stores. Keep new
optional rows empty by default and preserve old accepted station indices.

Aircraft-only profiles also include two explicit host retained-selection seeds:
`MIG17F.PT` hardpoint 3 needs `MK82.JT`, and `MIG23.PT` hardpoint 5 needs
`AIM9M.JT`. These zero-based rows retain a compatible selection at zero initial
quantity; the seeds add no default ammunition or mass. The report labels them
`host-retained-station-seed`, distinguishing this host requirement from a source
PT pointer. Missing required seed definitions fail the import with a reason.

With the imported development profile, run `cargo run --locked -p tore-app --
--validate-ordnance --no-audio` for all 36 reviewed retail aircraft. It checks
source compatibility coverage, default and older-load preservation, weapon/tank
replacement, accepted-load restoration and lightweight multiplayer validation.
The battery scenario is `menus-ordnance-availability`. This validates current
source data and host rules, not a live comparison with the retail game.

Supported extraction/flight validation commands:

```sh
python3 tools/extract_assets.py --aircraft f18 --exclude-archive 'disc1/LHX/*' --out .local/aircraft/f18 --validate-flight
python3 tools/extract_assets.py --aircraft rafale --exclude-archive 'disc1/LHX/*' --out .local/aircraft/rafale --validate-flight
cargo run --locked -p tore-app -- --free-flight --aircraft rafale --researched-flight
```

The LHX exclusion only skips unrelated bundled media; adapt source selection as
specified in [EXTRACTION](EXTRACTION.md). Register and document new CLI identities
only after source review. The reviewed additions are `f14`, `a4e`, `x31`, `mig29`, `su27`, `mig21`,
`su25`, `mig23`, `su35`, `f22` and `f22n`.
For FA-only CLI extraction, exclude `swpatch.lib` and unrelated disc archives,
as shown in the [acceptance record](baselines/aircraft-fa-expansion.md).

Start with `tools/extract_assets.py`, `crates/tore-extract`, the shared
`tore-formats` aircraft/schema/resource-selection code, `tore-sim::models` and
its flight-suite example, then the app aircraft/animation/cockpit/system consumers.
Keep formats and simulation independent of rendering and preserve the shared
app/CLI resolver. Detailed commands and host checks live in
[DEVELOPMENT](DEVELOPMENT.md).

The optional user-supplied [engine material](spec/engine-material.md) replaces reviewed burner
face materials at runtime. Its throttle glow is separate from the retail atlas
and does not change A-4E presentation or flame geometry.

Researched flight is now the default; `--legacy-flight` preserves the previous
model. HUD and audio share the [stall warning signal](spec/stall-warnings.md),
including the original imported warning samples.

[Pilot escape](spec/ejection.md) reads seat availability from each aircraft's
PLANE flags and uses the shared original pilot/seat/chute resources at runtime.
Refresh older caches to include the optional [ejection media](formats/ejection.md).
The pilot remains separate from the abandoned aircraft and its damage state.

Quick Mission lists only supported aircraft with parsed imported flight profiles,
not the wider metadata catalog. See [selector behaviour](spec/quick-mission-menu.md).

The runtime now reads the already-imported A/C damaged-body SHs, B/D pieces and their own PIC
references for every supported aircraft. Missing damage resources produce a
re-import error rather than substituting a different aircraft. No retail bytes
are embedded. [Resource evidence and remaining variant questions](formats/objects-and-shapes.md#combat-damage-and-smoke-resource-review).

Aircraft use the shared warm, continuous surface lighting and geometric shadows
in smooth weather. See the [surface spec](spec/surface-lighting.md) for coverage,
night lighting and transparent-material exceptions. Imported shadow shapes remain
unresolved and are not required by this geometry-based presentation.

The guarded F-15C and F-16C [derived stations](spec/stores-on-pylons.md#derived-f-15c-and-f-16c-stations)
are appended in memory after parsing. Their tank records are authored at pack
load. They need no reimport, keep source indices and default loads, and do not
modify user media. Edited station records suppress the additions.

The [optional carriers added on 2026-10-16](spec/stores-on-pylons.md#optional-carriers-2026-10-16)
extend the same mechanism to F22.PT, MIG29.PT and RAFALE.PT, and enable the
F-15C centreline bomb rack. F22N.PT and A10.PT remain unchanged. New authored
tanks are supplied only with their carrier; no reimport is needed.
