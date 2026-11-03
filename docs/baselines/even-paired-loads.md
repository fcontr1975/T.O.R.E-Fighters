# Even paired loads validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation validation, 2026-10-25, issue #6 and John's review follow-up.
The [even-paired-load contract](../spec/stores-on-pylons.md#even-paired-loads)
covers every resolved two-sided carrier, including fuselage, tunnel and
internal-bay pairs. [Source-row research](../formats/aircraft-ordnance.md#paired-point-counts-and-physics)
owns the retail evidence and build identity.

## Roster result

All 37 selectable identities were audited. Across 36 retail identities,
**828** aircraft/row/store maxima change on **26** aircraft: **695** from
1 to 2 and **133** from 3 to 2. Of these, **760** are live-supported weapons
or tanks and **68** are setup-only definitions. The F/A-XX runtime identity
adds 23 changed combinations on its paired side bay, inherited from its
F22N donor. Including it gives **851** changes, **782** live and **69**
setup-only: **714** increases and **137** decreases.

Normal compatibility remains 3,411 supported weapon placements, 180 tank
placements, 65 designator-pod placements and 170 Cheat-only novelty
placements across the retail roster. No human or AI standard weapon/tank
quantity changes, and no Cheat maximum changes. Custom AI wing loads use
the player's migrated quantities. No AI tactics change.

Compared with the earlier wing-only rule, 21 additional retail rows have
changed normally offered maxima, plus the F/A-XX side-bay row. Every
`Rack::two_sided` row is covered; raw compatibility and retained hardware
are still subject to the existing loadout rules.

| Example | Old maximum | New maximum | Evidence |
| --- | ---: | ---: | --- |
| F15.PT row 4, MK84.JT | 3 | 2 | 6,000 lb class, 2,000 lb per bomb |
| F18.PT row 5, AGM84A.JT | 1 | 2 | 2,000 lb class, 1,165 lb per missile; pair is 330 lb above the old row class |
| F18.PT rows 6/7, AGM84A.JT | 3 | 2 | Source maxItems 4, 4,000 lb class |
| F18.PT rows 6/7, AS16.JT | 1 | 2 | 2,640 lb each; pair is 1,280 lb above the old row class |
| F22.PT row 7, MICA.JT | 3 | 2 | Paired internal side bays are included |
| F16C.PT row 6, AGM84E.JT | 1 | 2 | SLAM weighs 1,380 lb against a 2,700 lb class |
| F16C.PT row 6, AGM84A.JT | 2 | 2 | Harpoon already fit twice; an earlier single-Harpoon capture used a partial load, not its maximum |

Aircraft without changed maxima: C130.PT, AC130.PT, E3.PT, IL76.PT,
E2.PT, V22.PT, MI24.PT, CH47.PT, B747.PT and A310.PT.

## Complete changed maximum inventory

Rows are zero-based source ordinals, independent of display order. Each
store in a cell changes by the stated amount on that row. `faxx` denotes
the selectable concept, not an additional retail PT identity. Setup-only
records remain unavailable for flight. Other normal capacities, true single
points and the original records remain unchanged. The local report includes
one tab-separated row per combination.

| Aircraft | Row | Old to new | Availability | Stores |
| --- | ---: | --- | --- | --- |
| A10.PT | 5 | 1 to 2 | Live | FAB1000.JT, MK82AIR.JT, MK82P.JT |
| A10.PT | 5 | 3 to 2 | Live | AS30.JT, B13.JT, FAB500.JT |
| A10.PT | 6 | 1 to 2 | Live | FAB1000.JT, MK82AIR.JT, MK82P.JT |
| A10.PT | 6 | 3 to 2 | Live | AS30.JT, B13.JT, FAB500.JT |
| A10.PT | 7 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, B13.JT, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| A10.PT | 8 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| A4E.PT | 5 | 1 to 2 | Live | F250.GAS, F350.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| A4E.PT | 5 | 3 to 2 | Live | GBU16.JT, GBU30.JT |
| A4E.PT | 6 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, B8.JT, CBU89.JT, FAB250.JT, GBU16.JT, GBU30.JT, RBK500.JT |
| A7.PT | 4 | 1 to 2 | Live | AGM65A.JT, AGM65G.JT, GBU16.JT |
| A7.PT | 5 | 1 to 2 | Live | F350.GAS, F500.GAS, MK82AIR.JT, MK82P.JT |
| A7.PT | 6 | 3 to 2 | Live | F500.GAS, GBU28.JT |
| AH64.PT | 5 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| AH64.PT | 6 | 1 to 2 | Live | AA11.JT, AA11B.JT |
| AV8.PT | 5 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| AV8.PT | 5 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| AV8.PT | 5 | 3 to 2 | Live | AGM84A.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT |
| AV8.PT | 6 | 1 to 2 | Live | FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| AV8.PT | 7 | 1 to 2 | Live | GBU10.JT, GBU10A.JT, PAVEWA3.JT, PAVEWAY.JT |
| F104.PT | 5 | 1 to 2 | Live | F150.GAS |
| F104.PT | 6 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F104.PT | 6 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F104.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT, PL10.JT |
| F104.PT | 7 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AS7.JT, B8.JT, CBU89.JT, F150.GAS, FAB500.JT, GBU16.JT, GBU30.JT, R530.JT, RBK500.JT |
| F104.PT | 8 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, F150.GAS, F250.GAS, GBU10.JT, GBU10A.JT, PAVEWA3.JT, PAVEWAY.JT |
| F14.PT | 4 | 1 to 2 | Live | FAB1000.JT, MK82AIR.JT, MK82P.JT |
| F14.PT | 4 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| F14.PT | 4 | 3 to 2 | Live | FAB500.JT |
| F14.PT | 5 | 1 to 2 | Live | F350.GAS, F500.GAS |
| F14.PT | 6 | 1 to 2 | Live | AAML.JT, AS16.JT, F250.GAS, F350.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F14.PT | 7 | 1 to 2 | Live | GBU12.JT |
| F15.PT | 4 | 1 to 2 | Live | F500.GAS, GBU28.JT, MK82AIR.JT |
| F15.PT | 4 | 3 to 2 | Live | F250.GAS, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F15.PT | 5 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F15.PT | 5 | 1 to 2 | Live | AGM45.JT, AGM84A.JT, AGM84E.JT, AGM88.JT, AS30.JT, B13.JT, B8.JT, F150.GAS, FAB500.JT, GBU16.JT, GBU30.JT |
| F15.PT | 5 | 3 to 2 | Live | AA12.JT, AIM7E.JT, MK20.JT |
| F16C.PT | 6 | 1 to 2 | Live | AGM84E.JT, AM39.JT, AS14.JT, AS16.JT, F250.GAS, F350.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F16C.PT | 6 | 3 to 2 | Live | AGM45.JT, AGM88.JT, B8.JT, CBU89.JT, RBK500.JT |
| F16C.PT | 7 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AS30.JT, CBU89.JT, FAB500.JT, GBU16.JT, GBU30.JT, RBK500.JT |
| F16C.PT | 8 | 1 to 2 | Live | AA11.JT, AA11B.JT |
| F16C.PT | 9 | 1 to 2 | Live | F500.GAS, GBU28.JT, MK82AIR.JT |
| F18.PT | 5 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F18.PT | 5 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F18.PT | 6 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| F18.PT | 6 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| F18.PT | 6 | 3 to 2 | Live | AGM84A.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT |
| F18.PT | 7 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| F18.PT | 7 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| F18.PT | 7 | 3 to 2 | Live | AGM84A.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT |
| F18.PT | 8 | 1 to 2 | Live | AA11.JT, AA11B.JT |
| F22.PT | 7 | 1 to 2 | Setup only | PL10.JT |
| F22.PT | 7 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AIM7E.JT, AS7.JT, B8.JT, CBU87.JT, CBU89.JT, FAB250.JT, GBU12.JT, GBU38.JT, MK82.JT, MK82HD.JT, R530.JT, RBK250.JT, RBK500.JT |
| F22.PT | 7 | 3 to 2 | Live | AA11.JT, AA11B.JT, AS15.JT, MICA.JT |
| F22N.PT | 7 | 1 to 2 | Setup only | PL10.JT |
| F22N.PT | 7 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AIM7E.JT, AS7.JT, B8.JT, CBU87.JT, CBU89.JT, FAB250.JT, GBU12.JT, GBU38.JT, MK82.JT, MK82HD.JT, R530.JT, RBK250.JT, RBK500.JT |
| F22N.PT | 7 | 3 to 2 | Live | AA11.JT, AA11B.JT, AS15.JT, MICA.JT |
| F31.PT | 6 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F31.PT | 6 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F31.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F31.PT | 7 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F31.PT | 8 | 1 to 2 | Live | AA11.JT, AA11B.JT |
| F4.PT | 5 | 1 to 2 | Setup only | AA9.JT |
| F4.PT | 5 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, GBU10.JT, GBU10A.JT, PAVEWA3.JT, PAVEWAY.JT |
| F4.PT | 5 | 3 to 2 | Setup only | PL10.JT |
| F4.PT | 5 | 3 to 2 | Live | AGM65A.JT, AGM65G.JT, AS7.JT, R530.JT |
| F4.PT | 6 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| F4.PT | 6 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| F4.PT | 6 | 3 to 2 | Live | AGM84A.JT, B13.JT, F150.GAS, FAB500.JT |
| F4.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F4.PT | 7 | 1 to 2 | Live | AGM84A.JT, AM39.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT |
| F4B.PT | 4 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F4B.PT | 4 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F4B.PT | 4 | 3 to 2 | Setup only | PL10.JT |
| F4B.PT | 4 | 3 to 2 | Live | AIM7.JT, AS7.JT, FAB250.JT, R530.JT |
| F4B.PT | 5 | 1 to 2 | Live | GBU28.JT |
| F4B.PT | 5 | 3 to 2 | Live | F350.GAS, FAB1000.JT |
| F4B.PT | 6 | 1 to 2 | Live | F500.GAS |
| F4E.PT | 5 | 1 to 2 | Setup only | AA9.JT |
| F4E.PT | 5 | 1 to 2 | Live | AGM84E.JT, AS14.JT, AS30.JT, GBU10.JT, GBU10A.JT, PAVEWA3.JT, PAVEWAY.JT |
| F4E.PT | 5 | 3 to 2 | Setup only | PL10.JT |
| F4E.PT | 5 | 3 to 2 | Live | AGM65A.JT, AGM65G.JT, AS7.JT, R530.JT |
| F4E.PT | 6 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| F4E.PT | 6 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| F4E.PT | 6 | 3 to 2 | Live | AGM84A.JT, B13.JT, F150.GAS, FAB500.JT |
| F4E.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F4E.PT | 7 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F4J.PT | 5 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| F4J.PT | 5 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| F4J.PT | 5 | 3 to 2 | Setup only | PL10.JT |
| F4J.PT | 5 | 3 to 2 | Live | AIM7.JT, AS7.JT, FAB250.JT, R530.JT |
| F4J.PT | 6 | 1 to 2 | Live | GBU28.JT |
| F4J.PT | 6 | 3 to 2 | Live | F350.GAS, FAB1000.JT |
| F4J.PT | 7 | 1 to 2 | Live | F500.GAS |
| MIG17F.PT | 3 | 1 to 2 | Live | F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| MIG21.PT | 5 | 1 to 2 | Setup only | PL10.JT |
| MIG21.PT | 5 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AS7.JT, B8.JT, CBU89.JT, FAB250.JT, GBU16.JT, GBU30.JT, R530.JT, RBK500.JT |
| MIG21.PT | 6 | 1 to 2 | Setup only | PL10.JT |
| MIG21.PT | 6 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AS7.JT, B8.JT, CBU89.JT, FAB250.JT, GBU16.JT, GBU30.JT, R530.JT, RBK500.JT |
| MIG23.PT | 5 | 1 to 2 | Live | GBU12.JT |
| MIG23.PT | 5 | 3 to 2 | Setup only | PL7.JT |
| MIG23.PT | 5 | 3 to 2 | Live | AA2.JT, AIM9B.JT, AIM9M.JT, AIM9X.JT, R550.JT |
| MIG23.PT | 6 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| MIG23.PT | 6 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT, GBU16.JT, GBU30.JT, MK82P3.JT |
| MIG23.PT | 7 | 1 to 2 | Live | AS16.JT, F250.GAS, F350.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| MIG23.PT | 8 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| MIG23.PT | 8 | 1 to 2 | Live | AGM45.JT, AGM84A.JT, AGM84E.JT, AGM88.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, B8.JT, F150.GAS, FAB500.JT, GBU16.JT, GBU30.JT, MK82P3.JT |
| MIG29.PT | 5 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| MIG29.PT | 5 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| MIG29.PT | 6 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| MIG29.PT | 6 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| MIG29.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| MIG29.PT | 7 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| RAFALE.PT | 5 | 1 to 2 | Live | AS16.JT, GBU28.JT, MK82AIR.JT, MK82P.JT |
| RAFALE.PT | 5 | 3 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, MK82P3.JT |
| RAFALE.PT | 6 | 1 to 2 | Live | AS16.JT, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| RAFALE.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| RAFALE.PT | 7 | 1 to 2 | Live | AGM45.JT, AGM84A.JT, AGM84E.JT, AGM88.JT, AS30.JT, B13.JT, B8.JT, FAB500.JT, GBU16.JT, GBU30.JT |
| RAFALE.PT | 8 | 1 to 2 | Live | AA11.JT, AA11B.JT |
| SU25.PT | 6 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| SU25.PT | 7 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| SU25.PT | 7 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| SU25.PT | 7 | 3 to 2 | Live | AGM84A.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT |
| SU25.PT | 8 | 1 to 2 | Live | AS16.JT, F350.GAS, F500.GAS, FAB1000.JT, MK82AIR.JT, MK82P.JT |
| SU25.PT | 8 | 3 to 2 | Setup only | AA6.JT, AA9.JT |
| SU25.PT | 8 | 3 to 2 | Live | AGM84A.JT, AS30.JT, B13.JT, F150.GAS, FAB500.JT |
| SU27.PT | 6 | 1 to 2 | Setup only | PL10.JT |
| SU27.PT | 6 | 1 to 2 | Live | AA10.JT, AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AS7.JT, B8.JT, CBU89.JT, FAB250.JT, GBU16.JT, GBU30.JT, R27ER.JT, R530.JT, RBK500.JT |
| SU27.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| SU27.PT | 7 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| SU27.PT | 7 | 3 to 2 | Setup only | PL10.JT |
| SU27.PT | 7 | 3 to 2 | Live | AA10.JT, AIM7.JT, AS7.JT, FAB250.JT, R27T.JT, R530.JT |
| SU27.PT | 8 | 1 to 2 | Setup only | PL10.JT |
| SU27.PT | 8 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AS7.JT, B8.JT, CBU89.JT, FAB250.JT, GBU16.JT, GBU30.JT, R530.JT, RBK500.JT |
| SU35.PT | 6 | 1 to 2 | Live | AS16.JT, F250.GAS, F350.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82AIR.JT, MK82P.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| SU35.PT | 7 | 1 to 2 | Setup only | AA6.JT, AA9.JT |
| SU35.PT | 7 | 1 to 2 | Live | AGM84A.JT, AGM84E.JT, AM39.JT, AS14.JT, AS30.JT, B13.JT, F150.GAS, F250.GAS, FAB500.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| SU35.PT | 7 | 3 to 2 | Setup only | PL10.JT |
| SU35.PT | 7 | 3 to 2 | Live | AA10.JT, AIM7.JT, AS7.JT, FAB250.JT, R27T.JT, R530.JT |
| YAK141.PT | 4 | 1 to 2 | Live | AGM84E.JT, AM39.JT, AS14.JT, F250.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| YAK141.PT | 5 | 1 to 2 | Live | AGM84E.JT, AM39.JT, AS14.JT, F250.GAS, FAB1000.JT, GBU10.JT, GBU10A.JT, GBU29.JT, GBU29P.JT, MK82P3.JT, MK84.JT, PAVEWA3.JT, PAVEWAY.JT |
| faxx | 7 | 1 to 2 | Setup only | PL10.JT |
| faxx | 7 | 1 to 2 | Live | AGM45.JT, AGM65A.JT, AGM65G.JT, AGM88.JT, AIM7.JT, AIM7E.JT, AS7.JT, B8.JT, CBU87.JT, CBU89.JT, FAB250.JT, GBU12.JT, GBU38.JT, MK82.JT, MK82HD.JT, R530.JT, RBK250.JT, RBK500.JT |
| faxx | 7 | 3 to 2 | Live | AA11.JT, AA11B.JT, AS15.JT, MICA.JT |

## Migration and overweight evidence

Saved singles now become pairs, including tanks; larger odd quantities round
down and all quantities clamp to current capacity. An imported Hornet fixture
with four single GBU-28 loads increases from **46,974 to 64,974 lb**, against
**49,224 lb maximum takeoff weight**. This deliberately uses the existing Cheat
compatibility to test a previously under-limit save crossing the aircraft limit.
The editor validation retains the migrated pairs. Actual non-interactive
`World::new` refuses the mission through the existing weight check with
`This plane is too heavy to take off. Remove stores or fuel.` No flight starts,
and no store, fuel or loadout substitution is made.

Synthetic UI tests exercise restored overweight loads, Fly refusal, adjustment
and successful Fly afterward. Synthetic world tests check the same non-interactive
combat construction path. The roster audit checks old saves, even maxima, edits,
tank quantities, strict network requests and non-pairing error wording. Geometry
and mass tests include fuselage, tunnel and internal bay pairs as well as wings.

## Validation

Validated on Linux with Rust 1.91.1, locked dependencies and eight build/test
workers. Commands used `nice -n 10`; the battery used two jobs and one window.

- Workspace tests: 7,159 passed, 106 existing ignored, including 2,088 sim and
  1,023 world tests. App tests passed again after the tank-probe adjustment:
  2,339 passed, 48 ignored.
- Workspace clippy with warnings denied, build, formatting, documentation and
  source/app/extractor asset checks passed.
- Python tools: 505 run, 500 passed, five skipped.
- Forty focused battery scenarios were verified. The full run passed 39/40;
  its one failure was the old F-14 tank probe's assumption that minus left a
  single tank. The probe now tests zero-or-two editing and saved-single
  migration to two with 3,300 lb fuel and 396 lb shells. Both tank scenarios
  passed on rerun. No production behavior changed for that probe correction.
- The battery includes Hornet firing, F-14/Hornet/Su-27 jettison, empty and
  gun-only takeoff/landing, AI/A-G jettison, paired side-bay/wing release,
  loadout sharing and validation, respawn loadout menus, network stores,
  network AI respawn, and all store replay checks. The broad creator sweep
  and full battery were not run.
- GPU before/after comparisons show one Mk-84 per F-15C fuselage side and one
  Harpoon per Hornet fuselage side. The display smoke test passed.
- Existing wire/checkpoint/replay round trips and checksum goldens passed;
  no golden or format version was changed.

Evidence is in the main checkout's local task scratch directory:
`.local/next-round/0.2.2-rc1/stores-balance/`. The inventory is
`followup-changed-maxima.tsv`; the full battery report is
`followup-battery/20261025-143321-all-paired-points/summary.md`, and the tank
rerun is `followup-battery/20261025-144830-tank-probe-updated/summary.md`.
The task report names every command log and the before/after captures.
No artwork is committed.

An initial UI test expected a silent refusal instead of the existing click
action; that assertion was corrected while keeping the overweight-message
and successful-Fly-after-adjustment checks. A later `/tmp` disk quota failure
affected four replay-file tests. The full rerun passed using the task's
scratch directory for temporary files; no replay code changed for that.

Retail execution and visual comparison remain unavailable. Per-side store CG
and drag torque are not implemented in T.O.R.E and are not claimed by this
validation. Wire, checkpoint and recorded store-layout schemas remain unchanged;
old simulation replays beginning from changed preflight loads may diverge
or refuse to start when migration makes the load overweight.
The lead owns the combined protocol/compatibility bump.
