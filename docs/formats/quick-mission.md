# Active Quick Mission tables and dialog geometry

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


Recovered from the reviewed FA.EXE/SMS pair on 2026-09-14. This specifies source
data and consumers; it does not implement mission generation. Full extracted
catalogs remain ignored, not embedded in the application or tests.

## Active selector dispatch

`0x42e720` subtracts 3 from the field ID, bounds it against 59 and indexes the
60-entry table at `0x42e86c`. Static branches return double-NUL-terminated lists.
Aircraft branches construct filtered catalogs; ground targets dispatch by theater
through 16 entries at `0x42e95c`. The research reader accepts only the reviewed
constant-pointer grammar for static branches; it never executes code.

| Field IDs | Meaning | Active source contract |
| --- | --- | --- |
| 3, 20 | Friendly/enemy nationality | Same 60-entry list, source order/spelling preserved |
| 4, 7, 10; 21, 24, 27 | Wing counts | Six choices, 0 through 5 |
| 5, 8, 11; 22, 25, 28 | Wing skills | Four retail choices, novice through ace. The host appends [Dummy (400 KTS)](../spec/dummy-aircraft.md) as index 4. |
| 6, 9, 12; 23, 26, 29 | Wing aircraft | Dynamic GetNames; ID 6 uses player filter, others use other-wing filter |
| 13 | Theater | 16 source names; source index 14 is Ukraine |
| 14 | Altitude | 5,000 / 10,000 / 20,000 / 40,000 feet |
| 15 | Conditions | Dawn, clear, cloudy, overcast, foggy, sunset, night |
| 16 | Situation | Advantage, neutral, disadvantage; see [Situation](../spec/quick-mission-menu.md#situation) |
| 17 | Separation | 1 / 2 / 5 / 10 / 20 / 50 miles. The manual (p.19) gives these as nautical miles. The host appends 75, 100, 150, 200 and 300 miles; see [Separation](../spec/quick-mission-menu.md#separation). The list offers only the distances that fit the theater, target and start ([keeping the enemy on the map](../spec/quick-mission-menu.md#keeping-the-enemy-on-the-map)). |
| 18 | Load | Standard or custom |
| 19 | Air combat | Guns only or guns and missiles |
| 30 | Ground target | Theater-specific list, including none |
| 31, 32 | AAA / SAM strength | Four levels, none through heavy |

Theater target counts, including none, in source index order:

| Index | Theater | Count |
| ---: | --- | ---: |
| 0 | Baltics | 9 |
| 1 | Cuba | 7 |
| 2 | Egypt | 8 |
| 3 | Falklands | 7 |
| 4 | France | 8 |
| 5 | Greece | 6 |
| 6 | Iraq | 9 |
| 7 | Kuril Islands | 8 |
| 8 | North Vietnam | 10 |
| 9 | Pakistan | 7 |
| 10 | Panama | 7 |
| 11 | Persian Gulf | 7 |
| 12 | South Korea | 7 |
| 13 | Taiwan | 7 |
| 14 | Ukraine | 9 |
| 15 | Vladivostok | 8 |

Map source indices to shared theater identities explicitly and revalidate target
selection on theater changes. IDs 33–62 are multiplayer extensions: player
aircraft/wing assignments, revive settings, scoring and end conditions. They are
reported, not added to single-player setup. No BARCAP or runway/carrier-start
option occurs in this single-player dispatch; recover downstream contracts separately.

## Shared selector and interaction

`0x430680` calls the option producer and uses QUICK14 (reference at `0x430732`).
List control 3 receives the runtime list at `0x430751–0x430767`. Its compiled
record does not contain the active strings. QUICKB lists are legacy evidence.

The handler supports modal selection and direct cycling. Modifier/input state
and a field-ID class choose the branch; exact modifier naming remains unverified.
Cycling wraps in both directions. Modal results 1/2 distinguish accept/cancel;
dynamic aircraft lists are released after use. Do not assume every click opens
a popup. Availability helper `0x4300e0` returns true for one participant; other
branches concern multiplayer ownership, not complete mission validity.
Initialization includes RNG/catalog choices: screenshot values are not fixed defaults.

## Static DLG records

`ui::dialog::parse` resolves bounded PE/PL imports and HIGHLOW relocations,
identifies inert imported draw thunks, and reads reviewed action/list/dial/rocker
position fields. Text-draw payloads remain opaque. Action labels are literal
strings or imported label symbols. No callback or generic widget VM is executed.

Coordinates are in the original 640×480 canvas. Stored action widths are not
inferred face/hit widths or sprite-shadow extents.

| Dialog | Origin | Stored size |
| --- | --- | --- |
| QUIKMISS | (12, 88) | 615×378 |
| QUICK14 | (185, 100) | 270×370 |
| LOADORD | (115, 356) | 470×102 |

| Control | Local position | Absolute position | Stored width |
| --- | --- | --- | ---: |
| QUIKMISS OK | (375, 331) | (387, 419) | 85 |
| QUIKMISS Cancel | (480, 331) | (492, 419) | 85 |
| QUICK14 OK | (32, 337) | (217, 437) | 85 |
| QUICK14 Cancel | (127, 337) | (312, 437) | 85 |
| QUICK14 list | (20, 15) | (205, 115) | 230 |
| LOADORD Fly | (378, 58) | (493, 414) | 80 |
| LOADORD Select Plane | (248, 58) | (363, 414) | 100 |

LOADORD's page rocker is local (145,52), fuel rocker (432,0), and both dial records
share (33,38). QUICK14's rocker starts at (0,0); this is not established final
placement. Native setup/drawing determines state-dependent art and hit regions.
LOADORD's rectangle covers bottom controls, not the whole illustrated screen.

## Runtime briefing geometry

Compositor `0x42fde0` indexes 29 records at `0x4f1d30`, each five signed 16-bit
words: x, single-player y, multiplayer y, width, height. It adds the current
dialog origin; text drawing subtracts one from y. Most regions are 264×16.

Single-player sentence layout relative to QUIKMISS:

| Lines | x | y | Purpose |
| --- | ---: | --- | --- |
| 0 | 23 | 50 | Friendly nationality |
| 1–3 | 23 | 78, 92, 106 | Friendly wings |
| 4–7 | 23 | 134, 148, 162, 176 | Theater, altitude/conditions, situation, separation |
| 8–9 | 23 | 204, 218 | Load and weapon restriction |
| 10 | 328 | 50 | Enemy nationality |
| 11–13 | 328 | 78, 92, 106 | Enemy wings |
| 14 | 328 | 134 | Ground target/AAA/SAM, 264×48 |

Remaining rows serve multiplayer composition. Inline field rectangles come from
text rendering and are written into dialog controls; they are not fixed boxes in
the zeroed QUIKMISS records. Reconstruct them with original font metrics/wrapping.

## Ordnance card placement

`0x41a428–0x41a739` initializes catalog anchors at (68,108), alternates x=68/188,
and advances y by 68 per pair. Page arithmetic uses eight entries per page.
`0x41a75b–0x41aadd` initializes station anchors at (350,121), alternates x=350/469
and advances y by 71 per pair. Store art/text have offsets from these anchors;
anchors are not complete card/hit rectangles. This agrees with the photos' layout.

## Remaining gates

Dynamic aircraft filtering/defaults, mode visibility, post-selection dependencies,
exact modifier mapping, final rocker/dial/card art geometry and font hit regions
remain open. The app still uses its earlier fitted briefing. Wire verified data
into state and rendering before claiming screen parity.
[Validation](../baselines/menu-options-geometry.md).

## Initialization and dependent fields

`0x42f2e0–0x42f876` initializes the setup; the single-player field state is
`0x537360 + 4 * field_id`. These are initial values, not saved-session defaults.
The function seeds its RNG using prior RNG/clock/input state. A fixed diagnostic
seed would be authored, not a recovered retail startup seed.

| Fields | Initial single-player value |
| --- | --- |
| 4, 5 | One friendly Wing 1 aircraft, average skill |
| 7, 8, 10, 11 | Zero aircraft and novice skill for friendly Wings 2/3 |
| 6 | Random player-catalog entry, rejecting catalog flag `2` |
| 9, 12, 23, 26, 29 | Random other-wing catalog entries |
| 13 | Random theater from 16 entries |
| 14, 15, 16, 17 | 5,000 feet; clear; neutral; 5 miles |
| 18, 19 | Standard load; guns and missiles |
| 21, 22 | Random 2–3 enemy aircraft; random experienced/ace skill |
| 24, 25, 27, 28 | Zero aircraft and novice skill for enemy Wings 2/3 |
| 30, 31, 32 | No ground target, AAA or SAM |

`0x4308a0–0x4309f8` sets friendly nationality to index 0 (American) and enemy
nationality by theater. This runs at initialization and after theater changes.

| Theater | Enemy nationality index / label |
| --- | --- |
| Baltics, Kuril, Ukraine | 10 / Russian |
| Cuba | 33 / Cuban |
| Egypt | 14 / Islamic Egyptian |
| Falkland | 57 / Argentinean |
| France | 3 / French |
| Greece | 41 / Turkish |
| Iraq | 23 / Iraqi |
| North Vietnam | 20 / North Vietnamese |
| Pakistan | 37 / Indian |
| Panama | 34 / Panamanian |
| Persian Gulf | 24 / Iranian |
| South Korea | 9 / North Korean |
| Taiwan, Vladivostok | 2 / Chinese |

Keep the source mapping even where a geographic assumption would suggest another
country. These are nationality indices, not simulation allegiance IDs.

After an accepted selection, `0x430843–0x430893` enforces friendly Wing 1 count
at least one in single player. Thus its raw list contains zero, but an accepted
single-player setup cannot retain zero. If the ground target is none, both defense
fields are cleared. Next, a changed theater clears the target and resets both
nationalities. The order matters: this span does not clear defenses again after
clearing the target on a theater change. Do not claim immediate defense reset in
that particular path without tracing the outer update loop.

## Selector input contract

`0x430680–0x43089b` uses `GetKeyFlags() & 3`. The input producer at `0x411600`
assigns bits 1/2 to right/left Shift; Ctrl is 4 and Alt is 8.

- Scalar fields cycle directly; Shift opens the shared QUICK14 list dialog.
- Aircraft fields invert that choice: direct activation opens the list, Shift
  cycles. The field set includes the multiplayer aircraft extensions.
- Cycling wraps at either end. Shell button bit 1 selects increment; its absence
  selects decrement. Physical mouse-button naming still needs producer review.
- The popup populates control 3 with the current list/index. Action 1 accepts its
  index; action 2 closes without copying it into the draft.
- The post-selection constraints above run after acceptance, not popup cancel.

`0x4301a0–0x4303d4` refreshes text and copies changed fields to the corresponding
shadow setup. Conditions index 6 also sets the night flag at `0x4f1c80`.
This is not evidence of file persistence or an immutable launch transaction.

## Aircraft filter masks

Normal single-player initialization uses catalog mask `0x02400007`, player-list
mask `0x02400807`, and other-wing mask `0x02000807`. The low player bits are **7**
in this branch, not the **3** used in the era branches. A hidden modifier path
also exists; it must not become the normal player-availability rule.

With Fly all enabled, the four era choices use base masks `0x04000003`,
`0x08000003`, `0x10000003`, `0x20000003`; player lists add `0x800`.
Other-wing masks are `0x06000807`, `0x0a000807`, `0x12000807`, `0x22000807`.

GetNames uses 48-byte records: flags at 0, resource name at 4, display name at 17.
The filter span `0x41d209–0x41d383` checks category overlap and requested flag
requirements, including `0x1000` (Alt bypass), `0x400000`, era bits when Fly all
is set, `0x1000000`, `0x2000`, and `0x40000000`. Tilde-prefixed resources have
special modifier gates. Flag construction, pluralization and final catalog
identity mapping remain separate from these verified call masks; do not treat
any nonzero overlap as full eligibility, or catalog presence as flyable support.

## Runtime implementation checkpoint

The app now consumes the fingerprinted active tables through `ui::creator` and an
inert cache; scalar controls, list acceptance/cancel, nationality/target dependencies
and supported airborne setup are wired. Aircraft catalog metadata is imported at
runtime; native eligibility/era flags remain incomplete. Popup presentation and imported-only filtering are defined by the
[fitted menu specification](../spec/quick-mission-menu.md). [Behavior and acceptance](../baselines/creator-ordnance.md).

## Ground target templates and defenses

Recovered 2026-10-10 from `FA.EXE` (sha256 `e31560c2...244c`, data at
0x4f2e68 to 0x4f3988, generator code at 0x430a40 to 0x432240) and the 129
template missions in `FA_2.LIB` (sha256 `fb8b3021...6198`), with the object
census taken from the extracted archive. This section records the data; what the
player experiences, and what TORE builds from it, is specified in
[surface objectives and air defenses](../spec/surface-defenses.md). The
tables here are recorded as facts in code (names and small integers, like the
existing nationality table) with an ignored import test that cross-checks them
against `FA.EXE`; no bytes are embedded.

### How the retail generator builds the ground target

1. Field 30 (ground target, theater list) picks a template name from per-theater
   pointer tables at 0x4f3278 to 0x4f3488 (`~qunoth.M`, `~qusflt.M`, ...). Field
   13 (theater) chooses the table. Each template's position in the table matches
   the retail label order exactly (checked for all 16 theaters).
2. The writer at 0x431ab0 copies the template into `quick.M` (or `quickmp.M`)
   token by token, after a header it writes itself: `textFormat`, `map <code>`,
   `armplane`, `layer <day2|cloud1|fog1>[b|e|f|t|v].LAY 0` (which variant goes
   with which theater is not traced), `clouds`, `time`, and `usGroundSkill 1`,
   `usAirSkill 1`, `themGroundSkill 1`, `themAirSkill 1`.
3. `type <sam>` and `type <aaa>`: `Percent(table[strength])` with the tables
   below. A failed roll writes `type <nothing>`. The type is then a uniform random
   pick from the list for the enemy nationality's equipment group.
4. Night (conditions index 6) with an F117.PT or B2.PT in a friendly wing
   replaces the AAA list with ZSU-23 only and rewrites that object's `skill` to 0.
5. `<tank>`, `<afv>`, `<vehicle>`, `<small>`, `<hovercraft>`, `<destroyer>`,
   `<carrier>`, `<cargo>` and `<cruiser>` are always placed (no roll), drawn from
   the group lists by helper 0x432850.
6. `nationality` and `nationality2` tokens are rewritten to `nationality3
   <enemy nationality | 0x80>` (field 20). **`nationality3` tokens pass through
   unchanged**, so templates authored with `nationality3` keep their own owners:
   `~QIRRETR` keeps 40 American objects and `~QSPFRU` 21 Pakistani objects on the
   friendly side.
7. `quickpos x y z` sets an accumulator, and every `pos` adds to it with a count
   (0x431fe8 to 0x43209a). The "nothing" templates contain only `quickpos`. This
   is the engagement anchor; how the sum is consumed (centroid for the air
   battle?) is not traced. Templates spell it `quickpos` or `quickPos`; the
   compare is byte-exact, so case handling depends on the tokenizer at 0x432700
   (not traced), and a reader should accept both.
8. Template `flags` keep their bits: 0x80 objects are the destroy targets that
   the debrief counts. All template objects are on the ground (`pos` y 0) with
   `speed 0`; five templates give some objects `waypoint2` routes (the Ukraine
   armored column `~QUCOL`, the North Vietnam cargo ships `~QTCARGO`, a truck in
   `~QUFACT` and in the unreferenced `~QFACT` and `~QUBUNK`; see
   [routes](quick-templates.md#routes)).
9. If the ground target is none, both defense fields are cleared (see
   [initialization](#initialization-and-dependent-fields)).

### Defense rolls

| Field | Meaning | Table | Values by level (index 0 to 3) |
| ---: | --- | --- | --- |
| 31 | AAA strength | 0x4f31b8 | 0, 25, 60, 100 percent |
| 32 | SAM strength | 0x4f3148 | 0, 25, 60, 100 percent |

The retail sentence labels for both are "not", "lightly", "moderately" and
"heavily" (creator strings). Fields 31 and 32 are cleared when field 30 is none.

### Placeholder unit lists and equipment groups

Each placeholder draws from one list per equipment group, and the enemy
nationality (field 20) picks the group. The lists, the nationality to group
table and their `FA.EXE` addresses are recorded in
[the template facts](quick-templates.md#recorded-facts-from-faexe-102f).
M113 is an APC with a machine gun and is a legal `<sam>` pick in groups 3 and 4
and a legal `<aaa>` pick in groups 0, 1, 3 and 4; retail's list is preserved.

With the default enemy per theater ([initialization](#initialization-and-dependent-fields)):
Egypt and the Falklands draw group 3, France group 1, Greece group 4 and the
other 12 theaters group 2. Changing the enemy nationality changes the defenses and
ships, not the template.

### Template grammar

A template is the theater `.MM` text grammar with placeholders, `quickpos` and
`waypoint2` routes; its own bounded reader keeps the theater path's grammar
unchanged. The grammar, the routes, the owner fields and the census are in
[the template format](quick-templates.md#template-files).

### Template list and target names per theater

Retail label order is the menu order; index 0 is "nothing". Target objects are
the template's 0x80-flagged objects; a placeholder flagged 0x80 counts as a target
only if it survives its roll. Counts are before the defense roll. `(PT)` marks
aircraft. A "nothing" template holds only `quickpos`. 124 templates are reachable
from the menus.

#### Baltics (9 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QBNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a fleet of ships | `~QBFLT.M` | 20 | 1 KIEV | 0 / 0 | - |
| 2 | an airstrip | `~QBAIR.M` | 66 | 7 BNK6 | 10 / 10 | - |
| 3 | a bridge | `~QBBRD.M` | 34 | 1 BRD1 | 10 / 10 | - |
| 4 | a border checkpoint | `~QBXING.M` | 48 | 2 BUNKER, 1 FUEL, 3 STORE | 10 / 10 | - |
| 5 | an armored column | `~QBACOL.M` | 36 | 4 T72, 2 T80, 2 T90 | 10 / 10 | - |
| 6 | a forward airfield | `~QBFAIR.M` | 55 | 2 BUNKER, 2 FUEL, 2 MICRO, 3 STORE | 10 / 10 | - |
| 7 | a supply base | `~QBSPPY.M` | 45 | 8 FUEL, 2 FACTD | 10 / 10 | - |
| 8 | a super hardened C&C bunker | `~QBSHAR.M` | 47 | 1 BNK9 | 10 / 10 | - |

#### Cuba (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QCNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | an airstrip | `~QCFAIR.M` | 62 | 5 BNK8 | 10 / 10 | - |
| 2 | SCUD Launchers | `~QCSCUD.M` | 66 | 6 SCUD | 10 / 10 | - |
| 3 | a group of submarines | `~QCSUB.M` | 36 | 4 OSCAR | 10 / 10 | - |
| 4 | radar installations | `~QCLST.M` | 32 | 1 BNK1, 2 GCI, 3 MICRO, 1 MICROM, 4 PRDR1, 1 COMM | 10 / 10 | - |
| 5 | cargo ships carrying war supplies | `~QCCARG.M` | 6 | 4 `<CARGO>` | 0 / 0 | 4 cargo, 2 destroyer |
| 6 | a command HQ | `~QCCMHQ.M` | 51 | 3 BUNKER, 1 CMHQ2 | 10 / 10 | - |

#### Egypt (8 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QENOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a small fleet | `~QESFLT.M` | 7 | 3 `<CARGO>` | 0 / 0 | 3 cargo, 2 destroyer, 2 cruiser |
| 2 | a small airstrip | `~QESAIR.M` | 32 | 2 SHELT, 2 BNK3 | 10 / 10 | - |
| 3 | a large airstrip | `~QELAIR.M` | 45 | 4 BNK2, 4 BNK3 | 10 / 10 | - |
| 4 | a command HQ | `~QECMHQ.M` | 31 | 1 CMHQ1, 1 BNK1, 1 MICROM, 2 LTRACK | 10 / 10 | - |
| 5 | a radar installation | `~QERDRI.M` | 32 | 2 MICROM, 2 BNK1, 1 MICRO, 1 BNK2, 1 LTRACK | 10 / 10 | - |
| 6 | an armored column | `~QEARMOR.M` | 28 | 5 M1 | 10 / 10 | - |
| 7 | a canal defense | `~QECDEF.M` | 34 | 3 BNK1 | 10 / 10 | - |

#### Falklands (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QLFNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | cargo ships transporting weapons | `~QLFCARG.M` | 27 | 3 `<CARGO>` | 10 / 10 | 3 cargo, 4 destroyer |
| 2 | patrol boats | `~QLFPATR.M` | 24 | 4 CYCL | 10 / 10 | - |
| 3 | forward SAM sites | `~QLFSAM.M` | 75 | 6 ASA5 | 10 / 10 | - |
| 4 | Super Entendards on an airstrip | `~QLFFAIR.M` | 60 | 5 SPE (PT) | 10 / 10 | - |
| 5 | supply depot | `~QLFSTOR.M` | 64 | 4 FUEL, 2 FACTD, 2 BLDG2, 8 STORE, 1 BLDG1 | 10 / 10 | - |
| 6 | a command headquarters | `~QLFCMHQ.M` | 65 | 1 BNK9 | 10 / 10 | - |

#### France (8 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QFNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a fleet of ships | `~QFFLT.M` | 14 | 1 CLEM | 0 / 0 | 5 destroyer |
| 2 | a small airfield | `~QFSAIR.M` | 37 | 4 BNK8 | 10 / 10 | - |
| 3 | a large airfield | `~QFLAIR.M` | 44 | 6 BNK6 | 10 / 10 | - |
| 4 | a supply convoy | `~QFSUP.M` | 29 | 5 TRUCK | 10 / 10 | - |
| 5 | a radar installation | `~QFRDRI.M` | 36 | 2 BUNKER, 2 MICRO, 4 PRDR2 | 10 / 10 | - |
| 6 | a command HQ | `~QFCMHQ.M` | 31 | 2 STORE, 1 CMHQ1, 2 BUNKER, 1 MICROM | 10 / 10 | - |
| 7 | an aircraft factory | `~QFFACT.M` | 38 | 2 FACT1, 2 FACTD, 5 BNK5 | 10 / 10 | - |

#### Greece (6 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QGRNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | small airfield | `~QGRSAIR.M` | 85 | 4 SHELT, 2 FUEL, 4 STORE | 10 / 10 | 4 tank, 7 afv |
| 2 | patrol boats | `~QGRPATR.M` | 4 | 4 CYCL | 0 / 0 | - |
| 3 | radar stations | `~QGRRDR.M` | 113 | 4 SRDR1 | 10 / 10 | 16 tank, 18 afv |
| 4 | cargo ships | `~QGRCARG.M` | 10 | 3 `<CARGO>` | 0 / 0 | 3 cargo, 2 destroyer |
| 5 | an invasion force | `~QGRSTOR.M` | 116 | 7 `<TANK>` | 10 / 10 | 15 tank, 20 afv |

#### Iraq (9 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QIRNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | radar stations | `~QIRRDR.M` | 87 | 2 COMM, 4 KING | 10 / 10 | 17 afv, 11 tank |
| 2 | an airfield | `~QIRFAIR.M` | 85 | 2 BNK2, 4 BNK7 | 10 / 10 | 7 tank, 5 afv |
| 3 | a power station | `~QIRPOW.M` | 47 | 1 REACTR, 4 RELAY | 10 / 10 | 4 tank, 2 afv |
| 4 | command bunkers | `~QIRCCC.M` | 105 | 2 BNK1, 4 MICROM | 10 / 10 | 18 afv, 18 tank |
| 5 | an armored staging area | `~QIRARM.M` | 74 | 7 `<TANK>` | 10 / 10 | 7 tank, 17 afv |
| 6 | SCUD launchers | `~QIRSCUD.M` | 94 | 4 SCUD | 10 / 10 | 10 tank, 14 afv |
| 7 | a chemical weapons plant | `~QIRCWP.M` | 65 | 3 BLDG1, 3 FCTYA | 10 / 10 | 7 tank |
| 8 | troops withdrawing from Kuwait | `~QIRRETR.M` | 94 | 8 `<TANK>` | 10 / 10 | 28 tank, 20 afv |

#### Kuril Islands (8 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QKNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a small fleet | `~QKSFLT.M` | 5 | 1 `<CARRIER>` | 0 / 0 | 1 carrier, 1 cruiser, 3 destroyer |
| 2 | a large fleet | `~QKLFLT.M` | 16 | 1 `<CARRIER>` | 0 / 0 | 1 carrier, 4 cruiser, 5 destroyer, 2 cargo, 4 small |
| 3 | a group of hydrofoils | `~QKSCFT.M` | 26 | 6 `<HOVERCRAFT>` | 10 / 10 | 6 hovercraft |
| 4 | a group of subs in a harbor | `~QKSUB.M` | 15 | 4 OSCAR | 6 / 5 | - |
| 5 | a group of planes at an airstrip | `~QKPLNGR.M` | 22 | 4 YAK141 (PT) | 10 / 8 | - |
| 6 | a missile silo | `~QKSILO.M` | 34 | 7 SILO | 10 / 10 | - |
| 7 | a tank platoon | `~QKARMOR.M` | 28 | 8 T80 | 10 / 10 | - |

#### North Vietnam (10 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QTNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a barge flotilla | `~QTBARG.M` | 49 | 5 BARGE | 4 / 5 | - |
| 2 | cargo ships | `~QTCARGO.M` | 17 | 3 CARGO2 | 5 / 0 | - |
| 3 | a bridge | `~QTBRDG.M` | 46 | 1 BR2MID | 6 / 4 | - |
| 4 | a bunker complex | `~QTBUNK.M` | 63 | 6 BUNKER | 7 / 8 | - |
| 5 | a comm center | `~QTCOMM.M` | 36 | 1 COMM, 1 GCI | 6 / 9 | - |
| 6 | storage units | `~QTSTRG.M` | 36 | 6 STORE | 6 / 6 | - |
| 7 | a truck convoy | `~QTTRUCK.M` | 48 | 3 MISTRK, 6 TRUCK, 4 TANKER | 6 / 4 | - |
| 8 | a AAA emplacement | `~QTAAA.M` | 66 | 2 KS12, 2 KS19, 4 M1939 | 5 / 0 | - |
| 9 | SAM sites | `~QTSAM.M` | 69 | 4 SA2A | 0 / 4 | - |

#### Pakistan (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QSPNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | an airstrip | `~QSPFAIR.M` | 82 | 3 SHELT, 2 BNK2, 1 BNK3, 2 BARKSA | 10 / 10 | 4 tank |
| 2 | SAM Sites | `~QSPSAM.M` | 98 | 9 SA3 | 10 / 10 | 13 tank, 7 afv |
| 3 | an armored staging area | `~QSPASA.M` | 70 | 12 `<TANK>` | 10 / 10 | 12 tank |
| 4 | forward radar units | `~QSPFRU.M` | 86 | 5 LTRACK | 10 / 10 | 20 tank, 21 afv |
| 5 | a supply column | `~QSPSUP.M` | 50 | 5 TRUCK, 5 TANKER, 5 MISTRK | 10 / 10 | 7 tank, 8 afv |
| 6 | a command HQ | `~QSPCMHQ.M` | 61 | 3 SHELT, 1 BNK9 | 10 / 10 | 12 tank, 3 afv |

#### Panama (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QAPNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | an airport | `~QAPFAIR.M` | 59 | 5 BNK5 | 10 / 10 | - |
| 2 | a warship blockade | `~QAPBLK.M` | 27 | 2 `<DESTROYER>` | 10 / 10 | 3 destroyer |
| 3 | patrol craft | `~QAPPATR.M` | 55 | 5 SARAN | 10 / 10 | - |
| 4 | helicopter base | `~QAPHELO.M` | 67 | 2 BUNKER, 2 SHELT, 4 STORE | 10 / 10 | - |
| 5 | SAM sites | `~QAPSAM.M` | 76 | 4 SA2A | 10 / 10 | - |
| 6 | a command HQ | `~QAPCMHQ.M` | 99 | 1 CMHQ2, 5 MICROM, 3 BNK1, 6 MICRO | 10 / 10 | - |

#### Persian Gulf (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QPGNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | patrol boats along coast | `~QPGPATR.M` | 24 | 4 SARAN | 10 / 10 | - |
| 2 | an airport | `~QPGFAIR.M` | 74 | 4 BNK3 | 10 / 10 | - |
| 3 | sam sites | `~QPGSAM.M` | 127 | 9 SA3 | 10 / 10 | - |
| 4 | a small airfield | `~QPGSRUN.M` | 103 | 2 SHELT, 2 FUEL, 3 STORE, 1 CRANE, 1 BNK1 | 10 / 10 | - |
| 5 | radar stations | `~QPGRDR.M` | 70 | 1 COMM, 4 KING | 10 / 10 | - |
| 6 | warships in the gulf | `~QPGWSHP.M` | 36 | 2 `<DESTROYER>` | 10 / 10 | 2 destroyer, 5 cargo |

#### South Korea (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QNSNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | an airport | `~QNSFAIR.M` | 91 | 6 BNK8, 1 FCTYA | 10 / 10 | 1 tank |
| 2 | troops massing along border | `~QNSARM.M` | 86 | 8 `<TANK>` | 10 / 10 | 19 tank, 18 afv |
| 3 | a forward observation area | `~QNSFOA.M` | 97 | 2 LTRACK, 2 BNK1, 4 MICRO | 10 / 10 | 13 tank, 10 afv |
| 4 | a border checkpoint | `~QNSBORD.M` | 70 | 3 FUEL, 4 SHELT, 3 FACTD | 10 / 10 | 12 tank, 8 afv |
| 5 | an armored column | `~QNSCOL.M` | 46 | 8 `<TANK>` | 10 / 10 | 8 tank, 10 afv |
| 6 | a supply cache | `~QNSSUP.M` | 90 | 8 FUEL, 6 STORE | 10 / 10 | 17 tank, 16 afv |

#### Taiwan (7 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QWTNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | aircraft on an airstrip | `~QWTFAIR.M` | 66 | 5 J7E (PT), 4 Q5 (PT) | 10 / 10 | - |
| 2 | patrol boats | `~QWTPATR.M` | 6 | 6 SARAN | 0 / 0 | - |
| 3 | a group of hydrofoils | `~QWTHYDO.M` | 11 | 5 LCAC | 0 / 0 | - |
| 4 | a pair of warships | `~QWTWARS.M` | 11 | 2 `<DESTROYER>` | 0 / 0 | 2 destroyer |
| 5 | cargo ships | `~QWTCARG.M` | 9 | 3 `<CARGO>` | 0 / 0 | 3 cargo, 2 destroyer |
| 6 | offloaded vehicles | `~QWTLAND.M` | 75 | 8 `<TANK>`, 5 `<AFV>` | 10 / 10 | 3 cargo, 2 destroyer, 8 tank, 5 afv |

#### Ukraine (9 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QUNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a small fleet | `~QUSFLT.M` | 5 | 1 `<CARRIER>` | 0 / 0 | 1 carrier, 1 cruiser, 3 destroyer |
| 2 | a large fleet | `~QULFLT.M` | 16 | 1 `<CARRIER>` | 0 / 0 | 1 carrier, 4 cruiser, 5 destroyer, 2 cargo, 4 small |
| 3 | tanks hiding in a city | `~QUCITY.M` | 39 | 16 `<TANK>`, 11 `<AAA>`, 12 `<SAM>` | 12 / 11 | 16 tank |
| 4 | a factory | `~QUFACT.M` | 19 | 3 FCTYB | 4 / 3 | - |
| 5 | an airstrip | `~QUSTRIP.M` | 58 | 1 HANGRB, 2 SHELT, 3 HANGR, 1 TOWER, 3 STORE | 10 / 10 | - |
| 6 | an armored column | `~QUCOL.M` | 29 | 3 `<TANK>` | 10 / 10 | 9 tank |
| 7 | a nuclear reactor | `~QUNUKE.M` | 34 | 2 COLTWR, 1 RELAY, 2 REDOM, 1 HANGRB | 15 / 11 | - |
| 8 | a bridge | `~QUBRI.M` | 15 | 1 BRDMID | 3 / 7 | - |

#### Vladivostok (8 entries)

| # | Retail label | Template | Objects | Target objects (flag 0x80) | `<sam>` / `<aaa>` slots | Other placeholders |
| ---: | --- | --- | ---: | --- | --- | --- |
| 0 | nothing | `~QVNOTH.M` | 0 | - | 0 / 0 | - |
| 1 | a small fleet | `~QVSFLT.M` | 7 | 3 `<CARGO>` | 0 / 0 | 3 cargo, 2 cruiser, 2 destroyer |
| 2 | a small airfield | `~QVSAIR.M` | 43 | 4 BNK5 | 10 / 10 | - |
| 3 | a large airfield | `~QVLAIR.M` | 55 | 7 BNK6, 4 BNK8 | 10 / 10 | - |
| 4 | a command HQ | `~QVCMHQ.M` | 27 | 1 CMHQ1, 4 BUNKER, 1 MICROM | 10 / 10 | - |
| 5 | an armored column | `~QVARMOR.M` | 28 | 4 T72, 2 BMP2, 2 BTR80 | 10 / 10 | - |
| 6 | a radar installation | `~QVRDRI.M` | 30 | 2 BUNKER, 1 MICRO, 2 STORE | 10 / 10 | - |
| 7 | a supply column | `~QVSUP.M` | 29 | 5 TRUCK | 10 / 10 | - |

Unreferenced templates (not reachable from any menu): `~QFACT` (a copy of
`~QUFACT`), `~QUBUNK` (Ukraine bunker), `~QURADAR` (Ukraine radar, every object a
target), `~QMANOTH` and `~QOSNOTH` (no matching theater). That makes 129 files.

The surface objects outside the Quick Mission are used far more heavily by the
other 388 missions (SA-6 814 placements, SA-15 596, ZSU-23 666, T-80 892, M1 962,
Ticonderoga 360, Sacramento 426, Eisenhower 137, Iowa 148). Ships move there:
947 ship waypoint routes (`waypoint2` blocks tied to the ship with `w_for
<alias>`) with `w_speed` 16, 33, 50 or 100 and `w_wng 1 0 2048 0`. No template
names HAWK, ROLAND, MIS, SA-7, SA-9, SA-13, SA-14, SA-15, SA-16, Iowa, Kitty Hawk,
Wasp or Sacramento directly; most can be drawn by a placeholder (HAWK, SA-16 and
Kitty Hawk cannot). KS-12, KS-19 and `A_M1939` appear only in North Vietnam
layouts and missions.

## Local creator draft airport-list revision

The local `quick-mission-v1.conf` file now writes `tore-quick-draft 2`.
The filename is retained so existing profiles are discovered. Version 1 is
accepted: airport-choice positions are resolved through the former
side-filtered list, then former second-half runway IDs map to the merged
field. Version 2 positions use the merged lists. Mission text and network
mission fields still use the existing runway object IDs, with old IDs
accepted by the world. This is a local draft revision, not a wire-format
change. See [airport selection compatibility](../spec/quick-mission-menu.md#paired-airport-selection-compatibility).

Custom-loadout history keeps its existing file format. New mission fingerprints
use draft revision 2. Backward lookup reconstructs the equivalent revision-1
keys by source runway identity, preventing an index shift from selecting an
unrelated airport's loadout. A saved revision-2 load takes precedence; otherwise
the most recently flown compatible old load is restored.
