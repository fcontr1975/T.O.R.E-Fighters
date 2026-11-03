# Quick Mission menu presentation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation specification, 2026-09-16. Existing field values and dependencies
come from [the recovered tables](../formats/quick-mission.md). The two original
menu screenshots supplied by John in this session guide presentation only; their
executable build identity is unknown. They do not establish filtering or input
behaviour.

Aircraft choices contain only exact supported identities with a successfully
parsed imported flight profile. Metadata-only aircraft and variant aliases are
excluded from every wing selector. This is an opinionated restriction requested
by John on 2026-09-16. Names use the supported aircraft's full variant label.

Aircraft and theater fields open a modal list on activation. Shift cycles these
fields; other scalar fields retain click-to-cycle and Shift-to-open. Direct
opening for theaters is an agent decision. Selecting a row is provisional until
OK; Cancel and Escape preserve the draft. Empty lists cannot be accepted.

John requested reverse cycling with right-click on 2026-09-21. Right-clicking an
inline option selects its previous available value, wrapping to the last value.
This also works for aircraft/theater/airport fields without opening their list;
their existing left-click behavior is unchanged. The player's wing count wraps
from one to its maximum, never through zero. Empty lists do nothing. Existing
field dependencies continue to apply to editable settings. Require a matching
right press/release on the same field.
Right-click cannot activate OK, Cancel, Exit, popup rows or controls behind a
selector/help menu. Focus loss cancels a pending press. In the ordnance view,
right-click retains its existing station-quantity decrement.

