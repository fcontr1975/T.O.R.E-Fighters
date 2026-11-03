# Aircraft ordnance source rows

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Research and implementation evidence, 2026-10-05. This pass audits the 36
registered retail PT identities at workspace base
`c1962c5d1250e43a60bd2566af1d8cdb0abf3ac7`. F/A-XX reuses the F22N source rows
and adds no retail identity. The [station behavior spec](../spec/ordnance-presentation.md#source-stations-and-availability)
owns the player contract. No original module was executed or retail gameplay
comparison performed. No aircraft rendering changes belong to this pass.

## Build identity and method

The source PT/JT records were parsed by the existing bounded Rust readers from
local FA_2.LIB extraction. Each source slot was tested against every installed JT
and against the current supported-name list with the shared loading component.
The local machine-readable evidence is `.local/ordnance-audit/slots.tsv`; retained
source files and derived scratch reports remain ignored. Source-build hashes:

| File | SHA-256 |
| --- | --- |
| `FA_2.LIB` | `fb8b30216e739292489d4872cc440debec334e14f8b9a3d0e340092445246198` |
| `FA.EXE` | `e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c` |
| `FA.SMS` | `e550a67e2dca36c583a5e7963db96da7a833e79a2b5cd13e5da4c2d966168de0` |

Static instruction references below use the hash-reviewed FA.EXE/SMS pair,
through the existing local disassembly. Only the stated short spans were
reviewed. The [existing ordnance notes](ordnance-menu.md) own previously recovered
geometry, category, quantity-step and sound facts.

## Source identity is a row, not a pylon count

A HARDPOINT contains 12 BRF tokens: flags; x/y/z; slew heading/pitch; slew limits;
default resource; byte maxWeight; word maxItems; byte name. The final byte maps to
Centerline, Fuselage, Internal Gun, Internal Bay, Wing and Wingtip. It is not the
maxWeight byte. A source ordinal stays stable even when the default resource is
null, a tank or equipment. Names are location labels, not proof of physical
mount count. maxItems is a quantity/capacity field, not the number of pylons.

At 0x41a12a..0x41a14e, original ArmPlane skips a row when fixed bit 8 is set and
the loaded resource is not projectile type 7. Other rows proceed regardless of
JT/GAS/SEE/ECM/null default class. At 0x41a31f the original source ordinal is
stored in the display list; 0x41a7aa uses that ordinal for the card. This
establishes one aggregate card per selected source row. It does not establish
that a multi-item row should be expanded into left/right physical pylons.

## Per-aircraft audit matrix

All indices below are zero-based original PT ordinals. W is the old default-JT
weapon map. T is the pre-pass tank-candidate map. Extra W names omitted rows
that accept at least one currently supported JT under normal source compatibility.
Roles count default JT/GAS/SEE/ECM/null entries. UI is the unified editable-row
count after adding Extra W and retaining occupied auxiliary hardware as locked.
Gaps counts distinct imported, source-compatible JT names not in the live
supported-name list. It does not count these weapons as implemented.

| PT | Total | Roles JT/GAS/SEE/ECM/null | W | T | Extra W | UI | Gaps |
| --- | ---: | --- | --- | --- | --- | ---: | ---: |
| `F18.PT` | 9 | 5/1/2/1/0 | 3, 5, 6, 7, 8 | 4, 5, 6, 7 | None | 6 | 36 |
| `RAFALE.PT` | 9 | 5/0/3/1/0 | 3, 5, 6, 7, 8 | 4 | None | 5 | 37 |
| `F14.PT` | 8 | 4/1/2/1/0 | 3, 4, 6, 7 | 5, 6 | None | 5 | 36 |
| `A4E.PT` | 7 | 3/1/2/1/0 | 3, 4, 6 | 4, 5 | 5 | 4 | 27 |
| `F31.PT` | 9 | 4/0/3/1/1 | 4, 6, 7, 8 | 5 | 5 | 5 | 32 |
| `MIG29.PT` | 8 | 4/0/3/1/0 | 4, 5, 6, 7 | 5, 6, 7 | None | 4 | 32 |
| `SU27.PT` | 9 | 5/0/3/1/0 | 4, 5, 6, 7, 8 | 5, 6, 7, 8 | None | 5 | 32 |
| `MIG21.PT` | 7 | 3/0/2/1/1 | 3, 5, 6 | 4, 5, 6 | 4 | 4 | 16 |
| `SU25.PT` | 9 | 4/0/3/1/1 | 4, 6, 7, 8 | 5, 6, 7, 8 | 5 | 5 | 36 |
| `MIG23.PT` | 9 | 4/0/2/1/2 | 3, 6, 7, 8 | 4, 6, 7, 8 | 5 | 6 | 35 |
| `SU35.PT` | 9 | 5/0/3/1/0 | 4, 5, 6, 7, 8 | 5, 6, 7 | None | 5 | 36 |
| `F22.PT` | 8 | 4/0/3/1/0 | 3, 5, 6, 7 | None | None | 4 | 36 |
| `F22N.PT` | 8 | 4/0/3/1/0 | 3, 5, 6, 7 | None | None | 4 | 36 |
| `C130.PT` | 1 | 0/0/1/0/0 | None | None | None | 0 | 0 |
| `AC130.PT` | 7 | 3/0/3/1/0 | 4, 5, 6 | None | None | 3 | 0 |
| `E3.PT` | 3 | 0/0/2/1/0 | None | None | None | 0 | 0 |
| `IL76.PT` | 3 | 0/0/2/1/0 | None | None | None | 0 | 0 |
| `E2.PT` | 3 | 0/0/2/1/0 | None | None | None | 0 | 0 |
| `AV8.PT` | 9 | 4/0/3/2/0 | 4, 5, 6, 7 | 5, 8 | None | 4 | 36 |
| `YAK141.PT` | 6 | 3/0/2/1/0 | 3, 4, 5 | 4, 5 | None | 3 | 33 |
| `V22.PT` | 4 | 1/0/2/1/0 | 3 | None | None | 1 | 0 |
| `AH64.PT` | 8 | 3/0/3/1/1 | 3, 5, 6 | 4 | None | 4 | 24 |
| `MI24.PT` | 3 | 2/0/1/0/0 | 1, 2 | None | None | 2 | 0 |
| `CH47.PT` | 1 | 0/0/1/0/0 | None | None | None | 0 | 0 |
| `MIG17F.PT` | 4 | 2/1/1/0/0 | 1, 2 | 3 | 3 | 3 | 18 |
| `F4B.PT` | 7 | 3/1/2/1/0 | 3, 4, 5 | 3, 4, 5, 6 | None | 4 | 36 |
| `F4J.PT` | 8 | 3/1/2/1/1 | 3, 5, 6 | 4, 5, 6, 7 | 4 | 5 | 36 |
| `F4E.PT` | 8 | 3/1/2/1/1 | 3, 5, 6 | 4, 6, 7 | 7 | 5 | 36 |
| `F4.PT` | 8 | 3/1/2/1/1 | 3, 5, 6 | 4, 6, 7 | 7 | 5 | 36 |
| `A7.PT` | 7 | 4/0/2/1/0 | 3, 4, 5, 6 | 5, 6 | None | 4 | 29 |
| `F15.PT` | 6 | 3/0/2/1/0 | 3, 4, 5 | 4, 5 | None | 3 | 37 |
| `F16C.PT` | 9 | 4/1/3/1/0 | 4, 6, 7, 8 | 5, 6 | None | 5 | 34 |
| `F104.PT` | 9 | 4/2/2/1/0 | 3, 6, 7, 8 | 4, 5, 6, 7, 8 | None | 6 | 32 |
| `A10.PT` | 9 | 5/0/3/1/0 | 3, 5, 6, 7, 8 | None | None | 5 | 28 |
| `B747.PT` | 1 | 0/0/1/0/0 | None | None | None | 0 | 0 |
| `A310.PT` | 1 | 0/0/1/0/0 | None | None | None | 0 | 0 |

The maximum aggregate editable-row count is six for these 36 records. Transport
and airborne-radar aircraft with no carrier rows keep zero; do not invent mounts
for them. Nine omitted source rows accept supported weapons. No omitted
SEE/ECM default row accepted a supported JT in the normal-compatibility probe.

## Omitted weapon-capable rows

These examples use actual source compatibility, not real-aircraft expectations.
They establish source placement eligibility; weapon behavior and
current sensor/target prerequisites remain separate.

| PT / source slot | Default | Location | Flags | maxItems / maxWeight | Example supported compatible JT |
| --- | --- | --- | --- | --- | --- |
| `A4E.PT` / 5 | `F150.GAS` | Wing | `0x1785` | 4 / 32 | MK82.JT, AGM65G.JT |
| `F31.PT` / 5 | `NONE` | Centerline | `0x0681` | 1 / 15 | AIM9M.JT |
| `MIG21.PT` / 4 | `NONE` | Centerline | `0x07e1` | 1 / 10 | AIM120.JT, AIM7.JT |
| `SU25.PT` / 5 | `NONE` | Centerline | `0x07e5` | 1 / 20 | AS7.JT, MK82.JT |
| `MIG23.PT` / 5 | `NONE` | Fuselage | `0x0085` | 4 / 6 | AIM9M.JT |
| `MIG17F.PT` / 3 | `F150.GAS` | Wing | `0x0705` | 2 / 40 | MK82.JT |
| `F4J.PT` / 4 | `NONE` | Centerline | `0x0785` | 1 / 40 | AIM9B.JT, MK82.JT |
| `F4E.PT` / 7 | `NONE` | Wing | `0x07e5` | 2 / 20 | AIM120.JT, AIM7.JT |
| `F4.PT` / 7 | `NONE` | Wing | `0x0765` | 2 / 20 | AIM7.JT, AGM88.JT |

F4.PT slot 7 has 0x0765 and lacks the IR/laser compatibility mask 0x80; it must
not inherit the F4E slot's IR choices. SUU16 appears source-compatible on
some rows, but the current host's fitted gun-pod behavior is restricted to the
reviewed source-installed F4J slot. A compatibility match alone does not extend
that gameplay support.

## F-14 exact current-to-source mapping

F14.PT has eight source records, not eight weapon cards or eight proved pylons.
Three fixed equipment rows are not ordnance carriers. The remaining five rows
are the complete original aggregate editing group. The tank row omitted from
the weapon-only map is source slot 5, location Fuselage, maxItems 2. That is one
source row containing a tank quantity, not evidence for two separately editable
source rows.

| Source slot | Default | Location | Flags | maxItems | maxWeight | Old weapon index | Old tank index |
| ---: | --- | --- | --- | ---: | ---: | --- | --- |
| 0 | `VIS340.SEE` | Fuselage | `0x0008` | 1 | 0 | - | - |
| 1 | `F14R.SEE` | Fuselage | `0x0008` | 1 | 0 | - | - |
| 2 | `F14.ECM` | Fuselage | `0x0008` | 1 | 0 | - | - |
| 3 | `M61.JT` | Internal Gun | `0x0008` | 675 | 0 | 0 | - |
| 4 | `AIM54C.JT` | Fuselage | `0x0155` | 4 | 40 | 1 | - |
| 5 | `F250.GAS` | Fuselage | `0x0605` | 2 | 38 | - | 0 |
| 6 | `AIM120.JT` | Wing | `0x07f5` | 2 | 30 | 2 | 1 |
| 7 | `AIM9M.JT` | Wing | `0x0485` | 2 | 5 | 3 | - |

Before this pass, source slot 6 appeared separately in the weapon and tank
views. It remains one source row and may hold only one selected store class. Its
position is 18 / -20 / 14 source units. Slot 5 is 9 / -23 / 20. Tank capacity,
fuel and shell values live in the [F-14 source review](aircraft.md#f-14-default-external-fuel).
Normal source compatibility permits no supported JT on dedicated tank slot 5.
It does permit supported JTs and compatible tanks on slot 6.

## Occupied auxiliary rows and sensor prerequisites

RAFALE.PT slot 4 defaults to AAS38.SEE with flags 0x0601. AV8.PT slot 8 defaults
to ALQ167.ECM with flags 0x0601. Both are nonfixed original screen rows and
source tank-compatible, but the current host cannot remove/rebuild the
selected auxiliary equipment suite when substituting a tank. Exposing them as
empty tank rows can overlap retained hardware. This pass locks those occupied
rows rather than claiming equipment replacement support. Fixed sensors and
countermeasures remain separate from editable ordnance.

The current catalog tests imported presence, supported-name status and station
compatibility. It does not require a current target or sensor lock. In the host,
supported-radar missiles with radar power on require actual aircraft support;
missing radar yields NoRadar. The requested radar-off dumb-release rule is
separate. Own IR seekers do not require an aircraft FLIR installation. Emitter
weapons require a compatible emitting surface target. None of the radarless
registered aircraft admits a supported-radar missile under normal source
compatibility in this audit; Cheat can bypass the normal station mask and may
show such a store without creating radar hardware. A selectable store must not
be described as guaranteed guided operation.

## Imported but not live-supported compatibility gaps

The 37 distinct source-compatible imported JT names outside the live list are:

`AA6.JT`, `AA9.JT`, `AEMP1.JT`, `AGM45.JT`, `AGM65A.JT`, `AGM84A.JT`, `AGM84E.JT`, `AM39.JT`, `AS14.JT`, `AS15.JT`, `AS16.JT`, `AS30.JT`, `AT12.JT`, `CBU87.JT`, `CBU89.JT`, `FAB1000.JT`, `FAB250.JT`, `FAB500.JT`, `GBU10.JT`, `GBU10A.JT`, `GBU28.JT`, `GBU29.JT`, `GBU29P.JT`, `GBU30.JT`, `LAU10.JT`, `MK20.JT`, `MK82AIR.JT`, `MK82HD.JT`, `MK82P.JT`, `MK82P3.JT`, `MK84.JT`, `PAVEWA3.JT`, `PAVEWAY.JT`, `PL10.JT`, `PL7.JT`, `RBK250.JT`, `RBK500.JT`.

These include additional radar/IR missiles, passive emitter and surface weapons,
laser/designator-dependent cases, bomb/cluster variants and another rocket pod.
The [missile inventory](../spec/missiles.md#first-pass-inventory-matrix) owns
candidate guidance and hold decisions. A name, physical fit or imported JT
never establishes implemented behavior. The per-aircraft gap counts above
include only names with a positive normal capacity, not every catalog name.

## Paired-point counts and physics

Research review, 2026-10-25, using the FA.EXE, FA.SMS and FA_2.LIB identities
above. The reviewed `HARDCanLoad` span below calculates one total capacity
for the source row. Pair flag 0x4 does not multiply that capacity or its
weight by two. `ArmPlane` creates one card per source ordinal. A paired
point's maximum of 3 therefore means 3 stores for that aggregate row, not
3 on each wing. The maximum can come from the weight quotient, rather than
an odd `maxItems` in the PT record. This distinction matters for the Hornet.

The [release-state evidence](weapons.md#release-state) reads the same row's
remaining count to alternate the lateral release offset. Thus retail counts
do have a release-side consequence, but do not establish a physical layout
of loaded stores. John reports that retail did not render those pylons or
stores. No retail execution or image comparison was performed in this pass.
The old T.O.R.E right-extra layout was an inference from release alternation,
not a recovered retail visualization rule.

The user-owned manual, printed pp.17 and 59 (local transcription
`.local/manual/manual.txt`, lines 1291-1316 and 3933-3942), describes
individual-count editing, total aircraft/weapon/fuel weight, and performance
penalties from ordnance weight and drag. It does not specify left/right
mass, CG or drag. The requested `.local/missile-update/manual.txt` was absent;
the existing manual transcription was used instead. AS-16 Kickback is a
separate store from AGM-84A Harpoon (manual weapon table, printed p.129).

The [reviewed flight loading](native-flight.md#force-assembly-now-translated)
uses aggregate mass buckets selected by hardpoint flag 1, not left/right
position, at `0x4516f2..0x451814`. Its loaded drag at `0x47a690/0x4784a0`
uses load percentages. The documented equipment resolver at `0x452770`
multiplies weapon quantity by weight, with pod/ammunition handling, and does
not double a paired count. This supports aggregate row weight and drag,
not a per-wing CG calculation, in the recovered path.

**Unknown:** whether another retail flight path computes a lateral store
moment or asymmetric drag. The reviewed loading, weight and release spans
do not prove absence across the executable. Next research step: trace the
flight consumers of the row quantities and `HARDStoreWeight`, specifically
any lateral-offset multiplication feeding roll or yaw. T.O.R.E currently
uses aggregate payload mass and aggregate load drag, with no store-CG term.
The [even-paired-load contract](../spec/stores-on-pylons.md#even-paired-loads)
is explicitly opinionated and does not depend on closing that research.

## Quantity, weight and pod follow-up

HARDStoreWeight 0x452940..0x45296c dispatches type 7 to OBJECT weight +0x57,
GAS type 8 to empty weight +5 plus fuel weight +8, and other equipment types to
word weight +5. HARDCanLoad 0x452bdd..0x452bfd uses
`min(maxItems, maxWeight * 100 / signed-word store weight)` for nonzero class and
weight. There is no paired multiplier in that reviewed weight branch. This
resolves that arithmetic, but does not establish the original default-loader
quantity or a physical pylon pairing rule.

HARDLoad 0x452c60..0x452c6d calls capacity only for count zero; an explicit
nonzero count uses the supplied number. The GAS branch 0x452cd1..0x452cde stores
fuel weight times installed count. The known A4E/F104 source-default versus
capacity discrepancies therefore still need the caller's initial/default-load
normalization, not a guessed paired weight factor. Keep the documented fitted
host default-capacity rule until that behavior is specified.

HARDPodHack 0x45377f..0x4537b0 multiplies or divides a projectile's stored
quantity by word projsInPod +0xaa. It does not divide by actualRoundsPerGame in
this span. For SUU16 this transforms one pod to 1200 stored projectile units.
The earlier host 600-logical-unit interpretation remains explicitly fitted;
confirm the player firing/HUD counter consumer before replacing it with a
claimed original displayed-ammunition number. No unrelated rocket-pod behavior
is changed by this evidence-only finding.

Unknown: original default-load normalization, player pod-counter display/debit
units, meaning of any physical pairing flag, stock/year/airbase lifecycle and
complete auxiliary-equipment replacement. Next source steps are the narrow
initial/default loader and player counter consumers. Full routine closure is
not needed to implement stable source-row editing with honest support limits.

## Hardpoint positions

Research for bug B3, 2026-10-12 (slice BP), from the same `FA_2.LIB` PT
records, read as text with the field order above. The location byte maps
0 Centerline, 1 Fuselage, 2 Internal Gun, 3 Internal Bay, 4 Wing, 5 Wingtip.
x / y / z are signed source words, right / up / forward of the aircraft's
reference point; the host divides by three for feet, the same conversion the
AC-130 gun mounts use ([aircraft variety](aircraft-variety.md)). One record
carries one position. Retail mirrors its lateral offset for flag 0x4 records
([release state](weapons.md#release-state)), so a flag 0x4 record's position is
its right-hand pylon. The records hold no pylon geometry, rack layout or store
attach point, and no retail shape draws a pylon, rack, rocket pod or fuel
tank (the weapon shapes in `FA_2.LIB` are missiles and bombs only; `.GAS`
records name no shape). Those are unknown in retail; the drawn ones are fitted
([stores on pylons](../spec/stores-on-pylons.md)).

What the table shows, by inspection:

- Wingtip rows sit at or just above the reference height (y 1 to -3) at the
  wing's tip: 19.3 ft out on the F/A-18D, 19.7 ft on the Rafale C.
- Wing rows step outward in about 15 to 17 unit (5 to 6 ft) increments, mostly
  a few feet aft of the reference point and 2 to 7 ft below it.
- Fuselage pairs sit 3 to 5 ft either side of the centreline, 5 to 8 ft below.
- The F-22's internal bay rows are one-sided (no flag 0x4): row 5 at x -10
  (left bay), row 6 at x +10 (right bay); its side bays (row 7) are flagged.
- Two Wing rows lack flag 0x4: `A4E.PT` row 4 (four Mk 82s at x 8) and
  `MI24.PT` row 2 (four AT-2s at x 25, flags `0x0188` with the fixed-slot
  bit). Retail releases those from one side; T.O.R.E pairs every Wing and
  Wingtip row (agent decision, [stores on pylons](../spec/stores-on-pylons.md#rules)),
  except a one-sided wing row less than 3 ft out, the A-4E's, which it hangs as a
  centreline rack (slice PY2, [the A-4E](../spec/stores-on-pylons.md#the-a-4e)).
  T.O.R.E's default A-4E load puts one 150 gallon tank on that row in place of the
  Mk 82s and two (not four) on row 5, the real aircraft's three tanks; the rows'
  capacities stay the source's (slice PY3).
- `F4E.PT` and `F4.PT` (F-4G) row 5 records their four Sparrows at x 0 with flag
  0x4: retail releases them from the centreline. T.O.R.E hangs them 3 ft either side,
  in the Sparrow wells (fitted, slice PY2).
- The records' heights (y) are where retail releases stores; T.O.R.E hangs and
  releases every wing store 1 ft under the model's underside over its pylon, a
  fuselage or centreline store 2 inches under it, and a wingtip missile on the tip's
  edge instead (opinionated, John 2026-10-13), so the y column is not where a store leaves in
  T.O.R.E ([pylon depth](../spec/stores-on-pylons.md#pylon-depth)). Fuselage rows
  with two stores a side hang them one behind another, which no record says either.
- `F14.PT` rows 4 to 7 hang at the real F-14's stations, not at these positions:
  the Phoenixes in the tunnel 1.25 ft out, the tanks under the intake nacelles,
  the AIM-120 under the glove pylon and the AIM-9 on its shoulder rail
  (opinionated, John 2026-10-14; [the F-14](../spec/stores-on-pylons.md#the-f-14)).
- `YAK141.PT` rows 4 and 5 hang at the Yak-141's inner and outer underwing pylons, 7.5 ft out
  2.8 ft back and 11 ft out 4.4 ft back, not at these positions (row 5's 16.7 ft at 0 ft forward is
  ahead of the swept wing, by the tip) (opinionated, John 2026-10-14;
  [the Yak-141](../spec/stores-on-pylons.md#the-yak-141)).
- `SU27.PT` and `SU35.PT` rows 5 to 7 hang at the Flankers' real stations: the centreline row
  in the tunnel one behind the other, row 6 under the intake ducts, row 7 one missile each on the
  middle and outer underwing pylons (opinionated, John 2026-10-14;
  [the Su-27 and Su-35](../spec/stores-on-pylons.md#the-su-27-and-su-35)).

Weapon and tank rows of the 36 registered identities (sensor, ECM and gun rows
omitted; Count is the default quantity):

| PT | Row | Location | Flags | x / y / z | Feet right / up / fwd | Pair (0x4) | Default | Count |
| --- | ---: | --- | --- | --- | --- | --- | --- | ---: |
| `F18.PT` | 4 | Fuselage | `0x0605` | 0 / -15 / 0 | 0.0 / -5.0 / 0.0 | yes | F150.GAS | 1 |
| `F18.PT` | 5 | Fuselage | `0x07e5` | 10 / -15 / 0 | 3.3 / -5.0 / 0.0 | yes | AIM120.JT | 2 |
| `F18.PT` | 6 | Wing | `0x07e5` | 25 / -6 / -3 | 8.3 / -2.0 / -1.0 | yes | AGM65G.JT | 4 |
| `F18.PT` | 7 | Wing | `0x07e5` | 41 / -6 / -6 | 13.7 / -2.0 / -2.0 | yes | AGM65G.JT | 4 |
| `F18.PT` | 8 | Wingtip | `0x0085` | 58 / 1 / -9 | 19.3 / 0.3 / -3.0 | yes | AIM9M.JT | 2 |
| `RAFALE.PT` | 5 | Fuselage | `0x05e5` | 10 / -20 / 0 | 3.3 / -6.7 / 0.0 | yes | AGM65G.JT | 4 |
| `RAFALE.PT` | 6 | Wing | `0x05e5` | 20 / -20 / 8 | 6.7 / -6.7 / 2.7 | yes | MICA.JT | 2 |
| `RAFALE.PT` | 7 | Wing | `0x05e5` | 36 / -20 / -18 | 12.0 / -6.7 / -6.0 | yes | R530.JT | 2 |
| `RAFALE.PT` | 8 | Wingtip | `0x0085` | 59 / -3 / -24 | 19.7 / -1.0 / -8.0 | yes | R550.JT | 2 |
| `F14.PT` | 4 | Fuselage | `0x0155` | 9 / -23 / 24 | 3.0 / -7.7 / 8.0 | yes | AIM54C.JT | 4 |
| `F14.PT` | 5 | Fuselage | `0x0605` | 9 / -23 / 20 | 3.0 / -7.7 / 6.7 | yes | F250.GAS | 2 |
| `F14.PT` | 6 | Wing | `0x07f5` | 18 / -20 / 14 | 6.0 / -6.7 / 4.7 | yes | AIM120.JT | 2 |
| `F14.PT` | 7 | Wing | `0x0485` | 26 / -16 / 5 | 8.7 / -5.3 / 1.7 | yes | AIM9M.JT | 2 |
| `A4E.PT` | 4 | Wing | `0x1781` | 8 / -16 / -6 | 2.7 / -5.3 / -2.0 | no | MK82.JT | 4 |
| `A4E.PT` | 5 | Wing | `0x1785` | 17 / -16 / -10 | 5.7 / -5.3 / -3.3 | yes | F150.GAS | 4 |
| `A4E.PT` | 6 | Wing | `0x1585` | 26 / -16 / -13 | 8.7 / -5.3 / -4.3 | yes | LAU61.JT | 2 |
| `F31.PT` | 5 | Centerline | `0x0681` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | no | none | 1 |
| `F31.PT` | 6 | Wing | `0x05e5` | 11 / -20 / -6 | 3.7 / -6.7 / -2.0 | yes | AIM120.JT | 2 |
| `F31.PT` | 7 | Wing | `0x05e5` | 22 / -20 / -12 | 7.3 / -6.7 / -4.0 | yes | AGM65G.JT | 2 |
| `F31.PT` | 8 | Wingtip | `0x0085` | 35 / -7 / -17 | 11.7 / -2.3 / -5.7 | yes | AIM9X.JT | 2 |
| `MIG29.PT` | 5 | Wing | `0x07e5` | 20 / -10 / -5 | 6.7 / -3.3 / -1.7 | yes | AA8.JT | 2 |
| `MIG29.PT` | 6 | Wing | `0x07e5` | 25 / -10 / -5 | 8.3 / -3.3 / -1.7 | yes | AA8.JT | 2 |
| `MIG29.PT` | 7 | Wing | `0x07e5` | 50 / -10 / -5 | 16.7 / -3.3 / -1.7 | yes | AA8.JT | 2 |
| `SU27.PT` | 5 | Centerline | `0x07e5` | 0 / -25 / 12 | 0.0 / -8.3 / 4.0 | yes | AA12.JT | 2 |
| `SU27.PT` | 6 | Fuselage | `0x07e5` | 24 / -25 / 8 | 8.0 / -8.3 / 2.7 | yes | AA12.JT | 2 |
| `SU27.PT` | 7 | Wing | `0x07e5` | 51 / -18 / -4 | 17.0 / -6.0 / -1.3 | yes | AA11.JT | 4 |
| `SU27.PT` | 8 | Wingtip | `0x07e5` | 84 / -8 / -16 | 28.0 / -2.7 / -5.3 | yes | AA11.JT | 2 |
| `MIG21.PT` | 4 | Centerline | `0x07e1` | 0 / -20 / -5 | 0.0 / -6.7 / -1.7 | no | none | 1 |
| `MIG21.PT` | 5 | Wing | `0x07e5` | 20 / -13 / -5 | 6.7 / -4.3 / -1.7 | yes | AA2.JT | 2 |
| `MIG21.PT` | 6 | Wing | `0x07e5` | 30 / -13 / -12 | 10.0 / -4.3 / -4.0 | yes | AA2.JT | 2 |
| `SU25.PT` | 5 | Centerline | `0x07e5` | 0 / -21 / 6 | 0.0 / -7.0 / 2.0 | yes | none | 1 |
| `SU25.PT` | 6 | Fuselage | `0x07e5` | 15 / -21 / 4 | 5.0 / -7.0 / 1.3 | yes | B13.JT | 2 |
| `SU25.PT` | 7 | Wing | `0x07e5` | 31 / -13 / 2 | 10.3 / -4.3 / 0.7 | yes | AS7.JT | 4 |
| `SU25.PT` | 8 | Wing | `0x07e5` | 48 / -13 / -1 | 16.0 / -4.3 / -0.3 | yes | AA8.JT | 4 |
| `MIG23.PT` | 4 | Centerline | `0x0601` | 0 / -20 / 17 | 0.0 / -6.7 / 5.7 | no | none | 1 |
| `MIG23.PT` | 5 | Fuselage | `0x0085` | 6 / -20 / 15 | 2.0 / -6.7 / 5.0 | yes | none | 4 |
| `MIG23.PT` | 6 | Fuselage | `0x07e5` | 11 / -20 / 11 | 3.7 / -6.7 / 3.7 | yes | B8.JT | 2 |
| `MIG23.PT` | 7 | Fuselage | `0x07e5` | 15 / -20 / 8 | 5.0 / -6.7 / 2.7 | yes | AS7.JT | 2 |
| `MIG23.PT` | 8 | Wing | `0x07e5` | 18 / -20 / 3 | 6.0 / -6.7 / 1.0 | yes | AS7.JT | 2 |
| `SU35.PT` | 5 | Centerline | `0x07e5` | 0 / -25 / 12 | 0.0 / -8.3 / 4.0 | yes | AAML.JT | 1 |
| `SU35.PT` | 6 | Fuselage | `0x07e5` | 24 / -25 / 8 | 8.0 / -8.3 / 2.7 | yes | AAML.JT | 2 |
| `SU35.PT` | 7 | Wing | `0x07e5` | 51 / -18 / -4 | 17.0 / -6.0 / -1.3 | yes | AA12.JT | 4 |
| `SU35.PT` | 8 | Wingtip | `0x0fed` | 84 / -8 / -16 | 28.0 / -2.7 / -5.3 | yes | AA11B.JT | 2 |
| `F22.PT` | 5 | Internal bay | `0x05e0` | -10 / -25 / 0 | -3.3 / -8.3 / 0.0 | no | AIM120.JT | 2 |
| `F22.PT` | 6 | Internal bay | `0x05e0` | 10 / -25 / 0 | 3.3 / -8.3 / 0.0 | no | AGM65G.JT | 2 |
| `F22.PT` | 7 | Internal bay | `0x05e4` | 27 / -11 / 0 | 9.0 / -3.7 / 0.0 | yes | AIM9X.JT | 4 |
| `F22N.PT` | 5 | Fuselage | `0x05e1` | -10 / -25 / 0 | -3.3 / -8.3 / 0.0 | no | AIM120.JT | 2 |
| `F22N.PT` | 6 | Fuselage | `0x05e1` | 10 / -25 / 0 | 3.3 / -8.3 / 0.0 | no | AGM65G.JT | 2 |
| `F22N.PT` | 7 | Fuselage | `0x05e5` | 27 / -11 / 0 | 9.0 / -3.7 / 0.0 | yes | AIM9X.JT | 4 |
| `AV8.PT` | 5 | Wing | `0x07e5` | 20 / -13 / -20 | 6.7 / -4.3 / -6.7 | yes | AGM65G.JT | 4 |
| `AV8.PT` | 6 | Fuselage | `0x0585` | 9 / -22 / -16 | 3.0 / -7.3 / -5.3 | yes | AGM65G.JT | 2 |
| `AV8.PT` | 7 | Wing | `0x0085` | 32 / -13 / -25 | 10.7 / -4.3 / -8.3 | yes | AIM9M.JT | 2 |
| `YAK141.PT` | 4 | Wing | `0x07e5` | 25 / -10 / 0 | 8.3 / -3.3 / 0.0 | yes | AA10.JT | 2 |
| `YAK141.PT` | 5 | Wing | `0x07e5` | 50 / -10 / 0 | 16.7 / -3.3 / 0.0 | yes | AA11.JT | 2 |
| `AH64.PT` | 4 | Centerline | `0x0605` | 0 / -10 / 0 | 0.0 / -3.3 / 0.0 | yes | none | 1 |
| `AH64.PT` | 5 | Fuselage | `0x0185` | 20 / -20 / 0 | 6.7 / -6.7 / 0.0 | yes | LAU61.JT | 2 |
| `AH64.PT` | 6 | Wing | `0x0085` | 30 / -20 / 0 | 10.0 / -6.7 / 0.0 | yes | AIM9M.JT | 2 |
| `MI24.PT` | 2 | Wing | `0x0188` | 25 / -10 / 0 | 8.3 / -3.3 / 0.0 | no | AT2.JT | 4 |
| `MIG17F.PT` | 3 | Wing | `0x0705` | 22 / -8 / -6 | 7.3 / -2.7 / -2.0 | yes | F150.GAS | 2 |
| `F4B.PT` | 3 | Centerline | `0x0785` | 0 / -19 / 0 | 0.0 / -6.3 / 0.0 | yes | AIM9B.JT | 4 |
| `F4B.PT` | 4 | Fuselage | `0x07e5` | 13 / -19 / 0 | 4.3 / -6.3 / 0.0 | yes | MK82.JT | 4 |
| `F4B.PT` | 5 | Wing | `0x0785` | 26 / -19 / -9 | 8.7 / -6.3 / -3.0 | yes | MK82.JT | 4 |
| `F4B.PT` | 6 | Wing | `0x0605` | 38 / -19 / -20 | 12.7 / -6.3 / -6.7 | yes | F350.GAS | 2 |
| `F4J.PT` | 4 | Centerline | `0x0785` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | yes | none | 1 |
| `F4J.PT` | 5 | Fuselage | `0x07e5` | 20 / -20 / -9 | 6.7 / -6.7 / -3.0 | yes | AIM7E.JT | 4 |
| `F4J.PT` | 6 | Wing | `0x0785` | 31 / -20 / -17 | 10.3 / -6.7 / -5.7 | yes | AIM9B.JT | 4 |
| `F4J.PT` | 7 | Wing | `0x0605` | 44 / -20 / -25 | 14.7 / -6.7 / -8.3 | yes | F350.GAS | 2 |
| `F4E.PT` | 4 | Centerline | `0x0605` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | yes | F150.GAS | 1 |
| `F4E.PT` | 5 | Fuselage | `0x00c5` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | yes | AIM7.JT | 4 |
| `F4E.PT` | 6 | Wing | `0x0765` | 20 / -20 / -9 | 6.7 / -6.7 / -3.0 | yes | AGM65G.JT | 4 |
| `F4E.PT` | 7 | Wing | `0x07e5` | 39 / -20 / -15 | 13.0 / -6.7 / -5.0 | yes | none | 2 |
| `F4.PT` | 4 | Centerline | `0x0605` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | yes | F150.GAS | 1 |
| `F4.PT` | 5 | Fuselage | `0x00c5` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | yes | AIM7.JT | 4 |
| `F4.PT` | 6 | Wing | `0x0765` | 20 / -20 / -9 | 6.7 / -6.7 / -3.0 | yes | AGM88.JT | 4 |
| `F4.PT` | 7 | Wing | `0x0765` | 39 / -20 / -15 | 13.0 / -6.7 / -5.0 | yes | none | 2 |
| `A7.PT` | 4 | Fuselage | `0x0085` | 7 / -28 / 0 | 2.3 / -9.3 / 0.0 | yes | AIM9M.JT | 2 |
| `A7.PT` | 5 | Wing | `0x0705` | 19 / -13 / -5 | 6.3 / -4.3 / -1.7 | yes | MK82.JT | 2 |
| `A7.PT` | 6 | Wing | `0x0785` | 35 / -12 / -11 | 11.7 / -4.0 / -3.7 | yes | AGM65G.JT | 4 |
| `F15.PT` | 4 | Fuselage | `0x07e5` | 25 / -10 / 0 | 8.3 / -3.3 / 0.0 | yes | AIM120.JT | 4 |
| `F15.PT` | 5 | Wing | `0x07e5` | 50 / -20 / 0 | 16.7 / -6.7 / 0.0 | yes | AIM9M.JT | 4 |
| `F16C.PT` | 5 | Centerline | `0x0601` | 0 / -20 / 0 | 0.0 / -6.7 / 0.0 | no | F150.GAS | 1 |
| `F16C.PT` | 6 | Wing | `0x07e5` | 32 / -10 / -7 | 10.7 / -3.3 / -2.3 | yes | AGM65G.JT | 4 |
| `F16C.PT` | 7 | Wing | `0x0185` | 43 / -10 / -11 | 14.3 / -3.3 / -3.7 | yes | AIM120.JT | 2 |
| `F16C.PT` | 8 | Wingtip | `0x0085` | 54 / 2 / -14 | 18.0 / 0.7 / -4.7 | yes | AIM9M.JT | 2 |
| `F104.PT` | 4 | Centerline | `0x0605` | 0 / -15 / -7 | 0.0 / -5.0 / -2.3 | yes | F250.GAS | 1 |
| `F104.PT` | 5 | Wingtip | `0x0605` | 37 / -5 / -10 | 12.3 / -1.7 / -3.3 | yes | F250.GAS | 2 |
| `F104.PT` | 6 | Fuselage | `0x07e5` | 10 / -15 / -7 | 3.3 / -5.0 / -2.3 | yes | AIM7.JT | 2 |
| `F104.PT` | 7 | Wing | `0x07e5` | 18 / -15 / -7 | 6.0 / -5.0 / -2.3 | yes | AIM7.JT | 2 |
| `F104.PT` | 8 | Wing | `0x0285` | 25 / -15 / -9 | 8.3 / -5.0 / -3.0 | yes | AIM9M.JT | 2 |
| `A10.PT` | 5 | Wing | `0x0585` | 10 / -10 / 0 | 3.3 / -3.3 / 0.0 | yes | AGM65G.JT | 4 |
| `A10.PT` | 6 | Wing | `0x0585` | 25 / -10 / 0 | 8.3 / -3.3 / 0.0 | yes | AGM65G.JT | 4 |
| `A10.PT` | 7 | Wing | `0x0585` | 40 / -10 / 0 | 13.3 / -3.3 / 0.0 | yes | AGM65G.JT | 2 |
| `A10.PT` | 8 | Wing | `0x0185` | 55 / -10 / 0 | 18.3 / -3.3 / 0.0 | yes | AGM65G.JT | 2 |

The F-15C and F-16C rows above are source coordinates. Their current
fitted placement is documented in [F-15C stores](../spec/stores-on-pylons.md#the-f-15c)
and [F-16C stores](../spec/stores-on-pylons.md#the-f-16c-and-fleet-rail-audit).
The Mi-24 row 2 also reads its attachment from below the stub wing
([placement](../spec/stores-on-pylons.md#mi-24-lower-wing-attachment)).
No source rows or station capacities were added.

## Original icons and geometry candidates

The four original tank thumbnails `$F150.PIC`, `$F250.PIC`, `$F350.PIC` and
`$F500.PIC` are present in FA_1.LIB and decode to 105 by 19 pixels with identical
artwork. GAS records have no thumbnail pointer. The editor uses the generated
`$<stem>.PIC` name, and selective dependency reports now record that generated
edge. Runtime menu imports already carried the icons independently.

`F250.SH` and `F500.SH` exist in FA_2.LIB and each decodes to 68 untextured
faces without state words. Matching `F150.SH` and `F350.SH` were not found.
Those filename matches do not prove an installed or jettisoned tank association.
No mesh is attached by this pass. Source Shift+J text describes external fuel
jettison, but does not establish shell removal, dumping versus dropping, or the
shape to use. Review the attachment/jettison consumer before using these shapes.

## Internal ammunition weight follow-up

John's supplied retail page reference shows the F-14 at 65,186 lb with 675 M61
units, four Phoenix, two F250 tanks, two AMRAAM and two Sidewinder, with 15,741 lb
internal fuel. The host shows 64,511 lb for that selection. The screenshot's
executable identity is unknown, but reviewed source arithmetic explains the
675 lb difference: ArmPlane retains fixed projectile rows, the card helper at
0x41c68c calls HARDStoreWeight and returns its weight at 0x41c6e9, and the weight
loop at 0x41a957..0x41a964 adds finite loaded quantity times that weight without
an internal-gun exclusion. Quantity 0x7fff takes a separate sentinel branch.
M61.JT has OBJECT weight 1 lb and F14.PT supplies 675 units. M61.JT SHA-256:
`0b79689a728911d1097569f6e6612050e414ba9703c9f131a51fe1ce9a7eb33d`.

The current host excludes internal-gun quantities from loadout weapon mass.
This is a known screen-weight discrepancy, not unknown arithmetic. The current
layout and availability pass preserves that existing mass rule. Next review
flight initialization and ammunition mass debit together before changing the
shared loadout/flight weight behavior; a display-only correction would make the
page disagree with launch validation and flight mass.
