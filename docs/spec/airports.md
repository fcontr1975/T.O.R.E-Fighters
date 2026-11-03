# Airports and attached ground objects

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation status, 2026-09-20. Base-layout placements, static scenes,
runway surfaces, individual targets and the deterministic landing service are
implemented. The HUD displays airport and runway names, range, and automatic
localizer/glide bars. The reviewed clear-to-land and welcome-home recordings are
connected to the player landing service. Campaign overlay application and
destroyed replacement art remain open.
[Input identities and measured extraction](../baselines/ukraine-airports.md).
[Placement contract](../formats/airport-placements.md).

## Placement and identity

Spec-derived target: each map displays its source airport runways and explicitly
placed surrounding objects at their authored positions and orientations, except
for the requested paired-field replacement below. Unpaired airports
retain source names; paired fields follow the single-field rule below. Buildings remain individually identifiable objects. Ukraine's
base-layout acceptance inventory is 14 runways and 99 airport-section objects.
Those section associations are research metadata, not a recovered gameplay link.
The eventual every-map scope includes base layouts and separately resolved mission
and campaign variants, with missing dependencies reported explicitly.

## Single fields replacing paired tiles

Opinionated, requested by John on 2026-10-15: replace each two-tile airport
with one custom-drawn field, visually and functionally. The
[84-pair inventory](../baselines/merged-airports.md) records every source
position and identity. The same rule applies to theater layout variants.
Standalone second-tile strips remain standalone.

