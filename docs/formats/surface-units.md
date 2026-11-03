# NT surface units: SAMs, guns, ships, vehicles and men

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Research notes for the surface-AI round, implementation mode, 2026-10-10. The
retail archive holds 84 `*.NT` active-object records. This page is the byte
and field contract for the bounded reader `tore_formats::surface_unit`
(`crates/tore-formats/src/surface_unit.rs`). What a player sees and the numbers
the AI uses are in [the surface defenses spec](../spec/surface-defenses.md).
Template grammar is in [quick-templates.md](quick-templates.md). Inputs are the
same build as the other format pages (`FA_2.LIB` SHA-256
`fb8b30216e739292489d4872cc440debec334e14f8b9a3d0e340092445246198`).

## Layout

An NT is the OBJECT and NPC blocks of a PT with no PLANE block, then its
hardpoint rows. The field order is `aircraft_schema.rs` (`OBJECT`, `NPC`,
`HARDPOINT`), shared with the aircraft reader; only the statement count differs.

```mermaid
flowchart LR
    NT[NT file] --> OBJ[OBJECT: identity, class, shape, hit points, signatures, movement words, utilProc]
    NT --> NPC[NPC: flags, ctName, search and attack delays, retargetT, zoneDist, numHards]
    NPC --> HARDS[hardpoint rows, 12 statements each]
    HARDS --> JT[JT weapon record]
    HARDS --> SEE[SEE sensor record]
    OBJ --> SH[main shape, shadow shape]
```

Rules the reader enforces (all 84 retail records satisfy them):

| Rule | Value |
| --- | --- |
| `structType` | 3 (a PT is 5) |
| `typeSize` | 186 plus 24 per hardpoint |
| Root statements | OBJECT then NPC exactly, no PLANE |
| Hardpoints | 0 to 64 rows, equal to `numHards`; zero rows still carry an empty `hards` block |
| Hardpoint store | a `.JT` weapon, a `.SEE` sensor, or none |
| Identity strings | three: short name, long name, `<NAME>.NT` |
| Size | the BRF reader's 1 MiB bound |

`utilProc` stays a symbol name. Nothing is executed.

## Census

| Class word | Meaning | Count | Notes |
| --- | --- | ---: | --- |
| `0x2000` | ship | 32 | 27 `_GVProc`, 5 `_CARRIERProc` |
| `0x1000` | SAM | 17 | SCUD is class SAM and carries four SA-9s |
| `0x0800` | AAA | 8 | includes the invisible A_M1939 barrage zone |
| `0x0400` | tank | 9 | M1, M1975, M2, M113, BMP2, BTR80, T72, T80, T90 |
| `0x0200` | vehicle | 11 | HUMVEE, TRUCK, TANKER, MISTRK, 3 MULEs, LTRACK, SFLUSH, SRDR1, SRDR2 |
| `0x0100` | structure | 1 | GCI radar, `_OBJProc` |
| `0x0040` | other | 6 | TROOPS, SOLDIER, RUNNER, PLTDWN, CATGUY, EJECT |

Hit points run from 1 (A_M1939) and 5 (men, MANPADS teams) through 100 for most
vehicles and SAMs, 650 for the SA-2 and 4,000 for the Iowa and the large
carriers. `damage[0..4]` is 255 on every NT and `dmgType` is 0 on every NT;
neither has a known consumer. `expType` is 21 for ground units, 35 for ships and
the GCI, 15 for men. `craterSize` is 6 for ground units, 0 for ships, 1 or 0 for
men. The debris positions (`dmgDebrisPos`, `dstDebrisPos`) are all zero on NTs.

Other OBJECT fields as NTs use them: `utilProc` is `_GVProc` (ground vehicles
and ships), `_CARRIERProc` (5 carriers), `_OBJProc` (GCI radar, men,
structures) or a unit's own proc (CATGUY, EJECT). `sigs[0..4]` is
100/100/100/100/0 for ground units, 150 to 300 for ships, 100/100/50/10 for the
Sea Shadow and 100/100/25/25 for small boats; `sigs[3]` is radar and `sigs[2]`
infrared per the [radar spec](../spec/radar.md). `maxVisDist` is 78 (men 59,
SA-2 391, GCI 195): 78, 195 and 391 match 20,000, 50,000 and 100,000 ft at 256
ft per unit (inference). OBJECT `flags` read 0x4000000 on SA2A, 0x2000000 on
M1939, KS12 and KS19, 0x801 on the Mule objects and 0xc8331, 0x108331 and
0x1c8131 on carriers; their meaning is unknown.

