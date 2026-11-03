# Load Ordnance presentation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation specification, 2026-09-16. The supplied retail screenshot guides
presentation; its executable identity is unknown. Recovered card positions and
loadout interactions remain documented in the [ordnance contract](../formats/ordnance-menu.md).
Store placement, fuel steps, launch validation, flight adapters and combat
behaviour retain their existing contracts. Catalog visibility is specified below.

All two-sided weapon and tank points now show the even maxima and change by
pairs under the [even-paired-load rule](stores-on-pylons.md#even-paired-loads).
Single points keep their existing quantities and steps. Restored singles become
pairs; larger odd loads round down. Overweight saved loads remain open for adjustment, and Fly
retains its existing refusal until the weight is fixed. Current multiplayer
requests must already obey the rule.

## Catalog availability

John requested this opinionated visibility rule on 2026-09-23: hide weapons
whose flight behaviour is not hooked up yet, including when Cheat is enabled.
An imported definition alone does not make a weapon available. The catalog uses
the same supported-weapon list as flight launch validation.

With Cheat off, show a supported weapon only when at least one station on the
selected aircraft can load it, with an editable capacity of 1 through 32,766.
With Cheat on, show every imported supported weapon, regardless of aircraft
compatibility. Station placement still follows the existing Cheat rules,
including fixed-station restrictions. Visibility does not bypass launch weight
checks, sensor requirements or weapon firing limits.

Toggling Cheat still unloads every station. Rebuild both catalog categories,
reset their pages to the first page and clear the catalog selection. Keep the
existing display-name ordering and eight cards per page; an empty category has
no selectable cards and its page rocker stays on the first page. Unsupported
weapons return only after their flight behaviour is implemented and validated,
following the [weapon update passes](../ROADMAP.md#weapon-catalog-update-passes).
Validation is recorded in the [catalog baseline](../baselines/ordnance-catalog.md).

John's retail reference and correction on 2026-10-05 put compatible tanks
first in the air-to-surface catalog. Retain exactly two categories controlled by
the original dial: air-to-air and air-to-surface. Tanks precede the supported
surface weapons, ordered by their display names (150, 250, 350, 500 gallon for
the reviewed catalog), and share the existing eight-card paging. Preserve the
existing weapon ordering after them. Do not show a separate Tanks button or the
added external-fuel/tank-shell banner above the station area. The supplied
reference's executable identity is unknown; it establishes this presentation
request, not broader source-build parity.

Tank quantities are installed equipment, not ammunition. Use original tank
thumbnails where imported. Catalog cards show the tank name and full loaded
mass in pounds, using the same mass-label format as weapons. The station view
stays the same in both categories. Internal fuel remains in its original panel;
vehicle weight still includes selected external fuel and shell mass. Existing
plus/minus and right-click gestures change one installed tank. The internal-fuel
rocker keeps its 500 lb step. Unload All and toggling Cheat clear both kinds of
equipment. Rules, persistence and geometry limits live in the
[tank specification](external-fuel-tanks.md).

## Air-to-ground stores

The air-to-ground round (plan decisions 13 and 16) put the free-fall, retarded,
pack and cluster bombs and the LAU-10 rocket pod on the Load Ordnance page.
The flight behaviour of each is in [air-to-ground](air-to-ground.md#bomb-motion):
bombs use the shared fitted gravity and drag model with plus or minus 1.5 degree scatter,
a pack releases all its bombs in one press, a rocket pod holds its real rocket
count. Provenance: the records are retail data; listing them is spec-derived
from the catalog rule above (a record is listed once its flight behaviour
works), and the Cheat-only rule and the bomblet flashes below are opinionated.

| Added to the supported list | Records |
| --- | --- |
| Free-fall bombs | MK84, FAB250, FAB500, FAB1000, MK82P3 (3-pack) |
| Retarded bombs and packs | MK82HD (Snakeye), MK82P (6-pack), MK82AIR (7-pack) |
| Cluster bombs | CBU87, CBU89 (labelled CBU-97), MK20 (Rockeye II), RBK250, RBK500 |
| Rocket pod | LAU10 |

With the 40 records supported before, the list holds 54. MK82 itself was
already listed. Rows carry no change to any aircraft's default stations: each
aircraft's flyable defaults are still its reviewed set, so these stores load
only through the page, a saved loadout or Cheat. The other air-to-ground
records (guided bombs, JDAMs and the remaining missiles) join
the list in the slices that make them work.

### Laser designator pods

The Pave Spike (`PAVESPK`) laser pod loads on any pod station, the retail
equipment rule T1 implemented (decision 7; [air-to-ground](air-to-ground.md)).
The page lists it in the air-to-surface catalog after the tanks and before the
weapons, as a card with the original `$PAVESPK` thumbnail, the pod's name, its
weight and "laser" in the seeker corner. Normal loading lists it when some
station of the aircraft can hang it; Cheat loading lists it whatever the
aircraft.

**The Pave Knife is no longer offered** (John, 2026-10-16: its data equals the
Pave Spike's, 425 lb, 10 nm laser, pod hemisphere, so it "really adds nothing").
It is gone from both catalogs, single player and the lobby's page. Old data
still loads (agent decision): a saved quick loadout, a `loadout` or
`plane-loadout` mission line or a lobby request that names `PAVEKNF.SEE` loads
the Pave Spike on the same station instead of failing
(`tore_sim::combat::sight::pod_record`). Replays and checkpoints that recorded a
Pave Knife are not touched and play as before.

**A station holding a pod blinks.** Its box outline alternates between the
normal red and yellow, two full cycles a second: red for 250 ms, then yellow for
250 ms (opinionated, John 2026-10-16). It runs on the page's frame clock, the
time since the page opened, and the page asks for frames while a pod hangs. A
preview or test fixes the clock, so the blink holds one phase and renders
repeat (`ordnance-pod` is the red phase, `ordnance-pod-yellow` the yellow).

A pod takes the station's place: loading one leaves the station's weapon
unloaded (and a tank on the same mount off), and a weapon or tank taking the
station back removes the pod. An aircraft carries one pod: loading a second, or
dragging the loaded one to another pod station, moves it. The station shows the
pod card and "1 loaded (max 1)"; a click on it adds nothing, a right-click or a
drag onto the catalog takes it off, and Unload All and the Cheat toggle take it
off. A pod hung or taken off plays the weapon cue (opinionated, agent). The
lobby's page offers the pods too since protocol 23 (slice NA): a pod not every
player has is dimmed and refused with the host's words, as a weapon is. The saved loadout names the pod's
SEE record on the station's row with none of the station's weapon, so the
mission the page builds, a saved mission text and the checks of
the loadout spec all keep it (agent decision). The ordnance audit loads each pod on every
station that takes it and restores it through the saved mission.

### Novelty bombs are Cheat only

Boulder (`~BLDR`) and Moose (`~MOOSE`) load on the same stations as ordinary
bombs and fly as free-fall bombs. John decided on 2026-10-10 that they are
Cheat loading only (decision 16, confirmed; opinionated). The two other
novelty records, Moth Balls (`~MOTHB`) and Vomit (`~VOMIT`), stay hidden:
nothing loads them.

```mermaid
flowchart TD
    R[Weapon record] --> N{Boulder or Moose?}
    N -->|no| S{In the supported list?}
    S -->|no| H[Hidden in both modes]
    S -->|yes| W{Cheat loading on?}
    W -->|no| F[Listed if a station of the aircraft takes it]
    W -->|yes| A[Listed whatever the aircraft]
    N -->|yes| C{Cheat loading on?}
    C -->|no| X[Hidden; no station gives it capacity; a loadout carrying it is refused]
    C -->|yes| A
```

The rule lives in the loadout, not only on the page: without Cheat loading no
station has capacity for either record, selecting one is refused, and a loadout
that carries one (a saved mission, a lobby loadout) fails validation with
"This store is available with Cheat loading only." Turning Cheat loading off
unloads every station as it already does, so a novelty bomb cannot survive it.
The page's catalog lists the two records only in the Cheat catalog, on every
aircraft, with the other supported records. Novelty records are not in the
supported list the ordnance audit counts; the audit checks them separately
(`--validate-ordnance`: every aircraft refuses them without Cheat loading and
the Cheat catalog lists them).

### Card seeker label

A catalog card's second line shows the weapon's weight and a seeker word. The
word follows the retail label list: "unguided" for a record without the guided
flag (every free-fall, retarded, pack and cluster bomb and the rocket pods);
otherwise the seeker signature, 0 "optical", 1 "laser", 2 "IR", 3 "SARH" with
the semi-active flag or "active rdr", 4 "passive rdr" (retail data,
[missile formats](../formats/missiles.md)). The [shared classification](missiles.md#shared-classification)
also applies its two existing surface exceptions, SA-19 and SA-N-11, so their
cards say SARH consistently with their supported flight behavior. Signature 0 guided records are the
JDAMs and the AT-2, so all of them read "optical". The AT-2 read "unguided" while
it flew unguided; it reads "optical" from this round on, and the card
describes the record, not today's flight model.

### Cluster bomb flashes

A cluster bomb does its retail damage: a wide splash, 2,200, 2,500, 1,600,
1,500 and 2,500 ft for CBU-87, CBU-97, Mk 20, RBK-250 and RBK-500, falling in a
straight line from the record's damage at the burst to nothing at the edge
(70 percent of it for a ground burst), no crater. The retail game draws the
burst as one small flash, and in a one-machine session about ten bursts in
eleven as nothing at all (the 0x800000 flag; [weapon formats](../formats/weapons.md#cluster-flag-0x800000)).
The remake adds a presentation-only scatter of bomblet flashes so a cluster
bomb looks like one (opinionated, agent, decision 13). The flashes do no
damage, add nothing to the simulation, a checkpoint, the wire or a replay, and
every client draws the same pattern, because it is derived from the burst's
explosion effect alone.

- **What counts as a cluster burst.** A ground impact whose explosion type is in
  the type 18 family (18, 19, 20, 24, 25, 26, 28, 29, which is type 18 and the
  variants the original rolls for it). The five cluster records are the only
  weapons whose land and water explosion is type 18 (guns name 15, bombs and
  missiles 21 and 35), so the burst identifies itself. A cluster bomb that
  strikes an object directly makes a plain hit effect, which a gun burst on an
  aircraft also makes, so it draws the single retail flash and no scatter.
- **The pattern (fitted).** 28 flashes, scattered evenly over a disc of 1,200 ft
  around the burst (inside the smallest cluster splash, 1,500 ft), each at a
  place, start and size taken from where the burst is, so the same burst always
  draws the same pattern. Each flash starts at a random tick within the first
  half second and plays half a second, so the last ends with the burst's one
  second. A flash draws type 18, 19 or 20 (`AIRSML`, `AIRSMLA`, `AIRSMLB2`)
  standing on the burst's ground level, 1.5 times that type's rolled width
  (38 to 120 ft). Ground height is the burst's, so on a steep slope a flash can
  sit a little above or below the true ground.
- **Not shown.** Over water the flashes are the same orange flashes, not
  splashes. A missing sheet skips its flashes only.

The scenario `a2g-cluster` flies a CBU-87, a Mk 20 and an RBK-500 onto an armored
column and checks the splash rule and the flash pattern; `a2g-catalog` runs the
ordnance audit in both catalog modes. `--blast-preview OUT_DIR` renders the
burst from three heights and close up through its second (`cluster-*` files).

## Source stations and availability

John requested an aircraft-wide ordnance and weapon-availability pass on
2026-10-05, including missing stations and external tanks. This pass changes the
editor and accepted loadouts; it adds no aircraft rendering. The per-aircraft
source evidence and missing-mount inventory live in the
[ordnance source matrix](../formats/aircraft-ordnance.md).

Agent-selected presentation rules: display one row per editable source
hardpoint. A row's heading is its source location label alone (John removed the
station numbers on 2026-10-16: "Internal Gun", "Wing", "Centerline"). The rows
are listed in a display order, not their source order: the internal gun first,
then the centreline, then the other stations from the fuselage outward by
lateral offset (see [display order](#display-order), John 2026-10-16). Station
numbers stay the source's everywhere else (mission text, the wire, saved loads,
checkpoints, replays). A source row may carry
multiple stores; its quantity is not evidence of multiple physical pylons.
Weapon and tank capabilities on the same hardpoint share one row. Always show
its actual loaded occupant, regardless of the selected catalog. Switching between
air-to-air and air-to-surface never hides an installed store or moves its row. Fixed sensors and countermeasure equipment are not editable store rows. A
source compatibility bit alone does not enable replacing an installed device
whose removal is not implemented. Keep those occupied device mounts out of the
tank compatibility test and editable union; do not present them as empty mounts.

An empty source hardpoint is available when its reviewed compatibility permits
an imported, implemented weapon. A default tank does not prevent that station
from being used for a compatible weapon instead. Preserve source-default
weapon and tank quantities; newly exposed empty weapon stations start empty.
The implementation may retain a compatible weapon definition at zero quantity
for editing, but this contributes no ammunition or weapon mass and must not
appear as an installed store. Saved loads retain their existing weapon station
order; any new optional rows append after those stations, and older saved loads
leave those new rows empty.

Use six station cards per page in the existing three-row area. When more than
six rows exist, station Previous/Next controls and a page count occupy the
top strip, independent of catalog paging. Tab cycles all rows in display order
and reveals the focused page; Previous and Next select the first row of their
page. This is an agent-selected layout safeguard, not a claim about
retail pagination. All currently reviewed aircraft fit in six rows.

A loaded row responds to left-click, right-click and quantity keys according to
its actual occupant, irrespective of catalog selection. Left-click adds one;
right-click and quantity keys retain the existing weapon step or one tank.
Clicking an empty row loads the selected compatible catalog item. Dragging a
catalog item onto a compatible row replaces its current occupant, including
weapon-to-tank and tank-to-weapon replacement. A rejected edit preserves the
previous load. A tank dragged between compatible stations transfers one tank,
limited by source quantity and destination capacity; dropping it in the catalog
unloads that row. Use the same original-thumbnail drag presentation and cancel
rules as weapons. An empty row can restore its retained type with the quantity
keys without changing its store kind merely because the catalog category changed. Draft edits may remain temporarily overweight;
removal must still work, and Fly/lobby acceptance enforces takeoff weight. These
cross-category interactions are agent choices extending the existing editor
contract.

Physical source compatibility does not supply missing radar or designator
capabilities. Guided release still follows the [missile contract](missiles.md)
and the aircraft's actual sensor and target state.

### Display order

John asked on 2026-10-16 for the station boxes to run gun first, then the
centreline, then the wing stations working outward, "so it is easier to keep
track of" (opinionated; the tie-breaks are agent decisions). It is a display
order only: no station is renumbered in the data, the mission text, the wire,
checkpoints or replays, so multiplayer and saved loadouts are unaffected.
Clicking, dragging, right-click, keys and Tab all act on the station a box
shows, whatever place the box has.

1. **Internal gun first**: a row whose store is internal (source flag 8, the
   gun rows, including the F-15C's gun listed under Fuselage) or whose location
   is Internal Gun.
2. **Centreline** rows (location 0).
3. **Every other row**, inboard to outboard by the absolute lateral offset of
   its source position (x; the source records one side and the pair mirrors
   it, [hardpoint positions](../formats/aircraft-ordnance.md#hardpoint-positions)).

Within a group, rows with the same offset keep their source order. A row with no
source position sorts after the positioned rows of its group, in source order. A
row that holds a weapon uses the weapon row's position, a tank-only row the tank
row's. The F-15C therefore lists gun, centreline, fuselage AIM-120s, wing AIM-9s,
wing tanks; the F-16C gun, centreline, its inner wet pair, then the weapon wing
rows and the wingtips.

## Presentation

The original ORD_AIR3 background, thumbnail images, dial, rocker and button
pieces remain runtime imports. Catalog cards use two columns at x=68/187,
four rows from y=108 at 68-pixel spacing. Station headings retain x=350/469,
three rows from y=121 at 71-pixel spacing. Card names, quantities, location
labels and numeric fields use the approved 10 px Noto Sans Bold atlas from
[Quick Mission](quick-mission-menu.md). This is an agent-selected extension of
the approved font, not evidence of the original font face. Selected weapon names
are yellow and catalog mass/guidance are blue. Names must fit 111 pixels.

John requested centered thumbnail boxes on 2026-09-22. Both catalog and station
black wells are 113 pixels wide. Their left edges are x=66/185 for the catalog
and x=351/470 for stations. The 109-pixel outlines start two pixels inside each
well, at x=68/187 and x=353/472 respectively. This leaves equal two-pixel side
margins on selected, unselected and empty cards. Thumbnails are centered within
the 109 by 23 pixel outline in both directions; original 105 by 19 pixel images
have two pixels of inset on each edge, including the outline. These measured
background bounds guide fitted placement, not a retail interaction claim.

The page and fuel rockers use the retail rocker frames and behaviour of the
[debrief rocker](debrief.md#presentation): level at rest, tilted while pressed,
acting on press and springing back on release. Page turns sound the rocker;
fuel changes keep their fuel cue. Arrow keys tap the page rocker. Requested by
John on 2026-09-23.

The category dial uses DIAL13 for air-to-air and DIAL11 for air-to-surface,
at (148,393), pointing toward the corresponding category lamp. LIGHTON and LIGHTOFF overlay both background lamps at (115,394) and (115,422),
so active and inactive indicators both use the original blue outer rim. These images were visually inspected; the angle choices
and coordinates are fitted to the reference. They keep the entire dial and shadow
inside the white category frame and do not cover either label or indicator.
The existing category hit regions and keyboard shortcuts are retained.

Vehicle weights have comma-separated integer pounds and align to x=467 inside
their black fields. The available-weight value is shifted down two canvas pixels
to y=381 following John's visual correction. Fuel pounds are centered in (487,354,56,14). Fuel percentage
shows the number only in (487,375,27,14), since the background supplies the percent
sign. The page count is centered in (248,388,48,15). Text centering measures the
visible glyph pixels, excluding transparent font padding.

Select Plane and Fly share Quick Mission's button composition and label offsets.
Fly includes the original ACTDFLT striped cap joined to ACTDFT0L/M/R. Select Plane
uses ACTION0L/M/R without an added outer outline. Labels retain the approved
lower position; pointer hit regions include Fly's cap. Button actions are unchanged.

Top menu labels retain MENUFONT. Ordnance and Quick Mission labels are centered
vertically inside y=38..58, using visible glyph bounds. Ordnance Weapons and
Airbase begin at x=103 and x=178; matching hit regions do not overlap. Main-menu
bar labels are centered within their existing interactive rectangles. The exact
original font alignment and dial placement remain unknown; these are fitted rules.

### Game messages

John requested single-line ordnance messages with a background sized to the text
on 2026-09-23. This is an opinionated presentation rule. Preserve the existing
message content, pale text and dark background colours. Agent-selected layout:
anchor the strip at (30,335), with four pixels of horizontal padding and two
pixels of vertical padding around the font row. The background is only as wide
as the displayed text plus padding and only one font row tall plus padding.
Collapse whitespace to single spaces. Messages wider than 572 pixels end with
`...` within that width, so the strip never exceeds 580 pixels or wraps.
An empty message draws no strip. Other screens retain their own message layout.

### Laser warning and sensor helper

Slice BQ, B24 (John asked on 2026-10-12 for helper text on aircraft with built-in
IR or targeting sensors and a subtle warning for laser ordnance with no
designator; the wording, colours and places are the lead's defaults,
opinionated). The words come from the aircraft's data, never from a list of
names, and they are advisory: Fly and Accept still work with the warning up (a
flight mate's laser spot guides the bomb, [buddy
lasing](air-to-ground.md#buddy-lasing)).

- **Warning.** When a loaded station (quantity above zero) holds a laser weapon,
  signature 1 with the designator flag (the Paveway bombs and GBU-28, and the
  laser missiles AS-30 and AT-12), and nothing on the aircraft can lase, the page
  says **No laser designator: needs buddy lase** in the message band, in a dim
  amber (196, 138, 52) on the message's dark strip. Something can lase when the
  aircraft has a laser in its own sensor records (the Su-25, the V-22, the F-16C
  and A-10's fixed Pave Spike) or a designator pod hangs on any station. A FLIR
  does not lase. A message the player has just earned (a refused load, Cheat
  loading) takes the band while it is up; the warning is back when the message
  clears. Single player's band is the message strip at (30, 335); the lobby's is
  the strip under the title bar, the same place its messages use.
- **Helper.** One line **below the Select Plane and Fly buttons**, on the
  backdrop under the right panel's frame, in the band John circled on
  2026-10-16: a dark strip 15 px tall at y=458, centred on the panel (x=466),
  4 px padding each side, in a light blue-grey (208, 228, 238), brighter than
  the first (150, 170, 180) because John asked for it to stand out. It clears
  the buttons (they end at y=438), the panel frame and the left panel: the strip
  starts no left of x=322 and ends before x=632 and a line too long for that
  ends with `..`. The words come from the aircraft's sight sensor
  ([who has a sight](air-to-ground.md#who-has-a-sight)): "Built-in laser
  designator: no pod needed for laser ordnance." or "Built-in FLIR: no pod needed
  for IR weapons." for a built-in sensor (a cone gimbal); **"Aircraft laser pod
  equipped"** (or "Aircraft FLIR pod equipped") for a fixed pod (a hemisphere
  gimbal: the Pave Spike row, the AAS-38), shortened on John's request on
  2026-10-16; and, for an aircraft with no sensor but a station that takes a
  pod, "No built-in designator: hang a Pave Spike pod to lase." An aircraft with
  none of these shows no line. The helper has its own place, so it shows
  together with a message or the warning, in the lobby too (it used to share the
  lobby's strip under the title bar and give way there). The message and
  warning keep their places.
- **Preview.** `--quick-mission --snapshot-state ordnance-laser` and
  `lobby-ordnance-laser` load a laser bomb on the first station that takes one
  and show the air-to-surface page.

### Tank graphics

Every tank card, in the catalog and in a station box, draws the retail tank
picture (`$F150`, `$F250`, `$F350`, `$F500`). The authored tanks
([stores on pylons](stores-on-pylons.md#derived-f-15c-and-f-16c-stations):
`F370`, `F610`, `F600`, `M396`, `R528`) have no retail picture of their own, so
the card draws the picture of the retail tank nearest in gallons (larger on a
tie): the 370 gallon tank takes the 350's, the 396 the 350's, the 528, 600 and
610 the 500's. John saw the 370 gallon card without a picture on 2026-10-16
(requested; the nearest-size rule is an agent decision). `ordnance-authored-tank`
previews it.

### Opening on a saved load

Slice BQ2 (John, 2026-10-12). In Quick Mission, a Custom load opens the page on
the load flown the last time the same mission was flown, kept in the profile
across restarts; a changed mission opens it on the aircraft's standard load. The
rules, the file and what counts as the same mission are in [the custom loadout
across restarts](quick-mission-menu.md#the-custom-loadout-across-restarts). A
saved load goes through the same checks as any accepted load, and falls back to
the standard load if it fails them. The lobby's page is not affected.

## Dragging and empty stations

John requested visible weapon dragging, unloading into the catalog, station
transfers and persistent empty-card outlines on 2026-09-22. These requested
interactions are opinionated requirements; thumbnail art and quantity steps
come from the linked recovered contract.

While dragging, only the imported weapon thumbnail follows the pointer, with
its transparent pixels preserved. The system cursor is hidden during the drag.
No card border, name or quantity follows it.
Catalog drops fill a compatible station to its capacity. Station-to-station
drops move one quantity step: 1 for capacity up to 100, 10 for 101 through 300,
and 100 above 300. Transfers stop at the available source quantity and free
destination capacity. A different destination weapon is replaced. Incompatible
drops preserve both stations and display the existing compatibility notice.
Dropping onto the source station leaves its quantity unchanged.

Dropping a station weapon anywhere in the eight-card catalog area, including
unused card spaces, empties that station completely. Other releases leave the
load unchanged. Escape, focus loss or leaving the menu canvas cancels dragging.
John also requested adding one weapon by left-click on 2026-09-22. A matching
left press/release on a loaded station adds exactly one round or store when
capacity remains, regardless of a prior catalog selection. A full station is
unchanged. Clicking an empty station loads the selected catalog weapon as before;
dragging from the catalog can replace a loaded station's weapon. Right-click
decrement and keyboard quantity steps retain their recovered scaling.
Agent-selected input details: movement of 3 canvas pixels starts a drag; the
thumbnail is centered on the pointer; only catalog drops unload, so releasing
over unrelated controls cannot discard stores. These details are fitted.

Every empty station, including an empty internal gun station, keeps its red
109 by 23 pixel thumbnail outline. The weapon image, name, quantity and Empty
label are absent. The station's location heading remains above the outline.

## Sound effects

Successful loading, quantity changes, unloading and station transfers use the
retail ordnance cue once per completed edit, without the standard menu click
layered over it. Ordinary stores use `&ARMWPN.5K` (about 0.633 seconds);
ammunition-marked weapons use `&ARMBLLT.5K` (about 0.419 seconds). Classification
comes from the imported weapon's ammunition flag, not its filename or station
location. This is spec-derived behavior from the
[retail sound selection](../formats/ordnance-menu.md#sound-selection).

Use the weapon being loaded or transferred to choose the cue; for an unload,
use the weapon being removed. A full station, empty decrement, rejected drop,
same-station drop, canceled drag or pointer movement plays no ordnance cue.
The requested one-at-a-time left-click addition uses the same successful-edit
cue as other quantity changes. Catalog selection and unrelated menu actions
retain their existing button feedback. The Unload All menu item also retains
its existing button feedback; its original dedicated sound remains untraced.

Fuel edits use `&ARMDRIP.11K` (about 0.257 seconds) when the fuel amount changes.
Do not stack or restart that sample while it is already playing. At either fuel
limit, no fuel cue plays. Sound-effects Off and `--no-audio` silence these cues.
Playback uses the imported unsigned PCM8 mono samples at the existing reader's
5,512 Hz / 11,025 Hz rates. Original device volume and shell repeat cadence are
unverified; the host retains its current effects mix and discrete edit events.

## R-27 identity and carriage

John's 2026-10-15 correction keeps AA10.JT as R-27R and adds R27ER/T/ET
virtual stores from the user-owned donor record. Cards show the R-27 variant;
selecting one shows its full NATO name in the existing notice band. The HUD
uses AA-10A/B/C/D. See the single [missile definition](missiles.md#corrected-identities-and-r-27-stores)
for names, aircraft, rail compatibility and fitted envelopes. These catalog
changes require matching host/client builds and a protocol bump by the lead.

## Smaller guided stores

GBU-12, GBU-16 and GBU-38 use private aliases of their donors' models, refitted
in the shared store-size table for pylons, previews and released weapons.
Their loadout cards reuse donor art and display their own GBU names. Sizes,
damage, crater scaling and provenance are specified in
[smaller guided bomb stores](missiles.md#smaller-guided-bomb-stores).