Agent decision, 2026-10-15: one 12,000 ft by 200 ft runway per merged field,
with 220 ft paved overruns, 75 ft taxiways and nine parking slots. This is a
common-purpose military-field fit, not recovered retail behavior or a claim
that every aircraft can depart at every weight. Little Rock's
[12,000 ft runway](https://www.expeditionarycenter.af.mil/News/Article-Display/Article/2606357/lrafb-reopens-assault-landing-zone/)
and JBER's [10,000 ft heavy-aircraft runway project](https://www.afcec.af.mil/News/Article-Display/Article/3132125/air-force-awards-runway-extension-contract-for-jber-alaskan-geostrategic-hub/)
provide real-world scale references. The plan reuses the existing single-field
taxi and parking pattern and retail pavement materials.

The base half supplies the name and side. Both former IDs resolve to its one
runway, airport service and AI parking queue. Unrelated airport IDs do not
change. Old Quick Mission profile list positions migrate to that field;
new profile drafts identify the merged list revision. Mission text still
accepts the old runway ID. Old recordings retain their recorded scene and
geometry; new recordings identify airfield scene 3.

The preferred centre is halfway between the source placement origins, with
the base heading. A fixed nearest-first search on a 2,048 ft grid, up to
32,768 ft on either axis, chooses flat, dry ground within the theater. The
4,000 by 13,000 ft site is sampled every 500 ft, with equal-height terrain
cell corners and 350 ft clearance from unrelated object footprints. A second
pass checks water every 50 ft across the entire landable field, including
grass margins, so a narrow picture cutout cannot fall between the coarse samples.
An opaque land picture over a coastal cell is land, not a water cutout. This is
an agent-selected deterministic placement rule using the existing site
queries. Source airfield buildings move to the new building lines and use
the shared grounding rule. Failure to find a valid site is an explicit error.

Both old runway shapes disappear. Ground pictures under the source footprints
retain their own ground grain and replace opaque non-ground markings with
nearest local ground texels, preserving the original water cutouts and rotation.
The fitted [cleanup rule](../formats/theater.md#merged-airfield-ground-pictures)
avoids a differently colored replacement rectangle. Runway contact, lights, markings, ILS and navigation use
only the replacement field. The developer retail-airfield switch and recorded scenes
0, 1 and 2 retain their earlier art and airport layouts.

## Ground targeting

The manual's ground-attack tutorial cycles ground targets until a tower is selected,
shows its type and image in the Target Window, and attacks it with an AGM-65.
Mission-goal examples separately identify runways, a control tower and barracks.
Therefore airport structures must be independently selectable and able to serve as
mission targets when their definitions and mission settings allow it.

Unknown: exact runway and building detection rules, damage thresholds, destroyed
appearance, whether runway damage disables landing, and effects of tower loss.
Next research: decode each OT's relevant fields, trace target eligibility and
weapon-damage consumers, then follow airport availability after destruction.
Do not treat destroying a tower as destroying or capturing the entire airport.

## Landing guidance

Manual-described behavior: navigation mode provides automatic ILS guidance near a
runway. The horizontal indicator corrects height and the vertical indicator corrects
lateral alignment. Above-path guidance moves down; right-of-path guidance moves
left. Centered indications show alignment. Airspeed brackets show the advised
landing range. Manual page 65 gives the player first landing clearance while other
aircraft hold at marshal; AI traffic follows that rule as described in
[wing landing orders and player priority](#wing-landing-orders-and-player-priority).

Both pages 67 and 87 give a 5 nautical mile activation distance. Page 67 gives an
altitude below 2,000 feet; page 87 gives below 4,000 feet and requires gear down
and navigation mode. The retail altitude threshold is **unknown**. John selected
4,000 feet above airport ground level on 2026-09-20, allowing room for larger
aircraft in future. This is an opinionated user-requested TORE rule, regardless
of which retail threshold is eventually confirmed. ILS becomes altitude-eligible
at or below 4,000 feet above the selected airport's ground elevation, not mean
sea level or terrain directly beneath the aircraft. Other entry gates still apply.
The retail altitude reference, distance metric, runway selection, glide slope angle,
indicator scaling, speed bracket numbers and boundary inclusivity are also unknown.
Next step: trace the ILS activation/display consumers in the identified FA.EXE,
using those two candidate altitude values and existing airport geometry as leads.
Do not generalize a tutorial's aircraft-specific approach speeds to every aircraft.

### ILS arming envelope

John requested a 90-degree forward cone and ILS-band restriction after the HUD
checkpoint `cfd35e5`. The interpretation is a full 90-degree cone, inclusive
of 45 degrees from the aircraft's body-forward direction. Compare the real
three-dimensional nose vector with the line of sight to the chosen runway
threshold. Head-look, bank about the nose axis and velocity/sideslip do not
change which way the nose points. Pitch does affect the cone. Reject missing,
zero-length or non-finite direction data.

The existing geometric band is horizontal range at most 5 nautical miles,
altitude at or below 4,000 feet above the airport's ground elevation, and the
approach side of the runway threshold. Outside this band or the forward cone,
return no ILS guidance: neither `ILS ARM` nor active ILS indicators appear.
Inside it, existing NAV, gear-down and alive gates still control active guidance.
An eligible NAV approach with gear up may show `ILS ARM` while awaiting gear.
The 2.5-degree localizer and 0.7-degree glide values remain display scaling,
not newly invented capture limits.

Apply the same eligibility to explicit selections, cleared approaches and
automatic discovery. Explicit selection/clearance persists while out of the
cone or band and can regain guidance on re-entry. Automatic discovery filters
ineligible airports before choosing the nearest valid candidate, so a closer
airport behind the aircraft cannot hide one in front. Tower requests and
clearance behavior are unchanged. These are requested gameplay rules with
fitted geometry, not assertions about retail or real-world ILS receivers.

NAV INFO now offers distance-ordered selection of landing-eligible airports.
[Instrument controls and eligibility rules](weapon-navigation-selection.md).

## Commands and ownership

John clarified on 2026-09-20 that commands means landing/tower radio commands.
Unknown: supported player radio requests, responses and availability conditions. The reviewed STRIP definition
selects airport initialization, shared events and speech, and airport comments;
callback names alone do not establish a player command menu.
Next step: identify player input/menu producers and their visible responses.
Capture or transfer of airport ownership is not established by nationality fields.
Mission orders and ownership changes are outside this requested command scope.

### Wing landing orders and player priority

Manual p.65: "your aircraft always receives first landing clearance; all other
aircraft will hold marshal while you land." The recovered retail condition
([AI format notes](../formats/ai.md#airfield-takeoff-and-landing-sequences)) is
spec-derived: the player counts as landing, and AI aircraft landing at that
airport hold at marshal, while the gear is down, the aircraft is below 4,000 ft
above the ground, no faster than 953 ft/s and within 25,000 ft of the nearest
friendly airport. Friendly uses the tower's rule: a field of the human's own
side or a neutral one that grants permission ([Allegiance](#allegiance)). No
tower request is needed. The player must be airborne. Rollout and
taxi do not claim landing priority; runway occupancy is checked separately.
A departing player is excluded until leaving the condition or approaching a
usable runway within 30 degrees of its heading with its near end ahead
(fitted, agent decision 2026-09-23). It is re-evaluated every tick,
so raising the gear, climbing or flying away, a crash, ejection or restart
releases it. Agent decision (2026-09-23, fitted): an airport's distance is
measured to its nearest usable runway centre, since the scene has no single
airport position, and an airport with no usable runway is ignored. The
player's Shift-N selection also chooses where Alt-L (land at selected airport,
an opinionated addition John requested on 2026-09-23) sends the wing. The
wingmen use the player's cleared runway at that airport, otherwise its longest
usable runway, and share the tower's refusal of hostile, unknown and
unpermitted neutral airports (agent decisions, 2026-09-23). The AI approach,
marshal and landing rules are in [AI airfield sequences](ai-airfield.md); the
keys are in [input](../INPUT.md#player-wing-orders).

## Short strips

John decided on 2026-09-30 that the short airstrips take no part in ground
starts or in the tower's service. On 2026-10-11 he lifted the ground-start
part: they are labelled STOL and offered as ground starts
([STOL runways](#stol-runways)); the tower and AI rules below stand. There are 22 of them, each with a runway of
about 1,074 ft: Cuba (La Coloma, Al Turas, Camino De Tobac, Deleite, Real
Campina, Siguanea), the Falklands (Goose Green, San Carlos), Pakistan (Kotaddu,
Chor, Akli, Nagaur), Panama (Santiago, Santa Fe, Toncri, Crawford, Rio Sidre) and
the Persian Gulf (Bandar E Laft, Gahkom, Minab, Chah Logh, Fannuj). Every other
airport in the sixteen theaters has a runway of 4,060 ft or more, and the
imported `~` layout variants have no short strip.

**The rule (opinionated, John 2026-09-30; the number is `fitted`, agent decision
2026-09-30).** An airport is a short strip when its longest runway is under
2,000 ft (`tore_sim::airport::SHORT_STRIP_FT`). Any line between 1,074 and
4,060 ft picks the same 22 airports; 2,000 ft sits well clear of both. The
length is the runway's own (`Runway::length_ft`, measured from the STRIP anchor
to the far end of the shape), so the rule needs no list of names.

What follows from it:

- **The player.** The airport list that Shift-N cycles and the NAV page's airport
  mode leave the short strips out, so they can be neither selected nor chosen for
  guidance. The tower refuses to select one ("no airport selected") and gives no
  landing clearance or ILS for it, whatever the aircraft is doing: automatic
  guidance never picks one, and the tower's approach cues never work one. The strips are still drawn, still on the map, still hit by weapons
  and can still be flown over or landed on by hand.
- **The wing.** Land at the selected airport (Alt-L) never sends a wing to a short
  strip. If one were somehow selected the order is refused, "is a short strip
  your wingmen cannot land on".
- **The AI.** No AI aircraft on either side takes a short strip as its home
  runway, landing field or return point (the same list as a vertical pad, which is
  also nobody's home). See [AI home airfields](quick-mission-menu.md#home-airfields).
- **Ground starts.** Since 2026-10-11 a short strip is offered as a ground
  start, labelled STOL ([STOL runways](#stol-runways)). The automatic start
  (`start ground auto`) still never picks one.

The map keeps showing every airfield, since it marks landmarks, not services.
The developer `--headless-flight ... --ground-start N` takeoff and landing runs
still start on any airport, short strips included: they measure the flight model,
not a mission.

## STOL runways

John asked on 2026-10-11: "for short runways maybe we label them as STOL so
the player knows and then don't limit starts there." A runway shorter than
3,000 ft is a STOL runway (`tore_sim::stol::STOL_RUNWAY_FT`; the number is
`fitted`, agent decision 2026-10-11). The AI F/A-18D's takeoff ground roll
measured 1,930 to 2,170 ft in the slice CL-AP probes, and fast jets need about
2,000 to 3,000 ft at a light load, so under 3,000 ft only a STOL or vertical
aircraft has margin. Every retail field is either a 1,074 ft dirt strip or
4,060 ft and longer, so the STOL fields are exactly the 22 short strips.

- **Labelled.** The field reads `Name (STOL)` in the Quick Mission's airport
  picker, on the in-flight map, in the NAV page's airport list and in the
  dedicated server's `--check` runway list (`, STOL`). The NAV list leaves
  the short strips out (the tower does not serve them), so today no entry
  there carries the label; any STOL field the tower serves would.
- **Not limited.** The picker offers a STOL field of the pilot's side like
  any other, listed after the longer fields, so the default start stays a
  runway any aircraft can use when the side has one. The Falklands' Blue
  field, San Carlos, is a Blue ground start for the first time. Nothing
  stops a conventional aircraft starting there: the label is the warning.
  An AI wingman of a conventional type rolls off the end onto the grass, and
  the battery keeps its AI wings off these fields.
- **Unchanged.** The tower, the AI's homes and landing orders and the
  automatic start keep the short-strip rule above.

## Allegiance

Every airport belongs to a side (John, 2026-10-10, slice AL1). It takes the
side of its runway in the theater layout: the runway's owner field
(`nationality`, `nationality2` or `nationality3`, decoded as in
[surface defenses](surface-defenses.md#ownership-and-sides)) with the side bit
0x80 set is Redfor's, and clear is Blue's. A runway with no owner field is
neutral, and a neutral field serves both sides (agent decision, AL1). The scene
records the side from Blue's point of view (friendly is Blue's, hostile is
Redfor's); a Redfor pilot sees the two swapped. In the retail theaters every
runway has an owner, so no field is neutral today.

| Theater | Blue | Redfor | Short strips | Blue default ground start |
| --- | --- | --- | --- | --- |
| Baltics | 11 | 7 | 0 | Siauliai |
| Cuba | 1 | 11 | 6 | Key West |
| Egypt | 8 | 9 | 0 | Ras Nasrani |
| Falklands | 0 | 2 | 2 | San Carlos (STOL, since 2026-10-11) |
| France | 9 | 14 | 0 | Bournemouth |
| Greece | 7 | 5 | 0 | Athinai |
| Iraq | 5 | 8 | 0 | King Khalid |
| Kuriles | 1 | 3 | 0 | Berezovka |
| North Vietnam | 0 | 10 | 0 | none |
| Pakistan | 7 | 5 | 4 | Bahawalpur |
| Panama | 4 | 7 | 5 | Tonosio |
| Persian Gulf | 4 | 7 | 5 | Sharjah |
| South Korea | 8 | 8 | 0 | Kimpo |
| Taiwan | 8 | 3 | 0 | Pei-Chian |
| Ukraine | 5 | 9 | 0 | Simferopol |
| Vladivostok | 9 | 10 | 0 | Spassk Dalniy |

The Blue and Redfor columns count the longer airports; the short strips of
both sides are counted apart. Redfor has a field in every theater. Source-side totals after the 84 pair merges are recorded in the
[paired-field inventory](../baselines/merged-airports.md). The
`airport-allegiance` battery scenario checks the runtime lists
(`tore-app --airport-allegiance`; [lane menus](../testing/lane-menus.md)).

What follows from it. One rule decides everywhere: a pilot or an AI aircraft
may use its own side's fields and the neutral ones, never the other side's.

- **The tower.** The airport list Shift-N cycles, the NAV page's airport mode
  and automatic ILS guidance offer only those fields. A landing request at the
  other side's field is refused as hostile. A Redfor pilot in a PvP game is
  served the Red fields as its own.
- **The wing.** Land at the selected airport (Alt-L) is refused at a field
  hostile to the pilot who gives the order ("is hostile").
- **The AI.** Home runways, landing fields and return points are the
  aircraft's own side's or neutral, so in North Vietnam and the Falklands a
  Blue AI aircraft has no home and keeps its start point
  ([home airfields](quick-mission-menu.md#home-airfields)). A human's landing
  priority over the AI counts only those fields too.
- **Ground starts.** The Quick Mission offers only the player's side's and
  neutral airports, and in a theater with none Ground is locked to Airborne
  ([Quick Mission](quick-mission-menu.md#player-ground-start)). The dedicated
  server refuses `start ground N` at an enemy field, and its `--check` list
  names each runway's side. The mirror for Redfor (slice CL-R): Enemy Wing 1's
  ground start (`enemy-start ground N`) takes only Redfor and neutral fields.
- **Targets.** The target window and the lock box's friend-or-foe cue read a
  runway's side, so the other side's runways show as enemy targets.

Free flight has the same sides; its pilot is Blue. The developer
`--headless-flight ... --ground-start N` runs still start at any airport, as
they do on short strips. Multiplayer needs no message for it: every machine
reads the same layout and builds the same sides. Whether retail refuses an
enemy field is **unknown**; the rule is John's design for TORE, not a retail
claim.

## Integration choices

Proposed agent choice: use typed world instances, stable source identities,
render meshes and independent targeting/contact state within TORE's existing
120 Hz simulation. Reuse existing terrain grounding and reviewed runway anchors;
use the planned fitted defaults below where retail evidence is incomplete.
Imported callbacks remain inert. Existing flight adapters stay distinct.
There is no requirement to reproduce the original scheduler or global state.
[Staged implementation plan](../ROADMAP.md#airport-and-ground-object-expansion).

## Planned TORE defaults where retail is unresolved

Except for John's explicitly identified ILS altitude choice, these are agent-selected
proposals for implementation, not claims about retail or choices attributed to John. They make the plan executable without requiring full
reverse engineering. Later measured evidence can replace a fitted rule locally.

| Component | Planned rule | Provenance |
| --- | --- | --- |
| Building association | Explicit mission relation first; otherwise source airport comment groups as diagnostic/import metadata. Never propagate damage or ownership through a proximity guess. Preserve unassigned placements. | Opinionated host data model |
| Runway grouping | Unpaired fields keep placed identities. Confirmed pairs use one canonical runway and airport; old IDs redirect to it. | Opinionated |
| Grounding | Use TORE terrain height at a zero-Y instance origin and preserve authored heading; apply supported source pitch/bank. One rigid transform for rendering, anchors and contact. A runway's largest pavement layer meets the ground; roads keep the map's height; bridges use [shared bottom support](bridges.md); every other object stands on the lowest point of its shape (B13, 2026-10-12, and slice GROUND, 2026-10-13; [grounding](../formats/objects-and-shapes.md#grounding-every-object-stands-on-the-bottom-of-its-shape)). | Fitted host placement |
| Ground wind grip | Imported maximum-takeoff-weight crosswind thresholds, universal 10-knot tailwind limit and fitted rollout coupling; stationary tire grip is retained. Headwind has no ground-rule penalty. | [Runway-wind specification](runway-wind.md), opinionated user request 2026-09-21 |
| Runway support | Use reviewed runway extents and a plane through the grounded origin. Inside the surface footprint, aircraft contact and the visual runway use that plane. Terrain remains unchanged outside it. No automatic large-area flattening. | Fitted contact |
| Weapon contact | Weapons meet a runway object at a slab over its shape's footprint, from 2 ft below the runway plane to 1 ft above it, not at its whole-shape box, so aircraft and units on or beside the field can be hit; structures drawn inside a composite runway shape do not stop rounds. Aircraft contact and the landable surface are unchanged ([parked aircraft](surface-defenses.md#parked-aircraft)). | Lead ruling, 2026-10-10; fitted slab |
| Unresolved collision shapes | Use an oriented bounding box of the reviewed scaled mesh for buildings, never one huge sphere for a runway. Contact and blast rules are distinct. | Fitted collision |
| ILS entry | NAV mode, gear down, selected usable runway, horizontal threshold distance at most 5 nautical miles, altitude at or below 4,000 ft above the selected airport's ground elevation, and threshold inside the aircraft's full 90-degree forward cone. No armed indication outside that geometric envelope. Use 6,076.12 ft per nautical mile. | User-requested opinionated altitude/reference, 2026-09-20; agent-selected inclusive boundary; manual-derived NAV/gear/range gates; fitted distance metric and conversion |
| ILS path | Fitted primary centerline through source contact anchor 0x11, extending toward the projected shape's forward bound, with an opposite approach end. Fall back to mesh centerline only if that anchor is absent/outside the longitudinal footprint. A 3-degree straight approach whose aim point is the touchdown zone, 1,000 ft past the selected threshold on the runway plane (about 52 ft over the threshold; `airport::glide_path_height_ft`, `GLIDE_SLOPE_DEGREES`, `AIM_PAST_THRESHOLD_FT`, changed 2026-09-29, requested by John; AI approaches fly the same path, see [AI airfield sequences](ai-airfield.md)). Localizer displacement is lateral error divided by forward approach distance to the threshold; glide error is the elevation angle of the wheels (origin height minus ground clearance) over the aim point, minus 3 degrees, so it reads zero on the path, positive high and negative low. Suppress behind-threshold guidance and guard zero distance. | Fitted geometry |
| ILS display | Full-scale lateral indication at 2.5 degrees and vertical at 0.7 degrees error, clamped to the existing HUD area; centered at zero. | Fitted presentation |
| Runway selection | Explicit user selection persists. Otherwise choose the nearest usable threshold eligible for the ILS band and forward cone, breaking equal distances by stable ID; choose the approach end nearest ownship. No wind-based automatic switching during final. | Opinionated selection |
| Tower command set | Select airport, request approach/landing, repeat last reply, cancel approach. A reply identifies the airport and selected runway/end. | Opinionated player interface pending recovered menu evidence |
| Tower availability | An airport takes its runway's layout side ([Allegiance](#allegiance)); the service serves a pilot its own side's fields and neutral ones, which grant both sides permission, and refuses hostile and unknown ones as that pilot's side sees them. Disabled runways decline. | Requested by John 2026-10-10 (slice AL1); neutral permission for both sides an agent decision, AL1 |
| Clearance lifetime | Stays with the selected runway until cancellation, airport selection change, runway disablement, flight reset or landing completion. Repeating a request repeats status rather than allocating another clearance. | Opinionated |
| Landing completion | Existing flight state reports supported, alive, on-runway contact and speed below 30 knots for 240 consecutive 120 Hz ticks. Taxi remains manual. | Fitted service completion, not flight damage criteria |
| Radio output | Manual landing replies retain their recordings. Automatic player takeoff/landing cues and named wingman status reports share a paced, cancellable channel. Startup takeoff clearance, airborne, farewell, wind, touchdown grade and welcome use reviewed recordings. Taxi and marshal status remain text-only. Carrier-only cues require carrier operations. | [Airfield radio specification](airfield-radio.md), reviewed phrase identities with fitted host event binding |
| Runway damage | At zero imported hit points disable new clearance and ILS; preserve its surface for physical contact. Individual tower/building loss does not disable other runway services in this first host policy. | Fitted service consequence |
| Missing destruction artwork | Remove the intact mesh when combat HP reaches zero, retain target/mission identity, and use the existing impact effect. Do not infer an A-suffix replacement. A runway is the exception: it stays drawn at zero hit points, retail or redrawn, with the craters its bombs left (B15, 2026-10-12). Its shape is the whole field's pavement and, retail, the field's towers and hangars too, so removing it took the whole airport away; STRIP and DTSTRP have 750 hit points, so the splash of bombs on the buildings beside them was enough. | Fitted visual fallback; runway exception opinionated (agent, BG, 2026-10-12) |

Static geometry applies one scale to visible and contact geometry,
`terrain::placed_shape_scale`: the reviewed SH CODE header scale `2^(word6-8)`
for runways (`_STRIPProc`), bridges and roads, whose size is part of the map,
and a third of it for every other placed object, which puts buildings at real
size beside real-size aircraft (John, 2026-10-10; see
[placed object scale](../formats/objects-and-shapes.md#placed-object-scale-2026-10-10)).
`RUNWAY.SH` uses scale 4, putting its visible longitudinal span near -2748
through 3252 feet. That agrees with reviewed STRIP anchors near -2512 through
3090 feet and avoids an invisible support footprint. Runway pavement keeps the
retail width, about twice a real runway's (pending John).

Runway support and the ILS datum use the authored airport ground elevation.
Airport surfaces use no slope depth bias, and only one pass uses any constant
bias. A slope offset can pull pavement over an aircraft, at a grazing view or
from a high overhead view, and so can a constant one on the solid pass. The
textured detail pass is the single exception: it applies one reversed-depth ULP
(constant bias 1, slope bias 0), which resolves the equal-depth ties between
runway paint and the pavement under it, where the paint used to flicker in and
out. It moves no geometry and no contact surface. The terrain shader's
clip-space position is also marked invariant, so the solid and detail passes
compute identical depths for shared vertices and every tie resolves the same
way. Every other pass keeps zero bias. This is an agent decision, made for
John's 2026-09-30 report of flickering runway paint and first made on a
side branch. It was validated with the two ignored GPU regressions
(`cargo test --locked -p tore-app gpu_airport_ -- --ignored`: pavement never
hides an aircraft above it, and paint over pavement holds with distant and
moving cameras) and with the `render-runway-paint-calm` capture scenario in the
[flight lane](../testing/lane-flight.md#render-captures).
Solid surfaces draw before textured detail, with equal-depth samples allowed
so later coplanar art remains visible. Geometry still obeys ordinary depth
occlusion. Distant, moving views must keep pavement above terrain separated
by one foot, with no camera-dependent surface lift. The shared world renderer
uses reversed floating-point depth to retain that separation across its
existing camera near planes and 2,200,000-foot far limit. This is a fitted
host precision correction (agent, 2026-09-23), with airport placement and
flight contact unchanged.
For composite runway shapes,
the horizontal layer with the greatest aggregate polygon area defines pavement.
The mesh is translated vertically so that layer meets the runway surface, with
all relative geometry retained. For example, RNWY1's source paving at -4 feet
receives a fitted +4-foot mesh offset. This aligns pavement and wheel contact
without changing the airport-ground ILS datum. Building collision remains a separate
solid OBB query and never raises terrain to roof height. The restricted native
research adapter continues to reject unsupported ground contact.

Ground-target aim points use the upper half of the OBB while projectile and
aircraft contact retain the complete box. This fitted point keeps a flat runway
target above its own terrain occlusion without changing collision geometry.

Airport commands and NAV/gear/support state are queued at fixed-tick boundaries
and included in combat tape version 7. Versions 2 through 6 remain accepted.
Replay reconstructs selection, clearance, guidance gates and synchronized
runway damage from the same imported scene. Version 6 distinguishes a scene reset
from an explicit range-fixture reset; older tapes retain their range-only behavior.
Version 7 also records physical wreck-body presence separately from launcher health,
so guns and missiles reproduce impacts on a falling human aircraft.

Aircraft-specific ILS speed brackets and target-relative camera imagery remain
open. The [target window](target-window.md) shows the object camera, name, damage, bearing and range/speed.
Ground radar/infrared signatures and damage category come from the OT fields;
the existing host aspect/visibility model remains fitted. Unknown shape programs
stay manifest-only with a diagnostic, without invisible target/collision proxies.
Static geometry has a 32 MiB host input budget; oversized scenes fail explicitly.
Original PIC sheets larger than 256 pixels on either axis are nearest-sampled
into a 256-square GPU layer, with UVs adjusted to retain the entire artwork.
This is a fitted resolution reduction for the current shared texture-array path;
small sheets retain original texels, and extracted media remains unchanged.

The rendered terrain is split exactly at each oriented airport footprint and
recessed to at most one foot below its existing support plane. Boundary walls
join higher terrain outside the footprint. Intersections interpolate the
original texture coordinates and colors. Terrain outside the footprint, the
source height grid, the airport transform and flight contact remain unchanged.
This fitted rendering-only correction (agent, 2026-09-23) prevents low cockpit
views from exposing overlapping terrain through pavement. Ordered equal-depth passes separate coplanar airport details without
changing their depth.
Mesh scale comes from the reviewed SH header exponent and the placed-object
rule above: map-tied objects at the header scale, all other placed objects at
a third of it, the aircraft renderer's factor.

Landing speed brackets require aircraft-specific performance values. First use
reviewed PT/flight-model limits if their landing meaning is established. Otherwise
plan a fitted lower/upper bracket of 1.15/1.35 times the selected aircraft's current
landing-configuration stall speed. If that stall-speed input is unavailable, omit
brackets with an explicit diagnostic until it is provided; do not invent a shared
speed for all aircraft. This does not change aircraft physics.

Health and sensor configuration must come from reviewed OT fields. Visual loading
can proceed without them; interactive completion cannot silently substitute an
aircraft's configuration. An unresolved field gets an individually documented
fitted value and synthetic acceptance case during its implementation slice.
Tower commands provide guidance and responses only. They do not take control of
the player's aircraft, add autonomous airport traffic or guarantee a safe landing.
Airport speech is serial and has its own cancellation ownership. A new airport
reply replaces stale queued airport speech without removing wing radio. Cancel,
selection change, runway invalidation and flight reset remove stale airport
speech. Pause freezes it, and muting effects clears it with the other effects.
After landing, repeat replays the welcome reply, not an obsolete clearance.
Verified phrase/sample identity and airport-consumer evidence are recorded in
[radio metadata](../formats/radio.md).