There is no radar-to-launcher link anywhere in the data: every launcher carries
its own missile, and the missile's seeker zone is its search volume. The site
pieces (revetments `SA3SITE.OT`, `HAWKSITE.OT`; radar vehicles LTRACK, SFLUSH,
SRDR1, SRDR2; Tall King `KING.OT`; passive radars; microwave relays; MISTRK)
carry no weapons. The SAM batteries TORE builds around a search radar are a
design rule ([SAM batteries](../spec/surface-defenses.md#sam-batteries)). For
comparison, the archive's OTs number 83 structures, 74 other objects (city,
houses, crates, rocks, roads, flags) and 13 airports (`_STRIPProc`).

## Hardpoints: arcs, rest direction, ammunition

| Field | Meaning |
| --- | --- |
| `name` | location byte, 0 on every NT |
| `flags` | 8 or 10; bit `0x2` is not understood |
| `pos.x/y/z` | mount position in hull source units; TORE places mounts at a third of the record's feet, the real-size rule of [placed object scale](objects-and-shapes.md#placed-object-scale-2026-10-10) (fitted) |
| `slewH`, `slewP` | rest direction of the mount, hull relative, heading and pitch |
| `slewLimitH`, `slewLimitP` | half-arc either side of the rest direction |
| `defaultTypeName` | the JT or SEE record |
| `maxItems` | rounds on the mount; 32767 means unlimited |
| `maxWeight` | 0 on every NT |

Angles count 182 units per degree: SA-6 pitch limit 12740 is 70 degrees, a
Nimitz Phalanx arc 21840 is 120 degrees, a full 90 degrees reads 16380 (not
16384). `slewH` is not always 0: the Eisenhower, Clemenceau and Wasp stern mounts read
32760 (180 degrees), the Kitty Hawk's four mounts rest at -60, 135, -90 and 90
degrees, and the Kiev's SA-N-9 rests aft, so a mount's field of fire is its
rest direction plus or minus the limit. Ground launchers have `slewLimitH` 0 and a
pitch limit only (SA-2 2730 = 15 degrees, SA-3 and SA-6 70 degrees, SA-15 0).
Guns carry 32767; missile racks carry the finite load (SA-2 six rails of 1,
SA-6 3, SA-15 8, MANPADS 2). The 22 finite weapon mounts in the archive are
those six SA-2 rails and one rack on each of 16 other launchers.

## Sensor hardpoints

Two NTs name a `.SEE` store instead of a weapon: `GCI.NT` (`GCIR.SEE`) and
`BUTLER.NT` (`REDCR.SEE`, with two `AAA30.JT` guns). They set the NPC flag bit
`0x1`, the only NTs that do. The reader lists sensors among the mounts (the mount
index is the mount's identity for ammunition state) and repeats the first one in
`SurfaceUnit::sensor`.

## Destroyed looks

The records name no damaged shape. By the retail naming rule every ship has
one: the main shape's stem plus `_A.SH` (`nimz.SH` and `NIMZ_A.SH`, `knx.SH` and
`KNX_A.SH`). The import test confirms all 32 exist in `FA_2.LIB`. No ground
unit, SAM or man has a variant shape. The archive holds one wreck object,
`DEST.OT` ("Destroyed Vehicle", shape `dest.SH`); which units leave it is not
traced. TORE swaps a destroyed vehicle, launcher or gun to it (fitted,
[destroyed looks](../spec/surface-defenses.md#destroyed-looks)). `expType` selects
the explosion ([explosions](explosions.md)).

## Supply trucks

Three soft supply vehicles appear in the equipment lists and templates: TRUCK,
TANKER (fuel) and MISTRK ("SAM-Carrying Truck"). All three are unarmed class
`0x0200` units. The reader tells them apart as `TRUCK_TYPES`; the two that
resupply SAM rails and AAA magazines under the surface-AI plan (TRUCK and
MISTRK) are `SUPPLY_TRUCKS`. That split is a design choice, not retail data.

## Shoulder-fired teams

Five SAM records are infantry teams with a shoulder-fired launcher (MANPADS):
SA-7, SA-14, SA-16, FIM-92 and Mistral (`MIS`). Nothing in the record marks
them as such. They are the only SAM-class units with the men's hit points (5,
every vehicle and launcher 100 or more) and the men's top speed (`_maxSpeed`
10, SAM vehicles 50; the fixed Crotale reads 10 but has 100 hit points). The
reader names this rule `SurfaceUnit::is_shoulder_fired_team`: a SAM-class unit,
not a ship, with at most `TEAM_MAX_HIT_POINTS` (5) and at most `TEAM_MAX_SPEED`
(10). The census test checks that the rule picks exactly these five. The AI
uses it to hide the teams from its threat picture
([AI strike](../spec/ai-strike.md#what-the-ai-knows)); the rearm times of the
surface supply rules name the same five.

## NPC block

| Field | Notes |
| --- | --- |
| `searchFrequencyT`, `unreadyAttackT`, `attackT` | quarter seconds ([AI B42](ai.md)); 20/20/20 (SA-15) through 40/144/60 (SA-2, SA-3, SA-6) and 192/176/176 (SCUD) |
| `retargetT` | 32767 except KS-12 and KS-19 (40, 10 s) |
| `zoneDist` | 0 except the A_M1939 barrage zone (195) |
| `ctName` | an AI script name; only SARAN names one (`HYDRO.BI`) |
| `flags` | 1 on the two sensor NTs, otherwise 0 |

## Movement words

`_turnRate`, `_minSpeed`, `_cornerSpeed` and `_maxSpeed` are plain words;
`_acc` and `_dacc` carry the `^` marker. Values: ships turn 910 (5 degrees at
182 per degree), tanks 2730 (15 degrees), speeds 50 for most ships, tanks and
SAM vehicles, 100 for hydrofoils, frigates and LCAC, 10 for men and MANPADS
teams, 0 for fixed guns. Waypoint speeds in the same files are feet per second
(the aircraft cruise values 843, 506, 759, 675 and 421 are exactly 500, 300,
450, 400 and 250 knots), and the plan treats these words as feet per second
too. The unit of the movement words and of `maxVisDist` is not established.

## Weapon zones of the armed units

Every surface weapon record (JT) carries two zones in its seeker block, both in
feet. The record text writes the ranges with the `^` marker (`dword ^50000`),
which stores feet times 256; the reader returns the feet as written, so
nothing is rescaled again. The plain integers `maxVisDist`, `zoneDist` and
`searchDist` are different: they are in 256 ft units (inference).

| Zone | JT offset | Meaning |
| --- | --- | --- |
| zone 0 | `+0xbb` | Where the weapon can see and pick a target: the acquisition (search) range and altitude band |
| zone 1 | `+0xcf` | Where it may launch or fire: the launch range and altitude band |

Read from `FA_2.LIB` (SHA-256 `fb8b3021...`, the build in the spec) on
2026-10-13 with `surface_inspect FA_2.LIB zones`. Ranges in feet; the guns'
zone 1 is the record's, before TORE caps it by the shells' reach.

| Unit | Record | Zone 0 range | Zone 1 range | Ratio |
| --- | --- | ---: | ---: | ---: |
| ZSU23 | ZSU23 | 60,000 | 50 to 7,500 | 8.0 |
| 2S6 | 2S6 | 54,000 | 50 to 15,000 | 3.6 |
| ZSU57, ZIF31 | ZSU57 | 60,000 | 50 to 12,000 | 5.0 |
| KIROV, SOVR, KIEV, SARAN, BUTLER | AAA30 | 60,000 | 50 to 15,000 | 4.0 |
| CLEM, IOWA, KITT, NIMZ, TICON, WASP | PHALANX | 50,000 | 50 to 20,000 | 2.5 |
| **M163** | **PHALANX** | **50,000** | **50 to 20,000** | **2.5** |
| KS12 | KS12 | 100,000 | 0 to 40,000 | 2.5 |
| KS19 | KS19 | 100,000 | 0 to 50,000 | 2.0 |
| M1939, A_M1939 | M1939, A_M1939 | 20,000 | 0 to 10,000 | 2.0 |
| AAA30BAD ships | AAA30BAD | 20,000 | 50 to 10,000 | 2.0 |
| Tanks, IFVs, APCs | M1, T72, BMP2, BTR80, M113, M2 | 20,000 | 50 to 3,000 / 5,000 | 4.0 to 6.7 |
| SA-7, SA-9, SA-13, SA-14, SA-16, FIM-92 | own | 15,000 to 100,000 | 500 to 54,000 | |
| SA-2 / SA-3 / SA-6 | own | 150,000 / 100,000 / 80,000 | 7,500 to 95,000 / 7,500 to 54,000 / 9,000 to 75,000 | |

What this shows:

- The M163's 50,000 ft (8.2 nm) detection is the retail value, read correctly.
  It is not a misread field, a unit error or a TORE default.
- **`M163.NT` mounts `PHALANX.JT`**, the ship Phalanx close-in weapon record
  (its single hardpoint `defaultTypeName`), so the M163 inherits that record's
  zone 0 of 50,000 ft and zone 1 of 50 to 20,000 ft (altitude to 15,000 ft),
  its 10 s round life and its 6-round bursts.
- `FA_2.LIB` also holds a dedicated **`M163.JT`**, which no NT, no other
  archive and no `FA.EXE` string refers to (only `M163.NT` is named in
  `FA.EXE`). It is a copy of the ZSU23 record: every field the reader keeps is
  equal to `ZSU23.JT`'s except the names (zone 0 60,000 ft, zone 1 50 to 7,500
  ft, altitude to 7,500 ft, life 5 s, 4-round bursts, the `hvygun` sound).
  Whether retail ever wired it up is unknown: the data says the M163 fires
  PHALANX.
- Every radar-directed gun in retail acquires at 2 to 8 times the range it
  fires at (a Shilka sees 60,000 ft and fires at 7,500 ft). The M163's 2.5 to
  1 is among the narrowest of the radar guns (the KS-19's is 2.0 to 1). A big acquisition zone is therefore
  the normal shape of the data, not a quirk of the M163.
- TORE replaces the M163's zones (opinionated, John, 2026-10-13): the AAA tuning row sets detection 18,000 ft and fire 7,500 ft up to 7,500 ft above, leaving the record and the ships' Phalanx alone ([AAA tuning](../spec/surface-defenses.md#aaa-tuning)). The table above is the retail data, not what the game uses for the M163.
- What stands out in the retail data is the M163's **fire** range: 20,000 ft (3.3 nm) is 2.7
  times the ZSU-23's 7,500 ft, because it borrows the ship record. The real
  M163 VADS fires effectively to about 1.2 nm (about 7,500 ft), which is
  exactly the unused `M163.JT`'s (a ZSU23 copy) and the ZSU23's zone 1.

## Not established

- The units of `maxVisDist`, `zoneDist`, `_maxSpeed`, `_turnRate`, `_acc`.
- The meaning of hardpoint `flags` bit `0x2` and of `damage[0..4]`.
- Which units leave `DEST.OT` and when.
- Whether retail activates the 321 SAM and AAA objects standing in the base
  layouts. Nothing in the data marks them inert.

## Reproduce

```sh
cargo run -p tore-formats --example surface_inspect -- gameassets/fighters-anthology/FA_2.LIB nt SA6
cargo run -p tore-formats --example surface_inspect -- gameassets/fighters-anthology/FA_2.LIB summary
cargo run -p tore-formats --example surface_inspect -- gameassets/fighters-anthology/FA_2.LIB zones
cargo test -p tore-formats quick_template::import_tests -- --ignored
```

The ignored tests need the retail install (`TORE_GAME_DIR`, or the
`gameassets` link) and check that all 84 NTs parse, that every shape, store and
damaged look they name exists, and the counts above.

Which of these records, shapes and textures the import keeps, and why, is in
[the import contract](../spec/import-cache.md#ground-target-data-slice-im1); the
walk is `tore_formats::surface_set` and its proof is
`cargo test -p tore-import --test surface_data -- --ignored`.
