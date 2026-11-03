# Quick Mission distance table

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation validation, 2026-10-28, branch `qmc-distance-table` from
`0.2.2-rc1` at `f207e919` (protocol 77). John asked that the Quick Mission
Creator offer only the enemy distances that fit, limited by theater and
objective, from a table generated once and checked in, with no check while
the player edits. The rules are in the
[creator specification](../spec/quick-mission-menu.md#keeping-the-enemy-on-the-map);
how to regenerate the table is in the
[developer guide](../DEVELOPMENT.md#quick-mission-distance-table).

## How it was measured

`tore-app --qmc-distance-table --jobs 6 --write ...` against an isolated
FA 1.02F import, with the launch solver (`mission_layout::availability`), in
340 seconds of wall time (about 34 CPU minutes over six workers). A distance
is offered only when it fits every case of its row:

- **No target, airborne:** all 216 enemy formations (each enemy wing 0 to 5
  aircraft) in each of the three situations, from the theater's airborne
  start. Area defenses place no start, so they share this row.
- **No target, ground start:** every airfield the creator offers a Blue pilot,
  with every wing size from 1 to 5 the runway holds, and every enemy formation.
  Vietnam offers no ground start; its ground row repeats the airborne one.
- **A ground target, airborne:** 64 surface seeds, the AAA and SAM strengths
  cycling through all sixteen pairs (four seeds each), at the theater's own
  enemy nationality and the default weather. Blue starts the distance from
  Red, so the row is where Blue's start fits the map at full distance.
- **A ground target, ground start:** every distance, since the target places
  the enemy (lead decision, 2026-10-26). Not measured.

Two full runs gave the same rows. No row has a hole (a distance left out below
an offered one). From 1 nm to 150 nm every row offers every distance: no
theater or target leaves out 1 to 5 nm. The three situations never changed a
row: the group's shape changes by a few thousand feet, while the steps between
the long distances are 50 to 100 nm.

## The table

Every row offers each listed distance from 1 nm up to the one named
(1, 2, 5, 10, 20, 50, 75, 100, 150, 200, 300). A target not listed in the last
column has its theater's airborne row. With a target and a ground start every
row offers up to 300.

| Theater | No target, airborne | No target, ground start | Targets that differ (airborne) |
| --- | --- | --- | --- |
| BAL (Baltic) | up to 200 | up to 200 | QBFLT up to 300, QBAIR up to 300, QBBRD up to 300, QBACOL up to 300, QBFAIR up to 300 |
| CUB (Cuba) | up to 200 | up to 300 | QCFAIR up to 300, QCCARG up to 300 |
| EGY (Egypt) | up to 150 | up to 200 | QESFLT up to 200, QESAIR up to 200, QELAIR up to 200, QECMHQ up to 300, QERDRI up to 200, QEARMOR up to 200, QECDEF up to 200 |
| LFA (Falklands) | up to 200 | up to 200 | none |
| FRA (France) | up to 150 | up to 200 | QFFLT up to 200, QFSAIR up to 200, QFLAIR up to 200, QFCMHQ up to 200, QFFACT up to 200 |
| GRE (Greece) | up to 200 | up to 200 | QGRCARG up to 300 |
| IRA (Iran) | up to 200 | up to 200 | QIRFAIR up to 300, QIRPOW up to 300, QIRCWP up to 300 |
| KURILE (Kuril Islands) | up to 200 | up to 300 | QKSFLT up to 300, QKLFLT up to 300, QKSCFT up to 300, QKSUB up to 300, QKPLNGR up to 300, QKSILO up to 300 |
| TVIET (Vietnam) | up to 150 | up to 150 | QTBARG up to 200, QTBRDG up to 200, QTBUNK up to 200, QTCOMM up to 200, QTSTRG up to 200, QTTRUCK up to 200, QTAAA up to 200, QTSAM up to 200 |
| SPA (Spain) | up to 200 | up to 200 | QSPFAIR up to 300 |
| APA (Pakistan) | up to 200 | up to 200 | QAPHELO up to 300, QAPCMHQ up to 300 |
| PGU (Persian Gulf) | up to 200 | up to 200 | QPGFAIR up to 300 |
| NSK (North Korea) | up to 200 | up to 200 | QNSFAIR up to 300 |
| WTA (Taiwan) | up to 200 | up to 200 | QWTFAIR up to 300, QWTPATR up to 300 |
| UKR (Ukraine) | up to 200 | up to 200 | none |
| VLA (Vladivostok) | up to 150 | up to 200 | QVSFLT up to 200, QVSAIR up to 200, QVLAIR up to 200, QVCMHQ up to 200, QVARMOR up to 200, QVRDRI up to 200, QVSUP up to 200 |

The masks are in `crates/tore-world/src/mission_layout/distance_table_data.rs`.

### Seed-dependent rows

The largest distance that fits a target's airborne start moves with its layout
seed in 39 of the 108 target rows (Red's start, and so Blue's, moves with the
target's relocation). Those rows offer the smaller distance: for example the
Baltic's QBXING reached 300 nm in some seeds and 200 nm in others, and offers
up to 200. France's QFSUP and QFRDRI and Vietnam's QTCARGO offer up to 150 for
the same reason. A seed outside the 64 measured could still leave a row's top
distance no room; launch then refuses that layout and tries the next seed,
at most sixteen, in single player and on a host alike.

### The target's fixed note

One limitation held in every seed: the Baltic carrier group (QBFLT) leaves out
"4 Yak-141 Freestyle-A (no supported carrier deck)". The creator shows it as
the target's note while QBFLT is chosen. The other limitations depended on the
seed and are no longer shown before launch: unresolved vehicles in 2 of 64
Kuril QKPLNGR layouts, 1 of 64 Kuril QKSILO and 1 of 64 Vladivostok QVARMOR;
area defenses that the Kuril map could not hold in full (28 and 30 of 40 units)
in two of four seeds.

## Cost of a creator change

Measured with `--qmc-distance-table --measure` on the development machine
(Ryzen 9 7900X), on one thread, `TORE_WORKERS=0`, priority 19. "Before" is the
placement audit the creator ran on its own `tore-placement-check` thread for
each change of theater, target, start, weather, defenses or wings (four
consecutive runs; the first is cold). "After" is the table lookup and the
distance's fit, averaged over a million calls.

| Setup | Before: wall, first run | Before: wall, warm | Before: CPU, warm | After: wall and CPU |
| --- | --- | --- | --- | --- |
| Kurile, no target | 109 ms | 35 ms | 35 ms | 6 ns |
| Ukraine, no target | 611 ms | 195 ms | 194 ms | 28 ns |
| Ukraine, QUCITY | 263 ms | 264 ms | 263 ms | 53 ns |
| Baltic, QBFLT | 557 ms | 274 ms | 273 ms | 4 ns |
| Vladivostok, area defenses | 303 ms | 162 ms | 162 ms | 30 ns |

The lookup is a scan of 16 theater rows and 51 target rows, a few tens of
nanoseconds; nothing loads terrain and no thread starts. The creator also lost
its "Checking distances and target placement..." wait: before, a change
showed that notice and held launch until the audit finished.