The selector shows 15 rows per page, black inset wells, blue diagonal markers,
a gold selected marker, a page count, a Prev/Next rocker, and beveled OK/Cancel
buttons. Arrow keys move selection, Home/End reach the ends, and Page Up/Down
move 15 entries. The rocker animates and pages on press like the
[debrief rocker](debrief.md#presentation); Page Up/Down tap it. Pointer paging preserves the pending selection until another
row is chosen. All 15 black wells remain visible on every page. Unused wells contain no marker
or label and have no hit region; the last page may contain fewer than 15 entries.

Presentation is fitted to the supplied references on a 640 by 480 canvas:
the existing 270 by 370 selector starts at (185,100), with 14-pixel wells spaced
18 pixels apart. Runtime original panel texture, rocker, button pieces and fonts
are reused. Diagonal markers and bevel edges are authored. Quick Mission briefing,
selector and button text uses a bundled raster atlas of Noto Sans Bold at 10 px.
It is flat light grey with antialiased edges, with no glyph shadow or bevel.
John requested a cleaner regular bold font at approximately the existing size
on 2026-09-16. Choosing Noto Sans Bold is an agent decision. Its 10 px raster size follows
John's request for a slight further reduction. The font is openly licensed, not derived from retail media; see the
[atlas provenance](../../crates/tore-app/assets/README.md).

Inline field boxes have a one-pixel raised bevel and two pixels of vertical
padding around the 11-pixel glyph cell. Each inline field reserves two extra
pixels on either side, in addition to sentence spaces, so adjacent boxes remain
separate. Ground-target fields retain sentence wrapping and spacing.
Menu magnification uses nearest sampling to avoid blur. Flight UI sampling is
unchanged. Unsupported mission settings still produce a notice at launch.

The blue OK artwork has three additional top rows compared with Cancel.
Its draw origin is shifted up three pixels so both coloured faces align.
The original `ACTDFLT.PIC` cap supplies the striped marker, outside outline and
left rim beside the blue face. It is placed directly before the imported
`ACTDFT0L/M/R` pieces at their shared top edge. No authored replacement border
is drawn over these assets. The OK hit region includes the marker.
Cancel uses only its imported button pieces, without an extra surrounding frame.
Both labels use the flat menu font, centered in the 21-pixel-high face and shifted
down two pixels to match John's supplied close-up. Label placement remains fitted.

Exact original font choice, popup texture placement and field bevel composition remain
unknown. Further research would inspect the original dialog drawing data.

The Aircraft menu label is vertically centered by visible glyph bounds within
y=38..58, matching the [ordnance menu bar](ordnance-presentation.md). The
**Map** entry (slice CL-R2, John 2026-10-11: "the map positioning in the menu
bar is off quite a bit") follows the same rule as the other bars' entries:
its label starts one label's gap, 14 pixels, after the word before it
(103 for Aircraft, then about 175 for Map; the ordnance bar's Weapons entry is 75
wide and Airbase starts 13 to 14 pixels after the word), and the hit boxes
abut, each running from its label to the next. Its label sits on Aircraft's
baseline, because the three letters of "Map" have a descender that would
otherwise lift a label centred by its own visible bounds a pixel or two.
Single player, the lobby's creator and the read-only view draw the one bar.

## Ground target and defenses

The sentence block "Friendly ground target is [target] [AAA strength] defended
by AAA and [SAM strength] defended by SAMs." is live in all sixteen theaters
(requested by John on 2026-10-10; before that, from 2026-09-23, it showed a
notice and changed nothing). Its words are retail data: each theater's target
list and the four strengths "not", "lightly", "moderately" and "heavily" come
from the executable's creator strings, and the text form of a mission file
writes the strengths `none`, `light`, `moderate` and `heavy`
([mission file](../DEDICATED-SERVER.md#the-mission-file)).

- **Target list.** Each theater offers its own list, whose first entry is the
  "nothing" template (no ground target) and whose other entries are the
  theater's template missions, in menu order
  ([the list per theater](../formats/quick-templates.md#recorded-facts-from-faexe-102f)).
  Clicking the field steps through the list; Shift opens it.
- **Strengths.** Each strength steps through its four words, with or
  without a target. With a target they man the target's defenses. With the
  target at "none" they set the enemy's **area defenses**: SAM sites and AAA
  guns scattered over Red's side of the front (John, 2026-10-12, B23; see
  [area defenses](surface-defenses.md#area-defenses)). Both "not" (the
  default) gives a mission with no target what it always had. Retail clears
  both strengths when the target is set to none; here they stay, and a
  theater change keeps them too (agent decision, 2026-10-12: the strengths
  now mean something without a target, and they are not tied to a theater).
- **Enemy nationality.** Field 20 is part of the mission. It picks the
  equipment group the target's units are drawn from, so the mission file
  carries it (`enemy-nationality`), and the lobby's read-only creator shows it
  instead of "as the King's". Changing the theater resets it to the
  theater's own enemy.
- **Start.** With a target chosen, the Airport list of a ground start gains a
  first row, "Automatic (near the target)", which is the default: the mission
  says `start ground auto` and the world picks the runway from the target.
  Naming an airport keeps that airport. Choosing no target (or a target when
  none was set) starts the list over. "Automatic" takes the Blue airfield
  nearest the target at least 15 nm from it
  ([start placement](surface-defenses.md)); when none qualifies, the first
  Blue airport whose runway holds the wing, then the first neutral one
  (fitted), and never an enemy field (slice AL1): with none the start is
  refused.
- **Seed.** Pressing OK on a mission with a target, or with area defenses,
  keeps the number the creator reserved for it (the map shows the same
  layout). A new flight reserves a new seed and a restart repeats the
  accepted one. The lobby sends the reserved seed to the host. Selecting a theater
  or target again reserves a fresh layout (fitted interface choice, agent,
  2026-10-25). If the reserved seed's layout cannot place every vehicle, or
  leaves a target's Blue start no room for the distance, the launch takes the
  next seed, at most sixteen in all, as a host does (agent decision,
  2026-10-28).
- **Lobby.** The King edits the fields like any other; everyone else sees the
  King's choice read-only, with the same dimming rule as the other fields.
  A multiplayer start stays airborne, so `start ground auto` does not reach
  a lobby.
- **What the flight gets.** The target's template, its SAMs, guns, ships,
  supply trucks and parked aircraft are built from these fields and the seed,
  defend the target and make it the mission's objective
  ([surface defenses](surface-defenses.md)). A template the import does not
  hold leaves the mission without it, and the log says so.
- **The front line** (John, 2026-10-11; slice CL-FL). A mission with a target
  also gets a few Blue and Red ground units standing in contact 5 to 20 nm
  from the targets (a tank and an armoured car and a gun per side, the gun only
  with AAA on), so a surface battle is visible on the way in. They are
  not objectives and add nothing to the creator: no field, no setting, drawn
  from the same seed ([the front line](surface-defenses.md#the-front-line)).
- **Area defenses** (John, 2026-10-12; B23). With no target and a strength
  above "not", the enemy's SAM sites (with their radar vehicles) and guns
  stand still on Red's side of the theater's front, 10 to 60 nm deep, with
  one site in three guarding a Red airfield. Ten SAM site slots and ten gun
  site slots are rolled the way a target's defense slots are (light 25,
  moderate 60, heavy 100 percent manned; John, 2026-10-13), so light gives 0
  to 10 sites of a kind (2.5 on average), moderate 0 to 10 (6) and heavy
  all 10, two units each (the slot count is fitted). The SAM units have
  supply trucks, as a target's do (John, 2026-10-13). They are not
  objectives and are not on the map. Quick Mission only: free
  flight has no creator, so it never has them. Same on every machine and in
  replays ([area defenses](surface-defenses.md#area-defenses)).

The old notice popup (sentence hit region (334,294), 278 by 49; popup (166,202),
308 by 88) is gone, and so is the launch refusal "Ground targets and defenses
are not available yet." The sentence keeps its layout and word-wrapping; the
fields are ordinary inline boxes. Snapshot states: `ground-target` (the
theater's first target, AAA moderately, SAMs heavily), `ground-target-last`
(its last, lightly and moderately), `ground-start-auto` (the first with a
ground start) and `lobby-creator-view-target` (the read-only creator on a
mission with a target); `field-30` to `field-32` open the lists.

## Mission end

End mission opens the [mission debrief](debrief.md). Its OK returns to this
creator with every setting of the mission just flown, including a custom load,
and the ordnance screen closed. Requested by John on 2026-09-23. Starting the
next mission reopens the ordnance screen for a custom load as before.

## First load and the saved draft

Slice BQ, B22 (John asked on 2026-10-12 that each slot's aircraft be random on
first load and that the mission state survive the end of a game and a trip to
the ordnance screen; the two rules below are agent decisions, recorded as such).
Opinionated, not retail: the original opens on the same list head every time.

- **First load** is the first time the creator opens in a profile that holds no
  saved draft. Each aircraft slot then takes a random aircraft from its own list:
  friendly wing 1 (the player's own aircraft) from every aircraft the game has,
  and the five other wings from the aircraft the AI can fly. `--aircraft` still
  names the player's aircraft and keeps that slot; the wings are random. The pick
  is a menu choice from the clock: nothing in a mission reads it, and once a
  mission is built it is a fixed mission, so it stays the same on every machine
  online and on every replay. John confirmed on 2026-10-12 that the picks stay
  100% random (no preference for armed combat aircraft, so a wing can be
  transports or an AWACS) and that the player's own slot is random too.
- **The draft is kept** in the profile as `quick-mission-v1.conf` and written
  after every menu action, so it survives the end of a flight (the debrief's OK
  returns to the creator as it was), a visit to the ordnance screen and a
  restart. The random pick is made once: the next launch opens on the saved
  draft and draws nothing. It holds the theater (by code), every aircraft (by
  selection key, so a changed import cannot turn one aircraft into another) and
  every other choice by its place in its list, with the group objectives and
  survival flags. A choice the lists no longer have keeps the creator's own value.
- **Not kept in this file:** the ground target's seed (each flight draws its own),
  the custom load on the ordnance page (it has its own file, [below](#the-custom-loadout-across-restarts)),
  the AI mission preset (a command-line choice), the lobby's creator and the
  read-only lobby view. A file this build cannot read, or one
  with a newer version, is left as it is and the creator opens on its defaults.
- **Command-line and diagnostic runs** (`--snapshot`, probes, scripted runs,
  `--smoke-test`, `--launch-quick-mission`, `--ground-start`, `--ground-target`,
  `--separation`) neither read nor write the file and never randomize, so every
  battery scenario still builds the same creator. `--theater` wins over a saved
  theater.
- **Preview.** `--quick-mission --snapshot-state first-load` draws the creator
  with every slot random from a fixed seed.

## The custom loadout across restarts

Slice BQ2 (John asked on 2026-10-12: "persist the custom loadout only if the
mission has not changed, or when re-flying a previously flown mission"; he also
confirmed that the first-load picks stay 100% random, with no preference for
armed combat aircraft, and that they cover every slot including the player's).
Opinionated, requested by John; the choices below are agent decisions.

- **The rule.** When the ordnance page opens for a Custom load, it shows the
  load the player flew the last time this same mission was flown, if there is
  one. Otherwise it opens on the aircraft's standard load, as before. A change to
  the mission therefore drops the saved load for that mission: the player starts
  from the standard load again.
- **What "the same mission" means.** The theater, every aircraft (the player's and
  all five wings, by selection key) and every other creator choice (all list
  fields, the group objectives and the survival flags). The only field left out is
  Standard or Custom load (field 18), which says whether the page opens, not what
  the mission is. Not part of it, because the creator does not hold them: the
  ground target's seed (a re-fly reuses it), the cheats, the AI mission preset
  and the other command-line choices. The stamp is a 64-bit hash of the draft's
  saved text, so it is the same on every launch.
- **Re-flying.** The debrief's OK returns to the unchanged creator, so the next
  mission is the same mission and its load comes back, after a restart too. Going
  back to a mission flown earlier (changing the creator away and back) restores
  that mission's load, because the profile keeps the loads of the last eight
  missions flown with a Custom load, the oldest forgotten first. In the same
  session, edits made on the page and not yet flown (Cancel, then reopen) are kept
  as they always were. If no load was flown for the new mission but the aircraft
  is unchanged, the page keeps the session's load, as before. Restarting the flight in progress never
  touches the creator and is unchanged.
- **When it is written.** On Fly, with the load that flew, so an abandoned edit
  is not kept. Standard load never opens the page and writes nothing.
- **The file.** `quick-loadouts-v1.conf` in the profile, beside the draft file:
  a header line, then per mission a `mission HASH` line, `fuel`, `cheat`, the
  `tank` lines and one `station N WEAPON COUNT QUANTITY` line per station, the
  same words the mission text file uses. A file this build cannot read or with a
  newer version is left exactly as it is: nothing is restored and nothing
  written. A saved load that no longer fits the aircraft (a weapon the import no
  longer has, a station that changed, a failed validation) falls back to the
  standard load.
- **Skipped runs.** Whatever skips the saved draft skips this too (snapshots,
  probes, scripted runs, `--smoke-test`, `--launch-quick-mission`, the creator
  setting flags), and so does `--loadout`, so battery scenarios build the same
  loads as before.
- **The multiplayer lobby** has a separate ordnance path (its Accept sends the
  load to the lobby) and is unchanged.

## The player's flight loadout

**Opinionated, requested by John on 2026-10-15.** Every AI aircraft in the
player's QMC wing starts with the accepted player load: identical weapon
records, station capacities and quantities (including zero), designator pods,
external tank choices and quantities, and internal fuel. Standard and Custom
loads follow the same rule, including Cheat loading. Airborne and ground starts
and a flight restart preserve it. Other wings retain their own default loads.
QMC selects one aircraft type per wing. A different-type member, if a future
creator permits one, keeps its own default instead of receiving incompatible
stores. The fixture-only development path is unchanged.

The loadout memory described above supplies the accepted load before sharing.
Sharing does not add AI employment logic: a carried store that no AI weapon
selection rule can employ stays aboard. Existing orders and weapon eligibility
still decide what the AI can release, and an order with no usable store can
receive Winchester. An unarmed load is not silently replaced.

The creator writes `share-flight-loadout yes` in its mission text. An authored
mission without this setting retains its existing loads. Multiplayer uses
per-plane loads and the [human-flight rule](multiplayer.md#human-flight-loadouts).

Retail QMC load sharing is **unknown**. The current recovered creator and AI
specifications do not establish how an accepted custom load is distributed to
wing members. Next research: trace the accepted QMC ordnance data into the
retail wing member initializer, recording the executable build identity and
station/count writes. This implementation is not a retail parity claim.

## Mission wings

Guns only applies to the player and every member of all six wings, on both
launch and restart. Each aircraft retains ammunition for its own gun; every
other weapon has zero ammunition, including missiles in internal bays. Choosing
Guns and missiles with Standard load restores normal default weapons. Custom
player loads still reject non-gun weapons at launch while Guns only is selected. This scope is an
opinionated requirement requested by John on 2026-09-22. It changes loadout
initialization only. It does not change flight or combat decision rules.

Implementation mode. Normal creator launches use AI for every selected non-player
aircraft, requested by John on 2026-09-17. Friendly Wing 1 includes the player,
so a count of 5 creates four wingmen. The other five wings each launch their
full selected count, up to 29 AI aircraft plus the player. Each wing retains its
own aircraft, experience, side and leader. Friendly Wing 1 follows the human;
the other wings have their own AI leaders. No command-line option is required.
`--ai-wings` remains a shortcut to open the creator.

AI helicopters and VTOL aircraft (slice ST-H, John's decision of 2026-10-11;
slice H3, John's decisions of 2026-10-12): every aircraft is offered in the five AI
wing fields, the AH-64, Mi-24, CH-47, V-22, AV-8 and Yak-141 included, and in
friendly wing 1 when the player flies one of them on an airborne start. The CH-47
is an AI wing choice only (no escort objective: the creator's escort missions
already protect a chosen wing) and carries no weapons, so it never takes the Strike
role, whatever the wing's objective or the bomber-class default. A helicopter of
the AI's own wings starts 1,500 ft over the ground under it rather than at the
chosen altitude, in forward flight (**opinionated**, agent, kept by John,
2026-10-12). A player in a helicopter, the V-22, the AV-8 or the Yak-141 who
chooses a ground start leads friendly wing 1 alone, with a notice, because the AI
cannot take them off the ground yet (slice H2); the other wings start airborne at
any start. With a ground target set, a friendly wing 2 or 3 of armed helicopters
strikes by the bomber-class default. See
[AI strike](ai-strike.md#ai-helicopters-in-quick-mission) and, for their fuel,
[the AI's fuel](ai.md#b48-routes-fuel-and-recovery).

John requested separate delta formations on 2026-09-17. The host uses the B43
alternating trailing slots with level stacking: slots 1 and 2 are 512 ft right
and left, 512 ft behind; slots 3 and 4 are 1024 ft right and left, 1024 ft behind.
This is an opinionated formation choice; it is not a new recovered formation
name. Idle wingmen track their own leader's moving slots using the
[physical formation and rejoin procedure](ai.md#physical-departure-and-rejoin).
Combat maneuvers can take them out of formation.

Original Quick Mission spawn geometry is **unknown**. Next research: recover
the generator's relative wing placements and situation offsets. Pending that,
the **fitted**, agent-selected placement puts friendly wings 2 and 3 at 4096 ft
left/right and 4096 ft behind the player. Enemy wing 1 starts at the selected
separation, with enemy wings 2 and 3 offset 4096 ft left/right. Where and which
way the enemy faces follows the [situation](#situation). Airborne aircraft start at the chosen altitude; a
ground start parks the player's wing instead (see
[Player ground start](#player-ground-start)). Slots rotate with each wing
leader. Close tracking projects the slot along the leader velocity; separated
aircraft use the linked rejoin procedure, with neighborhood clearance and
approach coordination.
Restart restores all six groups exactly as launched.

### Separation

Separation is in nautical miles (6,076.12 ft), as the manual states (p.19:
"between 1 and 50 nautical miles"). John chose nautical miles for every entry
on 2026-09-23. The retail list offers 1, 2, 5, 10, 20 and 50 miles; John
requested 100, 150, 200 and 300 miles on 2026-09-23 and 75 miles on 2026-10-09,
and the host appends them in ascending order in the retail label style ("75
miles", "100 miles", "150 miles", "200 miles", "300 miles"). Every label means nautical
miles. An out-of-range choice falls back to the 5 mile default instead of
failing.

### Situation

The manual (p.19): "Specify the battle situation for friendly forces.
Friendly forces can hold the ADVANTAGE (behind the opponents, on their tail),
be in a NEUTRAL situation (approaching each other head-on), or be at a
DISADVANTAGE (opponents are behind you, on your tail)." The creator's field 16
("Your situation is"), neutral by default, places an airborne start (John,
2026-10-28; the geometry is fitted, agent decision):

| Situation | Enemy group | Enemy heading |
| --- | --- | --- |
| Neutral | Ahead of the player at the separation | Toward the player, head-on |
| Advantage | Ahead of the player at the separation | The player's heading: flying away, the player on its tail |
| Disadvantage | Behind the player at the separation | The player's heading: on the player's tail |

The separation is measured from the player to Enemy Wing 1's leader. Enemy
wings 2 and 3 stay 4096 ft left and right of it. An enemy facing the player
mirrors its formation, its wingmen farther away; an enemy flying the player's
heading keeps it, so its wingmen trail toward the player (advantage) or away
(disadvantage). The friendly wings do not move. As in a neutral start, the
whole airborne scene turns to keep the enemy on the map.

The situation means nothing, and the start is neutral, when the player starts
on the ground (a parked wing cannot be on anyone's tail) or when a ground
target is chosen (its template places the enemy by the target). An enemy
ground start parks only Enemy Wing 1; the other enemy wings take the
situation. The mission text carries it as `situation advantage` or
`situation disadvantage` (absent when neutral), so the lobby's read-only
creator shows the King's choice.

### Keeping the enemy on the map

John requested on 2026-09-23 that the enemy group is aimed into the map when
the selected separation would put it off the edge. Rule (the numbers are
**fitted** agent choices):

- Every enemy aircraft, including the side wings 4096 ft left and right and
  the formation slots behind them, must start at least one terrain cell
  (8,192 ft) inside the map edge.
- Straight ahead (straight behind in a disadvantage) is kept when the whole
  group fits there. Otherwise bearings are tried one degree at a time, nearest
  first, clockwise before counter-clockwise at equal angles. The first bearing
  that fits at the full distance is used.
- In an airborne start the whole airborne scene turns: the player's starting
  heading and the friendly wings turn with the enemy, so the enemy keeps its
  place ahead or behind. In a ground start the parked wing cannot turn, so
  only the enemy group moves around the airport and still faces it.
- **Only distances that fit are offered** (John, 2026-10-28). The creator's
  Distance list holds the distances a fixed table gives for the theater, the
  ground target (or none, or area defenses, which place no start) and the
  start; the others are not listed. Nothing is greyed and nothing is checked
  while the player edits: the table was measured once, offline, with the
  launch solver, and is checked in (`tore_world::mission_layout::distance_table`,
  regenerated as [the developer guide](../DEVELOPMENT.md#quick-mission-distance-table)
  says; the measured table is in
  [the baseline](../baselines/qmc-distance-table.md)).
- When a theater, target or start change leaves the chosen distance out, it
  moves by itself to the largest offered distance at or below it, or to the
  smallest offered one when none is below. No notice is shown.
- A row offers a distance only when it fits every case the creator can make:
  with no target, every enemy formation (each enemy wing 0 to 5 aircraft) in
  all three situations, from the airborne start, and for a ground start from
  every airfield the creator offers with every wing size; with a target and an
  airborne start, 64 layout seeds over all sixteen AAA and SAM strength pairs.
  The situations never changed a row. With a ground target and a ground start
  every distance is offered: the target places the enemy, independently of
  the distance setting (lead decision, 2026-10-26).
- Launch still refuses a distance that does not fit ("Separation N nm does not
  fit this setup"); nothing is silently shortened (John, 2026-10-25). With a
  target, a seed whose layout leaves Blue no room tries the next seed first
  (above).
- A target whose layout always leaves something out says so while it is
  chosen, from the same table: the Baltic's carrier group (QBFLT) shows
  "Unavailable: 4 Yak-141 Freestyle-A (no supported carrier deck)." Shortfalls
  that depend on the seed (area-defense units the map cannot hold, vehicles
  with no valid nearby spot) are no longer shown before launch: launch
  refuses an unplaceable layout and tries the next seed.
- The footprint grade limits are 30% for tanks and 20% for vehicles, launchers,
  guns and added radars. These are fitted values; John requested the relaxed
  launcher/gun/radar policy on 2026-10-26. The bounded nearest-valid search,
  supply reach and retained-equipment behavior follow [vehicle footing](surface-defenses.md#jitter).
- A host retries a placement-refused seed in deterministic order, at most sixteen
  attempts, before publishing its mission. Only the host chooses a replacement;
  every client builds the accepted seed. See the [server policy](../DEDICATED-SERVER.md#surface-placement-seeds).
- CAP regions retain their ten-nautical-mile radius and are intersected with the
  theater rectangle. Clipping does not shrink the whole circle or change its
  station (John, 2026-10-25; an out-of-map station is clamped to the map).

Measured (2026-10-28): with no target, an airborne start offers up to 200 nm
in twelve theaters and up to 150 nm in Egypt, France, Vietnam and Vladivostok;
300 nm fits no theater from the airborne start. A ground start reaches 300 nm
in Cuba and the Kuriles and 200 nm elsewhere (Vietnam offers no ground start). Many targets reach 300 nm from an
airborne start; the [baseline](../baselines/qmc-distance-table.md) has every row.

### Home airfields

Every AI aircraft has a home runway for returning to base. The player's
wingmen in a ground start call the departure runway home. Every other aircraft
takes the nearest runway, measured from where it starts, that its side may
use: a field of its own side or a neutral one that grants permission, the same
test the player's tower service applies. Allegiance is recorded from Blue's
side, so an enemy aircraft's own fields are the ones marked hostile, and the
single neutral-permission flag serves both sides. Ties go to the lower runway
number. An aircraft with no usable runway keeps its start point as home. A short
strip (a runway under 2,000 ft, [see above](airports.md#short-strips)) is nobody's
home. Since slice AL1 (John, 2026-10-10) every imported airport takes its
runway's layout side ([allegiance](airports.md#allegiance)), so Blue aircraft
go home to Blue fields and Redfor aircraft to Red ones; in North Vietnam and
the Falklands, which have no Blue field, a Blue AI aircraft keeps its start
point. Before AL1 every airport was neutral and each aircraft took its nearest
runway. The nearest-runway choice is **fitted** (agent decision, 2026-09-23).

### Radar and aircraft on the ground

Manual p.208: "Grounded aircraft do not appear on enemy radar until they take
to the air." An aircraft on the ground, parked or rolling, gives no radar
return to anyone: it cannot become a radar contact, be locked, or be picked
through the radar, for the player or for AI. It becomes an ordinary radar
contact on the first moment its wheels leave the ground. Visual and infrared
sighting are unchanged, and its own radar and jammer are still received. The
rule is applied as a zero radar signature while the aircraft is on the ground
(fitted implementation detail).

The existing combat AI remains partial. Scoped [player wing commands and radio](ai.md#live-wing-command-and-radio-integration)
are connected, including recipient outcomes and cancellation. Broader mission
campaign routes and scoring remain. Persistent protection and reviewed missile
acquisition are connected through the current AI services. Separate wing placement and formation following do not
establish combat or retail parity.

## Straight-flight mission fixtures

The compatibility option `--fixture-wings` retains the straight-flight setup
originally requested by John on 2026-09-17. Every populated wing
launches its selected supported aircraft. Friendly Wing 1 includes the player;
its remaining aircraft and every aircraft in the other five wings are dummies.
The source count choices remain 0 through 5 per wing, permitting 29 dummies.
They fly straight and can be observed, designated and hit through shared combat
and sensor rules. They never maneuver, shoot, transmit radar or operate a jammer.
Nationality, skill and advantage do not control behavior or protect a dummy from
selection. These are practice fixtures, not friendly/enemy combat AI.

Agent-selected fitted placement: all dummies start at the player's altitude and
heading, forward at the selected separation in nautical miles (see
[Separation](#separation)), unavailable if it does not fit the map. In an
airborne start they turn with the player; in a ground start only the player is
parked and the dummies keep the runway heading. The first is directly ahead. Successive pairs are 500 feet farther
forward and 500 feet farther to either side per pair. Speed is 300 feet/second;
engine heat uses 70% throttle without afterburner. Radius is 28 feet. Aircraft
identity supplies original geometry, texture, radar signature and hit points.
Straight paths can intersect terrain, using the existing collision rules.
Restart restores the accepted formation, stores and fuel. No avoidance is added.

Normal free flight loads the aircraft's supported PT-default weapons as requested
by John on 2026-09-17. The range flag adds diagnostic fixtures; it is no longer
needed for ammunition. The restricted native research adapter and pilot-only input recordings remain
clean to preserve their existing initial conditions.
Weapon compatibility, custom loads and the guns-only creator choice still apply.

## Player ground start

John requested on 2026-09-23 that wingmen start queued on the taxiway near the
runway. With valid airport anchors, place them backwards along the taxi-out
polyline from its last point, facing toward the runway. Fitted spacing is
200 ft, with candidate positions checked every 50 ft and at least 250 ft from
the player's takeoff spot. Every slot must pass the existing pavement and
building checks. If the full queue cannot fit, retain the documented staggered
runway fallback. Only one queued aircraft enters the runway at a time; the
player goes first. This requested placement supersedes parking-slot starts
for Quick Mission. Ordinary parking-start sequences remain supported.


John requested a ground-start choice in the Quick Mission creator on 2026-09-20.
The creator adds Start (Airborne/Ground) and a named airport/runway selector.
Airborne remains the default. Airport choices belong to the selected theater;
changing theater resets the airport choice. Popup cancel preserves the draft.
The accepted start is retained through ordnance setup and mission restart.

Ground start parks the player's whole wing, friendly wing 1, at the selected
airport. The other wings, friendly and enemy, keep the airborne launch.

**Only the player's side's airports (John, 2026-10-10, slice AL1).** Each
airport takes its runway's layout side ([allegiance](airports.md#allegiance)).
The picker lists the player's side's airports and the neutral ones, in layout
order, and its default is the first of them, so a Blue player no longer starts
at Polotsk in the Baltics (an enemy field ringed by ZSU-23s) but at Siauliai,
and in South Korea at Kimpo instead of Koksan. A Redfor pilot's picker would
list the Red fields. In a theater with
no field for the player's side (for Blue, the Falklands and North Vietnam)
Ground is not offered: the Start field is greyed with the note "(no friendly
airfield)", the Ground row of its list is dimmed, and choosing it, or changing
to such a theater with Ground set, leaves the start airborne with the notice
"No airfield of your side in this theater, so you start airborne. Ground start
needs a friendly or neutral airfield." The `--ground-start N` option refuses an
enemy field with "is an enemy airfield: no ground start there. Choose one of
your side's airfields or Airborne." A dedicated server refuses a mission whose
`start ground N` names an enemy field, before it opens its port.

**Enemy ground start (John, 2026-10-11, slice CL-R).** "Add redfor ground
starts. Match the quick mission creator styles." The creator's right column
gains two rows under the ground target sentence, the mirror of Start and
Airport: **Enemy start: Airborne/Ground** and, with Ground, **Enemy airport:**.
They park Enemy Wing 1 (the flight Redfor players take online; the AI flies it
in single player) at a Redfor or neutral field, never a Blue one (a STOL strip is
allowed, labelled, as for Blue); Enemy Wings 2 and 3 stay airborne.

| Blue (Start, Airport) | Redfor (Enemy start, Enemy airport) |
| --- | --- |
| Airborne (default) or Ground | The same two choices |
| The player's side's and neutral fields, layout order | The Red and neutral fields, layout order, STOL fields last and labelled |
| With a ground target, "Automatic (near the target)" first: the Blue-side field nearest the target at least 15 nm from it | The same first row: the Red-side field nearest the target at least 15 nm from it |
| No field: Start greyed, "(no friendly airfield)" | No field: Enemy start greyed, "(no Redfor airfield)", and "No Redfor airfield in this theater, so the enemy starts airborne. An enemy ground start needs a Redfor or neutral airfield." |
| A changed theater resets the airport | The same |
| `--ground-start N` refuses an enemy field | `--enemy-ground-start N` refuses a Blue field: "is a Blue airfield: no enemy ground start there. Choose a Redfor airfield or start the enemy Airborne." |
| Mission text `start ground N/auto` | Mission text `enemy-start ground N/auto` |

Enemy start Ground with Enemy Wing 1 empty is refused at OK ("Enemy wing 1 has
no aircraft to park. Give it aircraft or start the enemy Airborne."), and both
sides on one airfield are refused ("You and the enemy cannot start on the same
airfield."). The enemy ground start overrides the enemy's start distance
([start placement](surface-defenses.md#start-placement)).

**The multiplayer creator offers ground starts (agent decision, slice CL-R,
2026-10-11).** It locked Start to Airborne from EF8 (John's 2026-09-28 rule,
"everyone starts airborne"); John's 2026-10-11 request for Redfor ground
starts in the creator's style supersedes that, so the King's creator offers
both sides' rows as single player does, Accept carries them in the mission
text, and the read-only creator shows them. The notice on opening reads
"Multiplayer: Accept sends this mission to the lobby."

**STOL strips (opinionated, John 2026-10-11; replaces his 2026-09-30 rule).**
The 22 airstrips with a runway under 3,000 ft (about 1,074 ft, in Cuba, the
Falklands, Pakistan, Panama and the Persian Gulf; the number is `fitted`, see
[STOL runways](airports.md#stol-runways)) are in the airport picker, labelled
`(STOL)` after their names and listed after the theater's longer airports, so
the default start is a longer runway whenever the side has one. A takeoff roll
that long does not suit a conventional aircraft or a wing (with the player
alone the paved run is 1,020 ft, falling by 250 ft for each wingman), and the
label says so; the choice is the player's. The Falklands' Blue field, San
Carlos, is now a Blue ground start. `start ground auto` still never picks a
STOL strip. The imported `~` layout variants have none. From 2026-09-30 to
2026-10-11 these strips were left out of the picker and the `--ground-start N`
option refused them.

**Source anchors and player placement (spec-derived).** Each airport's STRIP
shape carries a takeoff spot, taxi route and nine parking slots
([recovered roles](../formats/native-strip.md#remaining-template-callback-boundaries)).
The player starts exactly on the takeoff spot, facing down the runway. Wingmen
use the requested taxiway queue described above. Reviewed TRAIN01.M places
another grounded aircraft in parking; it does not establish Quick Mission's
new queue placement. The takeoff spot is the near end of the host's runway
line, 100 ft behind the earlier fitted single-aircraft start. The anchor
coverage and placement evidence are in the
[ground-start baseline](../baselines/ground-start.md#airport-data).

**Fallback (fitted, agent decision, 2026-09-23).** Vertical-landing pads
(DTSTRP) and the STRIP5A, STRIP6A and STRIP7A fields have no usable points,
because theirs fall off their own paving. So does an airport whose takeoff spot
is blocked or whose taxiway queue cannot fit. There the wing parks
staggered on the runway:

- The player is farthest down the runway, on the centerline. Wingmen stand
  behind, wingman 1 right of the centerline, 2 left, 3 right, 4 left, each
  250 ft farther back than the one before and 40 ft off the centerline.
- The last aircraft stands at the old single-aircraft start point, so the
  player is (wing size - 1) x 250 ft farther down the runway than before.
- Every slot must be on the airport's paving, on a landable surface, and clear
  of buildings (checked 6 ft above the surface). If one fails, the spacing
  tightens to 200, 150 and then 100 ft. If none works, the creator shows a
  notice and nothing launches.
- The 40 ft offset keeps the widest selectable wingspan, the F-14's 64 ft
  spread, within 72 ft of the centerline, inside a 150 ft wide runway.
  Neighbours stand on opposite sides, 80 ft apart across the runway and a full
  spacing apart along it; even 100 ft is more than an F-14's length, so no two
  aircraft overlap.

A player alone (wing size 1) uses the same placement: the takeoff spot, or the
old start point on a fallback field.

Every parked aircraft is stationary with the engine idling, gear and flaps
fully down, brakes applied, afterburner and autopilot off. Aircraft height
includes each aircraft's own wheel/CG clearance above the surface. The
wingmen fly the researched flight model, each with its own fixed seed (the
player 1, wingman *n* 1 + *n*), so a run repeats exactly. Nobody is dropped
from altitude, and restart restores the same layout. Existing B releases the
player's brakes and throttle controls begin the takeoff roll. How the AI
wingmen wait, taxi, take off, return and land is in the
[AI airfield rules](ai-airfield.md). No cold-start sequence is added. These
initial settings and the new UI layout are agent-selected host behavior, not
recovered retail Quick Mission rules.

Other selected wings keep the airborne launch path at the chosen altitude,
positioned relative to the player's slot, with the enemy aimed as described in
[Keeping the enemy on the map](#keeping-the-enemy-on-the-map). If any aircraft
starts in the air, the altitude must clear the airport ground by at least
100 feet; otherwise the creator shows a notice instead of launching. Parked
wingmen do not need that clearance. Ground mode ignores the altitude setting
for the parked wing. Airborne mode retains its existing behavior. Only the
default researched flight model supports ground start; legacy and restricted
native research modes report that incompatibility without silently changing
adapters. Missing or obstructed starts are rejected. Choosing ground start
selects the airport for tower commands but does not grant landing clearance or
announce that a landing has completed. The straight-flight fixtures
(`--fixture-wings`) park only the player. Every airborne mission aircraft, friendly or enemy, is
also raised to at least 5,000 ft above sea level and 1,000 ft above the
ground under its own start point, whichever is higher, so no group starts
inside the terrain (opinionated, requested by John 2026-09-29; before this,
enemies 50 nm from Jixian or Bahawalpur started inside the mountains).

Parked aircraft do not show on radar until they fly; see
[Radar and aircraft on the ground](#radar-and-aircraft-on-the-ground). The
headless `--ai-probe-ticks N --ground-start AIRPORT` probe gives the player two
wingmen so the parked-wing path is exercised.

[Ground-start validation](../baselines/ground-start.md) records creator launch,
restart, real runway support, takeoff probes and rendering checks.

[Reverse-cycling validation](../baselines/horizon-creator.md).

## Mission map

John, 2026-10-11: "Add a map button that shows a screen like the in-game map
and shows the bluefor and redfor starting locations", and "show the objective
as an APPROXIMATE location (an area/circle around it, not the exact target
positions)". An addition: retail's creator has no map.

- **Map** sits in the creator's top bar after "? Aircraft" (see above). It
  works in single player, in the lobby's editable creator and in the
  read-only view. Esc, Enter, Space or Close return to the creator
  unchanged; the in-game map's +/- zoom, arrows pan and Home resets.
- The screen is the in-game map's ground, frame and scale bar. Airfields are
  drawn one per airport on blue, red or grey tiles by side, as the Blue
  pilot sees it; the in-flight map uses the same tiles, swapped for a Redfor
  viewer ([airfield sides](flight-map.md#knowledge-and-detection)).
- **It builds in the background** (slice CL-R2, John 2026-10-11: opening the
  map took about 4 s on the menu's own thread). Clicking Map starts the
  build on a thread of its own and the creator carries on, saying "Building
  mission map..." in the notice box (the creator's retail notice style) until
  the map is ready, when it opens by itself. Esc, Cancel and OK give the build
  up (Esc gives up the build before it leaves the creator). If any setting
  changes while the map builds, that result is stale: it is dropped and the
  map is built again for the settings now in force, so the map that opens is
  always the draft's. The result is the same as before for the same
  settings and seed. The map and OK retain the seed the creator reserved.
- **Zoom, scroll and rocker** (opinionated, John 2026-10-11: "it should start
  zoomed in and have the scroll bars for the map, there should also be a rocker
  switch to zoom in and out"). The map opens zoomed in on the objective area
  and the starts: the closest view, no narrower than 50 nm across, that holds
  both starts and the whole objective circle with a fifth of the plot to
  spare; if no view holds them all, the first start (the player's) and the
  objective; with neither, one step in. Home returns to that view. A
  horizontal bar under the plot and a vertical bar beside it scroll the map:
  each has an arrow button at either end (a click moves the view an eighth
  of its width or height), a thumb that drags, and a track that pages by
  seven eighths on a click; the arrow keys still pan by a quarter. A retail
  vertical rocker (the debrief's, five frames, tilting on press) in the side
  panel zooms: its top half in, its bottom half out, with ZOOM IN and ZOOM
  OUT beside it; + and - do the same and tap the rocker. The bars are drawn
  flat in the menus' greys with arrow ends (retail has no horizontal bar or
  arrow-ended bar decoded); the rocker is retail's own art.
- **No black border at any zoom.** The widest zoom is the one at which the
  theater just fills the plot (as wide as the theater, or as tall, whichever
  is the tighter), then the in-game map's 800, 400, 200, 100, 50 and 25 nm
  steps that are narrower. The view is held inside the theater at every zoom
  and pan, so the picture always fills the plot.
- **BLUE START** and **RED START** mark where the player and Enemy Wing 1
  start: the airborne point with its heading, or the airfield's name for a
  ground start. Another wing that starts more than 3 nm from its side's start
  is marked **BLUE WING n** or **RED WING n**. A line under the map says
  whether each side starts in the air or on the ground.
- The positions are the flight's: opening the map builds the mission the
  creator would fly, using the seed the creator reserved. OK keeps
  it once, so the flight flies the layout the map showed (unless that layout
  is refused at launch and the next seed is taken).
  The lobby map uses the same seed that the host receives.
- **The objective area** (opinionated, John 2026-10-11; numbers fitted): a
  circle of 5 nm radius around a centre moved 0.5 to 2.5 nm off the targets'
  centroid, in a direction and by a distance hashed from the theater, the
  target and the seed, so the same mission shows the same circle, the targets
  are always inside it and never at its centre. It is drawn at least 7 pixels
  in radius. No ground target, no area.
- The map lists no units, so the [front line](surface-defenses.md#the-front-line)
  does not appear on it (it would give the objective's neighbourhood away);
  it is seen in flight and in the replay. Nor do the
  [area defenses](surface-defenses.md#area-defenses) (John, 2026-10-12:
  hidden from the map), here or on the in-flight map's mission markup; like
  any unit, a site the aircraft's sensors detect shows as a contact.
- **The route** (opinionated, John 2026-10-12, with the in-flight map's mission
  markup): a thin line from the Blue start to the edge of the objective area,
  in the map's one line style (a one-pixel ink line over a dark edge, as the
  in-flight map's leaders are). The map drew no lines before. A start inside
  the area has no line. The in-flight map's Mission layer draws the same area,
  starts and route from the same function ([in-flight map](flight-map.md#mission-markup)),
  so the two always agree.
- **Off the map**: a start or the area that lies beyond the theater is pinned
  at the edge it lies past, with the words OFF MAP beside it, as in the
  in-flight map.

## Group objectives

All six groups carry objective selectors in ordinary briefing sentences, using
the same `line` layout, font and beveled inline fields as the other mission
parameters. Wing counts use 14-pixel row spacing; objective sentences occupy
rows 201, 215 and 229 in each column. Separate survival-required/optional
sentences occupy rows 249, 263 and 277. Other mission parameters begin at row
301, with the optional airport row at 399, clear of the bottom buttons.

For example, `Your primary target is enemy group 1.` identifies that group as
the player's mission objective. `Your flight will use free fire.` permits any
observed eligible hostile without assigning every enemy as a primary objective.
This objective does not authorize firing at launch. Both sides begin in neutral
formation and follow [leader authorization](ai-awareness.md#formation-and-leader-authorization).
CAP, protection, self-defense, hold and mission inheritance remain available.
Click an objective field to choose, right-click to cycle backward, or use
Tab/arrows and Enter. Survival fields toggle between required and optional.
Inactive groups preserve both settings. [Assignment and target-label rules](ai-awareness.md#quick-mission-objective-stamps)
define the semantics; [validation](../baselines/mission-objectives.md) covers
styling and two opposing-group discrimination.

## Retail map variants

John selected retail map-detail expansion on 2026-09-23, and on 2026-09-29 asked
for the variants to be taken out of the player's list (`opinionated`, requested by
John on 2026-09-29): the imported `~` layouts mostly have one or two airports and
are incomplete. The location picker offers only the sixteen base theaters. The 59
imported MM variants stay loadable as a developer option, `--theater '~CODE'`
(see [DEVELOPMENT.md](../DEVELOPMENT.md#retail-map-detail-validation)), which
adds that one layout to the creator for probes and the battery. A variant loads
its scenery and airport list; nationality/target menus use the variant's base
theater tables; restart retains the selected layout. The renderer uses the
[static variant contract](terrain-detail.md), not a live campaign simulation.
The creator probe checks that the list holds exactly the sixteen base theaters,
then sweeps every layout, variants included.

## Paired-airport selection compatibility

The [single-field replacement](airports.md#single-fields-replacing-paired-tiles)
shows each pair once in both sides' airport lists. Mission files may still
name either old runway object ID. Profile draft version 2 records positions
in the merged lists; version 1 drafts load through their old side-filtered
lists and redirect a second-half selection to the base. New loadout fingerprints include the list revision. When no new-version
load exists, lookup reconstructs equivalent old airport-list positions by
source ID, including either former half and shifted STOL entries. If both
old halves have saved loads, the most recently flown compatible load wins.

The accepted creator sentences are also available in flight through the
[Mission brief](flight-map.md#mission-brief-tab), which uses this same sentence
builder. Pre-flight Load Ordnance and lobby Loadout remain unchanged.

## Multiplayer Player Choice

Opinionated requirements from John, 2026-10-16:

- The host can disable AI aircraft. Starts use the configurable
  [ready-player minimum](multiplayer-menus.md#minimum-players), updated by John
  on 2026-10-22; unfilled slots stay empty. SAMs, AAA and ground targets remain active. An idle human keeps
  neutral-control keepalive. Departure removes its live plane without a loss
  or kill, and a late join uses revive placement.
- Each wing can be Player Choice with an AI-flyable default. Human choices may
  use any flyable aircraft shared by the session, including helicopters,
  powered-lift aircraft and heavies. Existing content checks apply.
- Taking a Choice slot opens the existing aircraft selector, then Load Ordnance.
  Loadout reopens this flow. All peers build the accepted per-plane types.
- Every player waiting to revive gets loadout controls over the running flight,
  with timer and revive prompt visible. Choice slots also offer aircraft
  selection. John updated the in-flight presentation on 2026-10-16: use the
  in-game Escape menu widget, font, panel, highlight, sounds and input controls;
  the lobby continues to use the retail dialogs. The host applies revive-weapons and loadouts own/any. Escape cancels
  edits and input returns to flight after revival.
- Taking a Choice slot from its AI default removes that aircraft and spawns the
  chosen one at the existing revival distance, side bearing, altitude and speed.

Agent decisions: AI defaults stay mandatory with AI off; co-op without enemy
humans is allowed, with no enemy aircraft. Force Start requires the ready-player
minimum and can bring unready slot holders in, including with AI off.
Cancelling the lobby aircraft selector retains the previously displayed type
and proceeds to its ordnance page; cancelling ordnance sends nothing.

Current implementation uses a Fixed/Player choice control beside each wing's
existing aircraft field, which serves as its AI default, and an AI aircraft
control above the wing rows when hosting. The in-flight menu uses station display
order from the shared loadout model and provides internal fuel, Standard load
and Done. Capacity and weight validation use the same loadout code as the lobby
and host. The host applies revive-weapons to the accepted choice. Quantities
use single units up to a capacity of 20, otherwise ten-percent steps. Internal
fuel cycles from 10 to 100 percent in ten-percent steps. Done stages a choice
for the normal revive request; Escape at the top level discards edits.

John requested searchable scrolling respawn selectors on 2026-10-23, an
**opinionated** presentation addition. Selecting a station opens its stores
sub-menu. Its two groups and ordering share the pre-flight
[Load Ordnance catalog](ordnance-presentation.md): Air-to-air, then
Air-to-surface, with tanks and designator pods before surface weapons. Each
store's quantities stay together in increasing order. Empty is first. Only
compatible, valid choices appear, including shared-content, weight and
revive-weapons restrictions. Displayed ammunition counts include the revival
cut; the host still applies that cut once to the submitted full quantity.

Aircraft is one complete scrolling list in the creator/lobby's display-name
order (exact selection key breaks ties), restricted to available shared aircraft. Search matches a
case-insensitive substring of the aircraft name/designation or the store's
names/designation. For example, `16` finds F-16C, `hornet` finds F/A-18D and
`120` finds AIM-120. No matches shows an empty result and Enter does nothing.

Agent presentation decisions: both lists show 16 rows at once, including group
headings, in the existing flight font and colours with the retail red slider.
Click Search or press Tab to focus it; Tab toggles between search and results.
Typing edits the focused search, including M and spaces; Backspace removes a
character. Search is limited to 64 characters and edits reset to the first
result. Up/Down or controller D-pad moves through results, skipping headings;
Enter or controller A selects. Home/End reaches the ends, Page Up/Down moves
16 results, the wheel scrolls three rows per notch, and the red bar supports
track clicks and dragging. A result click selects it. Left/Right switches tabs.
Escape or controller B clears a nonempty search first, then closes the station
sub-menu, then cancels the unsaved page. In Aircraft, Escape after clearing
search cancels the page. M opens Mission only outside search focus; Mission...
remains available as a button in every view. Done and Cancel keep their meaning.

John requested persistent access on 2026-10-22, an **opinionated** multiplayer
addition. While waiting under `respawn revive` with lives remaining, the
in-flight Escape menu offers **Loadout...**, and **Aircraft...** only for a
Player Choice slot. These entries remain available throughout the delay and
until the player actually respawns. The initial loadout page has no timeout.
Done saves the aircraft and ordnance together; reopening either page restores
that saved choice, including fuel. Cancel or Escape discards only the current
unsaved edits. **Mission...** continues to open the same read-only brief.

Agent decisions: close the menus, then press Enter or controller A to request
revival. Enter inside a page selects its focused row; Done never respawns.
Enter before the delay ends is ignored, with no request queued. Holding Enter
cannot carry a menu selection into a revival confirmation. The countdown and
ready prompt show remaining lives when finite. Waiting spends no life. There
is no maximum wait and no automatic revival. The existing idle rules exclude
lost aircraft, so AI never takes a dead player's slot, even after the away
time; idle handoff still applies to live aircraft after revival. Ending the
mission or leaving the game still ends the wait. Lives, weapon restrictions,
loadout validation, `ai-slot`, and single-player end-of-flight rules are
unchanged. The existing revival request carries the saved choice, with no
protocol change.
