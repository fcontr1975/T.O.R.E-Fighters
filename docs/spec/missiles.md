# Missile guidance and lifetime

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

The energy-flight overhaul has its own [behaviour contract](missile-energy.md)
and [calibration baseline](../baselines/missile-energy.md). This page remains
the home of reviewed roles, acquisition/support and the previous Spec motion.
Energy-specific flight, estimates and physical decoy consequences supersede
those parts only when Energy rules are selected.

Implementation mode, 2026-09-17. Delivery status is in the
[feature matrix](../features.md) and measured checks in the
[baseline](../baselines/missiles.md).
John requested four guidance types, pitbull, range, motor burn and tracking
lifetime. IR is independent of passive emitter homing. The activation thresholds
and launch modes are **opinionated game rules requested by John**. The manual supports delayed seeker activation in general, but does
not establish our distances or the additional launch mode.
John also requested approximate per-weapon activation distances on 2026-09-17.
John also requested launch-velocity inheritance, uncued seeker-active launches,
a HUD cone and narrow forward IR acquisition with tone. The numeric defaults,
unit choice and detailed rules below are agent proposals. Sequencing is in
[the missile update plan](../missile-update-plan.md).

## Four game guidance types

| Type | Before and after launch | Loss of support or signal |
| --- | --- | --- |
| S: supported radar | Aircraft holds radar lock on this missile's target throughout guided flight. For future ground/ship weapons, the equivalent support comes from their launcher. | No steering from hidden target state after support loss. Retain the last observed intercept and seek renewed support until guidance lifetime expires. No pitbull. |
| A: active radar | Cued launch: snapshot the aircraft firing solution and intercept, then activate at the matrix distance. Boresight launch: own seeker searches immediately, without aircraft designation or radar lock. | Aircraft updates are optional. Losing aircraft support freezes the last supported intercept; it does not destroy the missile or expose live target coordinates. After seeker acquisition, guide independently. |
| I: infrared | Use the weapon's own heat seeker, with either a designated target or narrow forward boresight search. Aircraft radar and installed FLIR are not required for boresight search. | Retain the last observed intercept and attempt same-target reacquisition until guidance lifetime expires. No radar or emitter fallback. |
| E: passive emitter | Use the surface weapon's receiver with designation for compatible emissions, or the HARM boresight cue ([air-to-ground](air-to-ground.md#missiles-and-the-harm-boresight-cue)). An enabled passive receiver does not transmit radar. | Once the seeker has acquired its radar, an emission shutdown does not stop it: it keeps homing on the object, angles only, while the object stays inside its zone-1 cone (retail data, slice W2). Before acquisition a silent object is not seen. No heat fallback. |

John revised loss behavior on 2026-09-17: retain the known intercept and attempt
same-target reacquisition within the applicable seeker cone until guidance life
expires. The 2-second **agent-fitted** memory timer now separates MEMORY from
LOST searching; it never permanently disables reacquisition. Search can steer
toward the frozen intercept, but cannot follow hidden movement. S still requires
launcher support; IR and emitter weapons retain their own sensor types. This
supersedes the draft permanent-loss rule and does not interpret `trackT`.
John's 2026-10-02 loop report adds a fitted guard: once a missile passes the
remembered intercept along its current velocity, stop steering toward that stale
point. Continue searching for the same target until guidance expires. A fresh
seeker observation or renewed launcher support permits steering again. This
prevents orbiting a missed memory point without shortening the weapon's lifetime.
Use current shared sensor support for S. A missile retains its own target ID;
changing cockpit selection never redirects an existing shot. At most one target
receives aircraft support at once. The same profiles now guide AI-owned shots;
[AI warning and defense policy](ai-awareness.md) specifies their observations
and defensive behavior.

## Shared classification

Implementation mode, 2026-10-15. `Weapon::seeker_kind` is the authority for
missile guidance, launch speech and ordnance labels. For guided records (flag
`0x1`), signature 2 means infrared, 3 with support flag `0x200` means
semi-active radar, 3 without it means active radar, and 4 means passive emitter.
Other signatures remain other guidance, not an invented Fox class. Retail bytes
are unchanged; the load-time variant rules below adjust owned weapon fields. Reviewed flight eligibility and the fitted activation distances
below remain separate from classification.

The only store-name overrides preserve the existing fitted surface policy:
`SA19.JT` and `SAN11.JT` are supported radar despite lacking `0x200`, as specified
in [surface defenses](surface-defenses.md#sam-missiles). There are no A2A
classification overrides. New exceptions belong in that one table in
`tore-formats`, with their evidence here, never in HUD-name comparisons.

John requested correct Fox terminology on 2026-10-15. Active A2A releases now
say Fox three (`^FOXTHR`), including AIM-120, MICA, AA-12 and AAM-L. This is an
**opinionated correction**, deliberately differing from the original's
Phoenix-only rule. Semi-active and IR releases retain Fox one and Fox two and
the original 50 percent generic-call alternative. Active releases use the
Phoenix's existing always-Fox-three rule, an agent decision. Surface-target
shots keep generic calls. See [radio](radio-chatter.md#weapon-launch-calls).

The 22 retail A2A records now have three derived R-27 stores. Five retail
records remain catalog-only: AA6, AA9, AEMP1, PL7 and PL10. MICA's radar
record represents the EM role; no MICA-IR or AIM-9L record is added.

## Corrected identities and R-27 stores

**Opinionated, requested by John on 2026-10-15.** AA10.JT remains the stable
R-27R resource identity, including in old replays, but its HUD reads AA-10A.
The three new resource keys are R27ER.JT, R27T.JT and R27ET.JT. Loading a
user's pack supplies these virtual records from AA10.JT in memory. The checked
weapon reader applies the variant rules, so the UI, host, clients, AI and
replay effects resolve one definition. No retail bytes are distributed.

Agent naming choice: compact NATO-style AA-10A/B/C/D on the HUD, full names
R-27R (AA-10A Alamo-A), R-27T (AA-10B Alamo-B), R-27ER (AA-10C Alamo-C),
and R-27ET (AA-10D Alamo-D) on selection in Load Ordnance. Cards use the
R-27 variant name. Other corrections: AA-2A / R-3S, AA-6A / R-40R family,
S530D / Super 530D, MICA-EM and AIM-54C. AA8's all-aspect record represents
R-60M. AA-11B keeps its distinct game IR variant, without the misleading
Backfire name; PL10-R explicitly means the 1997 radar concept, not today's
IR PL-10. AAM-L and AEMP-1 remain fictional game stores. Exact AA-11B and
historical PL-10 hardware identities remain unknown.

The **fitted** envelopes below are agent choices, not measured retail behavior
or real missile performance claims. R/ER are SARH, T/ET are IR. The ER/ET
larger motor multiplies donor acceleration by 1.25 and fuel/removal timers
by 1.5, with integer truncation. ER multiplies both donor range maxima by
1.5. T/ET clear launcher/support flags 0x300, use signature 2 and all-aspect
IR. All other donor physics, turn limits, damage and fuze arming remain.
Extended variants reuse the donor artwork, so their larger motor is modeled
in propulsion and mass, not a new mesh.

| Resource | HUD | Guidance / Fox | Nominal launch min/max ft | Seeker max ft | Burn / removal s | Mass lb |
| --- | --- | --- | --- | ---: | --- | ---: |
| AA10.JT | AA-10A | SARH / one | 12000 / 120000 | 120000 | 53 / 106 | donor |
| R27ER.JT | AA-10C | SARH / one | 12000 / 180000 | 180000 | 79.5 / 159 | 772 |
| R27T.JT | AA-10B | IR / two | 12000 / 100000 | 60000 | 53 / 106 | 541 |
| R27ET.JT | AA-10D | IR / two | 12000 / 150000 | 60000 | 79.5 / 159 | 756 |

Normal loadouts allow all four on Su-27 and Su-35 medium-missile rails,
and on the MiG-29's inner pair (source hardpoint row 5). MIG29.PT is the
Fulcrum-C, not an early export Fulcrum-A; the [US Army MiG-29S entry](https://odin.t2com.army.mil/WEG/Asset/8e5ee70c6f22f4df21b8514ebc3a68e9)
lists T1/ER1/ET1 capability. Yak-141 takes R/T; extended variants are withheld
there as an agent choice. No Su-30 aircraft exists in the current 36-aircraft
roster. Existing defaults remain: Flankers use AA-12, MiG-29 uses AA-8,
Yak-141's AA10 is R-27R. Wingtip and short IR-only rails do not gain R-27
capacity; T/ET require underwing positions for their seeker view. Cheat
loading retains its explicit override.

Real-world family grounding: [Rosoboronexport R-27ER1](https://roe.ru/pdfs/pdf_4712.pdf)
and [R-27ET1](https://roe.ru/pdfs/pdf_4714.pdf) identify the extended radar
and IR family. These references do not calibrate this game's motor timers.

## Smaller guided bomb stores

**Opinionated, requested by John on 2026-10-15.** GBU-12, GBU-16 and GBU-38
are derived game stores. Their guidance, seeker, steering class, release and
impact sounds, fuze rules, lifetime and loadout art stay those of the donor.
They are not recovered retail variants. The numerical scaling and model
sizes below are fitted agent decisions for the requested weight classes.

| Resource / HUD | Donor | Weight lb | Damage multiplier | Shape alias | Length / diameter / fin span ft |
| --- | --- | ---: | ---: | --- | --- |
| GBU12.JT / GBU-12 | PAVEWAY.JT, Paveway II | 500 | 0.25 | GB12.SH | 10.8 / 0.90 / 4.4 |
| GBU16.JT / GBU-16 | PAVEWAY.JT, Paveway II | 1000 | 0.50 | GB16.SH | 12.1 / 1.17 / 5.0 |
| GBU38.JT / GBU-38 | GBU29.JT, JDAM | 500 | 0.25 | GB38.SH | 7.7 / 0.90 / 1.67 |

All five `damage.by_class` values divide by four or two, with integer
truncation toward zero. `damage.collateral_radius` divides by the cube root
of that divisor and rounds to the nearest integer. `collateral_percent`,
fuze radius, arm time and side-hit failure remain the donor's. Thus splash
strength inherits the smaller class damage and splash reach shrinks by
volume scaling. Explosion types and the record's crater-presence field stay
unchanged. Existing [explosion sizing](explosions.md#size-by-weapon) uses
`round(100 * (weight / 500)^(1/3))`: 100 percent for GBU-12/38 and 126 percent
for GBU-16, versus 159 percent for a 2000 lb donor. Craters use that same
scale on the existing 500 lb crater template when the donor leaves a crater.

The [shared bomb body](air-to-ground.md#bomb-motion) derives mass and drag
area from the smaller record weight. GBU-12/16 retain Paveway II bang-bang
steering; GBU-38 retains JDAM proportional steering. Existing AI store
classification and strike logic consume those same definitions.

Loading a pack supplies donor-identical virtual record bytes and private
shape aliases in memory. Parsing applies these rules. Each shape is refitted
to the table's length, body diameter and fin span for both carriage and flight;
the donor shape remains unchanged. No retail model or image is distributed.
The import fingerprint excludes only aliases whose bytes still match their
donor, including shape aliases, preserving old tapes' import identity.
Explicit different resources continue to participate in the fingerprint.

Every station that accepts a donor accepts its smaller variants. An exact
donor-default station recognizes the variant as belonging to that default
family. Other station compatibility masks remain in force. Existing count
limits and total weight-class limits determine carriage: a four-place,
2000 lb rack takes four 500 lb bombs or two 1000 lb bombs. Aircraft defaults
are unchanged. Cheat loading includes all three through its existing rules.
Validation is in the [bomb-flight baseline](../baselines/bomb-flight.md).

## Infrared aspect and acquisition

**Opinionated, requested by John on 2026-10-15; numeric model fitted by the
agent.** The reviewed retail `allAspect` field is 100 for AIM-9B and AA2,
30 for AIM-9M, AA11/B, AA8 and R550, 20 for AIM-9X, and 40 for PL7.
The correction table explicitly records rear-only B/AA2 and all-aspect
M/X, AA11/B, AA8 (R-60M role), R550, PL7 and the new R27T/ET. PL7 stays
catalog-only. The field's exact original numeric formula is unknown.

For the reviewed A2A stores against an airborne target, strength is `max(IR signature, 0) / 100 * engine heat`.
Engine heat retains 0.1 stopped, 0.5 + 0.5 * throttle running, 1.5 in
burner, or 1 for an unspecified source. Facing is the dot product of the
target's nose and its direction toward the seeker, +1 head-on, -1 rear.
All-aspect acquisition reach is `nominal seeker range * (0.7 - 0.3 * facing)
* sqrt(strength)`: a head-on fighter has 40% of its rear reach; burner extends
both by sqrt(1.5), capped by the imported seeker maximum. Rear-only weapons require a 45-degree rear quarter and
use the full nominal reach within it. Outside reach there is no return.
Inside, quality is `strength / (1 + (distance / reach)^2)`. Existing acquisition
and retention thresholds remain 0.25 and 0.20, with 30 ticks dwell at 120 Hz.
This fixes the previous head-on quality ceiling of 0.25, which distance made
strictly lower than the acquisition threshold.

SAM profiles retain their previous heat response and minimum-range rules.
AI jet observations refresh engine heat; rotorcraft retain their supplied heat
observation, including a handoff snapshot. Collective is not jet throttle.

The mounted seeker produces growl while acquiring and locked tone after dwell.
CUED IR can fire with or without its own lock, including head-on. A release
without lock starts BORE search. AI employment reads the same IR response and observed target
strength, retaining its minimum range, pointing, cadence and tactical discipline.
The player readout drives both local and client tone; recordings retain tone
changes for replay audio. [USAF Sidewinder](https://www.af.mil/About-Us/Fact-Sheets/Display/Article/104557/aim-9-sidewinder/)
confirms the L/M all-aspect distinction; no L store is present here.

## Manual-supported behavior

The [1999 EA/Jane's FA manual](https://pdfcoffee.com/famanual-pdf-free.html),
pp. 83-84, describes weapon/count, target range, signed closure, aspect,
hit probability, IN RNG, a fixed reticle, seeker diamond, target box and vertical
minimum/maximum range scale. Lock does not guarantee a good shot. Pages 118-119
describe delayed active-radar acquisition, designated IR launch and stronger
A2A IR growl with better lock; A2G IR uses a different tone. Page 137 relates
closure to reach. These passages do not establish uncued launch or our numerical
rules. Manual text was reviewed, not pixel geometry or a matching retail run.

## First-pass inventory matrix

All 135 retail JT definitions in the import were reviewed for inventory inclusion.
The three virtual R-27 variants above are separate from this raw census. The table
contains **63 missile or missile-like candidates**, including ground/ship and
unresolved special roles, not 63 accepted playable missiles. [Inventory method,
source identity and validation](../baselines/missiles.md).

An asterisk means a **proposed game classification from record fields**, not a
confirmed retail guidance contract or a real-world missile identification.
`sig=2` suggests I; `sig=3` with support flag `0x200` suggests S, otherwise A;
`sig=4` suggests E. [Evidence limits and exceptions](../formats/missiles.md).
Hold rows remain inventoried and outside implementation until their role is
specified. Do not force laser/designator or command-guided cases into these four.

**Live** means included in the current aircraft default-store allowlists,
not that all guidance behavior has passed acceptance. **Catalog** means inventory
only, hidden from Load Ordnance until flight support is connected under the
[catalog availability rule](ordnance-presentation.md#catalog-availability).
Missile presence does not authorize SAM, ship, ground-target or AI work.
John authorized surface air defenses on 2026-10-10: the SAM and ship records
that surface units fire have reviewed profiles, supported radar or infrared, as
listed in [surface defenses](surface-defenses.md#sam-missiles) (the S and I
rows above; ASROC and the anti-ship SS-N-9 stay without one). They stay
Catalog: no aircraft carries them.

Ranges are the source launch envelope converted using **6,076 feet per nmi**,
rounded to two decimals. They are not a guaranteed intercept range. Times use
the **existing fitted host conversion of 4 source timer units per second**.
Ignition and removal are launch-relative; burn is `(fuelT - igniteT) / 4`.
These are nominal compatibility numbers, not real-world motor specifications or measured
retail seconds. Exact source values, not rounded table text, drive implementation.
The old host clock quantizes launch age to quarter seconds; the new profile
should use launch-relative 120 Hz timers. Removal can precede burnout, as noted
below. Tracking/battery life remains a
separate proposed profile setting, never inferred from these motor numbers.

| Record (current HUD label) | Type | Availability | Launch envelope nmi | Ignition s | Burn s | Removal s | Seeker max nmi | Seeker H/V half-angle deg | Active-on distance nmi (fitted) | Uncued search H/V deg (fitted) |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | --- |
| AA10.JT (AA-10A) | S* | Live | 1.97 to 19.75 | 0 | 53 | 106 | 19.75 | 45/45 | N/A | N/A |
| AA11.JT (AA-11) | I* | Live | 0.33 to 9.87 | 0 | 26 | 52 | 9.87 | 80/80 | N/A | 5/5 |
| AA11B.JT (AA-11B) | I* | Live | 0.25 to 9.87 | 0 | 26 | 52 | 9.87 | wide*/90 | N/A | 5/5 |
| AA12.JT (AA-12) | A* | Live | 1.97 to 24.69 | 0 | 66 | 132 | 24.69 | 60/60 | 5 | 5/5 |
| AA2.JT (AA-2A) | I* | Live | 0.66 to 3.95 | 0 | 4 | 20 | 4.94 | 45/45 | N/A | 5/5 |
| AA6.JT (AA-6A) | S* | Catalog | 1.97 to 14.81 | 1 | 39 | 80 | 14.81 | 45/45 | N/A | N/A |
| AA8.JT (AA-8) | I* | Live | 0.66 to 3.95 | 0 | 4 | 20 | 4.94 | 45/45 | N/A | 5/5 |
| AA9.JT (AA-9) | S* | Catalog | 1.97 to 41.15 | 2 | 130 | 264 | 41.15 | 45/45 | N/A | N/A |
| AAML.JT (AAM-L) | A* | Live | 4.94 to 74.06 | 0 | 130 | 270 | 74.06 | 45/45 | 8 | 5/5 |
| AEMP1.JT (AEMP-1) | A* | Catalog | 0.33 to 14.81 | 0 | 35 | 70 | 19.75 | 45/45 | 3 | 5/5 |
| AGM45.JT (AGM-45) | E* | Live | 0.00 to 9.87 | 0 | 10 | 40 | 9.87 | 45/45 | N/A | 45/45 (HARM cue) |
| AGM65A.JT (AGM-65A) | I* | Live | 0.00 to 8.23 | 1 | 9 | 40 | 8.23 | 45/45 | N/A | 5/5 |
| AGM65G.JT (AGM-65) | I* | Live | 0.08 to 9.87 | 1 | 9 | 40 | 9.87 | 45/45 | N/A | 5/5 |
| AGM84A.JT (AGM-84A) | A* | Live | 0.08 to 59.25 | 2 | 118 | 120 | 59.25 | 45/45 | 8 | N/A |
| AGM84E.JT (AGM-84E) | I* | Live | 0.08 to 49.37 | 2 | 118 | 120 | 49.37 | 45/45 | N/A | 5/5 |
| AGM88.JT (AGM-88) | E* | Live | 0.08 to 32.92 | 0 | 25 | 40 | 32.92 | 45/45 | N/A | 45/45 (HARM cue) |
| AIM120.JT (AIM-120) | A* | Live | 1.97 to 23.70 | 0 | 66 | 132 | 23.70 | 45/45 | 5 | 5/5 |
| AIM54C.JT (AIM-54C) | A* | Live | 4.94 to 98.75 | 2 | 139 | 283 | 98.75 | 45/45 | 10 | 5/5 |
| AIM7.JT (AIM-7) | S* | Live | 1.32 to 19.75 | 2 | 51 | 106 | 19.75 | 45/45 | N/A | N/A |
| AIM7E.JT (AIM-7E) | S* | Live | 0.49 to 16.46 | 1 | 11 | 60 | 16.46 | 45/45 | N/A | N/A |
| AIM9B.JT (AIM-9B) | I* | Live | 0.66 to 3.95 | 0 | 3 | 20 | 8.23 | 45/45 | N/A | 5/5 |
| AIM9M.JT (AIM-9M) | I* | Live | 0.66 to 3.95 | 0 | 11 | 22 | 8.23 | 45/45 | N/A | 5/5 |
| AIM9X.JT (AIM-9X) | I* | Live | 0.49 to 3.95 | 0 | 15 | 24 | 8.23 | 75/75 | N/A | 5/5 |
| AM39.JT (AM-39) | A* | Live | 0.08 to 59.25 | 2 | 64 | 132 | 59.25 | 45/45 | 8 | N/A |
| AS14.JT (AS-14) | Hold: designator | Catalog | 0.08 to 6.58 | 1 | 9 | 30 | 8.23 | 45/45 | TBD | TBD |
| AS15.JT (AS 15) | S* | Live | 0.08 to 9.87 | 1 | 19 | 40 | 9.87 | 45/45 | N/A | N/A |
| AS16.JT (AS-16) | A* | Live | 0.08 to 6.58 | 1 | 9 | 30 | 8.23 | 45/45 | 2 | N/A |
| AS30.JT (AS 30L) | Hold: designator | Catalog | 0.08 to 9.87 | 1 | 12 | 26 | 9.87 | 45/45 | TBD | TBD |
| AS7.JT (AS-7) | S* | Live | 0.08 to 4.94 | 1 | 9 | 30 | 8.23 | 45/45 | N/A | N/A |
| ASROC.JT (ASROC) | Hold: radar role | Catalog | 0.08 to 12.34 | 2 | 20 | 40 | 16.46 | wide*/90 | TBD | TBD |
| AT12.JT (AT-12) | Hold: designator | Catalog | 0.16 to 4.28 | 3 | 97 | 180 | 4.28 | 45/45 | TBD | TBD |
| AT2.JT (AT-2) | Unguided fitted host adapter | Live | 0.08 to 2.96 | 1 | 9 | 30 | 3.29 | 45/45 | TBD | TBD |
| FIM92.JT (FIM-92) | I* | Catalog | 1.23 to 8.89 | 0 | 4 | 20 | 16.46 | wide*/90 | N/A | 5/5 |
| HQ2J.JT (HQ-2J) | S* | Catalog | 1.23 to 8.89 | 2 | 21 | 40 | 16.46 | wide*/90 | N/A | N/A |
| HQ61.JT (HQ-61) | S* | Catalog | 1.23 to 8.89 | 2 | 21 | 40 | 16.46 | wide*/90 | N/A | N/A |
| MICA.JT (MICA-EM) | A* | Live | 1.97 to 24.69 | 0 | 66 | 132 | 24.69 | 45/45 | 5 | 5/5 |
| MIM23.JT (MIM-23) | S* | Catalog | 0.16 to 7.41 | 1 | 9 | 20 | 15.43 | wide*/90 | N/A | N/A |
| MIS.JT (MIS) | S* | Catalog | 1.23 to 7.90 | 2 | 21 | 40 | 10.53 | wide*/90 | N/A | N/A |
| PL10.JT (PL10-R) | S* | Catalog | 1.32 to 24.69 | 2 | 18 | 40 | 24.69 | 45/45 | N/A | N/A |
| PL7.JT (PL-7) | I* | Catalog | 0.66 to 3.95 | 0 | 4 | 20 | 8.23 | 45/45 | N/A | 5/5 |
| R440.JT (R-440) | S* | Catalog | 1.23 to 8.89 | 2 | 21 | 40 | 16.46 | wide*/90 | N/A | N/A |
| R530.JT (S530D) | S* | Live | 1.32 to 16.46 | 0 | 53 | 106 | 19.75 | 45/45 | N/A | N/A |
| R550.JT (R-550) | I* | Live | 0.66 to 3.95 | 0 | 7 | 20 | 8.23 | 45/45 | N/A | 5/5 |
| ROLAND.JT (Roland) | S* | Catalog | 0.16 to 4.44 | 1 | 9 | 20 | 7.41 | wide*/90 | N/A | N/A |
| SA13.JT (SA-13) | I* | Catalog | 0.25 to 2.47 | 1 | 9 | 20 | 4.11 | wide*/90 | N/A | 5/5 |
| SA14.JT (SA-14) | I* | Catalog | 0.08 to 2.47 | 0 | 22 | 20 | 3.29 | wide*/90 | N/A | 5/5 |
| SA15.JT (SA-15) | S* | Catalog | 0.16 to 5.92 | 1 | 9 | 20 | 13.17 | wide*/90 | N/A | N/A |
| SA16.JT (SA-16) | I* | Catalog | 0.08 to 1.32 | 0 | 5 | 20 | 2.47 | wide*/90 | N/A | 5/5 |
| SA19.JT (SA-19) | S (surface, default pending John) | Catalog | 0.08 to 3.95 | 0 | 5 | 20 | 9.87 | wide*/90 | TBD | TBD |
| SA2A.JT (SA-2A) | S* | Catalog | 1.23 to 15.64 | 2 | 21 | 40 | 24.69 | wide*/90 | N/A | N/A |
| SA3.JT (SA-3) | S* | Catalog | 1.23 to 8.89 | 2 | 21 | 40 | 16.46 | wide*/90 | N/A | N/A |
| SA6.JT (SA-6) | S* | Catalog | 1.48 to 12.34 | 2 | 21 | 20 | 13.17 | wide*/90 | N/A | N/A |
| SA7.JT (SA-7) | I* | Catalog | 0.08 to 1.48 | 0 | 5 | 20 | 2.47 | wide*/90 | N/A | 5/5 |
| SA9.JT (SA-9) | I* | Catalog | 0.41 to 2.96 | 0 | 10 | 20 | 3.29 | wide*/90 | N/A | 5/5 |
| SAN11.JT (SA-N-11) | S (surface, default pending John) | Catalog | 0.25 to 3.95 | 2 | 8 | 40 | 16.46 | wide*/90 | TBD | TBD |
| SAN3.JT (SA-N-3) | S* | Catalog | 0.82 to 16.46 | 2 | 8 | 180 | 16.46 | wide*/90 | N/A | N/A |
| SAN4.JT (SA-N-4) | S* | Catalog | 0.66 to 5.76 | 2 | 8 | 40 | 16.46 | wide*/90 | N/A | N/A |
| SAN5.JT (SA-N-5) | I* | Catalog | 0.08 to 1.48 | 2 | 3 | 20 | 2.47 | wide*/90 | N/A | 5/5 |
| SAN7.JT (SA-N-7) | S* | Catalog | 0.25 to 9.87 | 2 | 8 | 40 | 16.46 | wide*/90 | N/A | N/A |
| SAN8.JT (SA-N-8) | I* | Catalog | 0.08 to 2.47 | 2 | 3 | 40 | 4.11 | wide*/90 | N/A | 5/5 |
| SAN9.JT (SA-N-9) | S* | Catalog | 0.16 to 5.92 | 2 | 8 | 40 | 13.17 | wide*/90 | N/A | N/A |
| SEA_SPAR.JT (AIM-7) | S* | Catalog | 0.49 to 19.75 | 0 | 10 | 40 | 19.75 | wide*/90 | N/A | N/A |
| SSN9.JT (SS-N-9) | S* | Catalog | 0.49 to 19.75 | 0 | 100 | 360 | 41.15 | wide*/90 | N/A | N/A |

Seeker maximum uses source `zone0`; relative-altitude limits remain binding.
Minimum range does not suppress an aircraft-launched missile seeker. Half-angles use 182 source units per degree. `wide*` denotes
the 0x7fff source value, whose wide-angle handling needs review, not a normal
180-degree cone. These columns describe imported geometry, not proven detection
range or completed seeker behavior. The uncued-search column contains agent-fitted
half-angle caps, separately from the imported seeker envelope. IR search uses the smaller of the fitted cap and source limit. Active airborne
maddog search now uses the source limits without the fitted cap; the table
caps for active stores describe the cockpit cue only.

## Variety-default weapon extension

Implementation choice, 2026-10-05. The source-reviewed AA10, AIM7 and AIM7E
records use the existing supported-radar family: source signature 3 and support
flag 0x200. AIM9B uses the existing infrared family: source signature 2. Their
own source ranges, cones, motor timers and steering limits remain binding.
Guidance lifetime uses each source `removeT * 30` at 120 Hz and the existing
240-tick fitted memory interval. These classifications follow the inventory
candidates above; exact retail homing behavior is still unvalidated.

AGM88 uses the existing passive emitter profile on a designated surface radar
emitter, or in BORE on the HARM cue's candidate. It accepts actual enabled radar
emissions and no jammer homing. After acquisition an emission shutdown no longer
stops it: it homes on the object on angles only (retail data; slice W2, see
[air-to-ground](air-to-ground.md#missiles-and-the-harm-boresight-cue)). Source ground-object systems
and target categories determine whether a compatible surface emitter is present;
an airborne fighter's radar is not a compatible substitute.

AT2 has source signature 0. Its original command-guidance behavior is unknown.
The host now allows the Mi24 default AT2 store as an explicitly **fitted unguided
forward weapon**, using its own source launch/motor/coast/removal and damage
records through the shared unguided path. It receives no invented IR, radar or
emitter seeker, no designation lock, and no automatic steering. Source ignition
is 1 second, powered duration 9 seconds, removal 30 seconds under the shared
4-source-units-per-second conversion. This is a known difference from a possible
original guided mode. The next research step is to recover the original AT2
player command-guidance control contract before adding that mode.

## Activation and independent acquisition

The matrix's **Active-on distance** is the proposed profile attribute
`active_seeker_activation_nmi`. Measure straight-line distance in three dimensions
from the missile to its **last known intercept point**. Activate at or below that
weapon's threshold, using 6,076 feet per nmi. This is not distance traveled,
distance from the launcher or necessarily current distance to the target.
Once activated, the seeker stays on. A launch already inside the threshold starts
searching immediately. Aircraft updates can move the remembered intercept while
support exists; losing support freezes it, including for the activation test.

These are **fitted game defaults selected by the agent**, as requested by John,
not recovered FA activation distances or estimates of real-world seeker hardware.
They replace the universal five-mile draft rule. The first-pass tuning rule uses
the imported launch maximum to give shorter-range game weapons a later search
phase and longer-range weapons more search distance:

- Below 10 nmi launch maximum: activate at 2 nmi.
- From 10 to below 20 nmi: activate at 3 nmi.
- From 20 to below 40 nmi: activate at 5 nmi.
- From 40 to below 80 nmi: activate at 8 nmi.
- At least 80 nmi: activate at 10 nmi.

Use exact source range for the bands, not the rounded display value. These bands
explain this draft's choices; the explicit per-weapon matrix value is the profile
setting and can be tuned independently later. No historical era or hardware
capability is inferred. Activation distance does not replace or expand seeker
range, cone, signature or terrain checks, and does not guarantee acquisition.

Only A rows receive numeric values. S/I/E have `N/A`, meaning no active-radar
transition, not activation at zero range. Held rows have `TBD`; do not create an
activation profile until their guidance type is resolved. Validate any configured
activation distance as finite and positive. The numeric values remain conditional
on the starred guidance classifications being accepted.

For cued launch, calculate the intercept using the launch and target motion
contract below. Update it only from new observations while the same target has
valid aircraft support. Never update from hidden target movement. A failed
intercept estimate falls back to the last observed target position and shows
`EST --` in the debug window and `0%` in the HUD; it must not
invent a time-to-go.

Boresight launch overrides delayed activation: the seeker is enabled before
release and remains enabled afterward. No guessed intercept is required to fire.
The matrix's active-on distance applies to cued A launches only.

Separate `MIDCOURSE`, `ACTIVE SEARCH`, `PITBULL`, `LOST` and `EXPIRED` states.
`PITBULL` requires a successful seeker acquisition, not merely crossing the
activation distance. Cued shots search for their assigned target only. Uncued
shots may acquire one eligible contact from their own cone; after acquisition,
keep that identity and use the same loss rules. Active search continues until
guidance expiry. There is no opportunistic switching after acquisition.
After acquisition, loss uses the same MEMORY/LOST indications as I, with
same-target reacquisition allowed until guidance expiry.
Guidance expiry has priority over activation or reacquisition on the same tick.

Use each weapon's imported seeker range and horizontal/vertical half-angles,
with explicit handling for wide-angle/sentinel records. The current live cone
collapses horizontal and vertical limits; correct that before claiming the
matrix's seekers work as specified. Do not borrow the launching aircraft's cone.
Seeker observation must account for terrain masking and the relevant target
signature: shared radar aspect/RCS for A, IR signature for I, actual enabled
emissions for E. An emitter switch-off cannot be replaced by its reflective RCS.
Radar noise alone must not reveal hidden target position or identity.

Era-based seeker presets are optional **fitted** fallbacks for missing detection
or countermeasure parameters, not replacements for imported range/angle values.
No era constants or per-weapon era assignments are established in this pass.
Implement the imported geometry first; document and test numeric presets before
adding any. Radar and jammer homing eligibility must be explicit per E profile;
AGM45/AGM88 are candidates, not evidence that both can home on every jammer.

## Launch velocity and intercept estimates

**Opinionated behavior requested by John, 2026-09-17:** aircraft velocity at
release contributes to missile motion. Motor boost develops over the burn, and
observed target speed and direction affect the calculated intercept and closing
speed. This applies to actual game motion as well as HUD estimates.

Capture aircraft **world velocity as a vector**, not just speed along the nose.
A climbing or slipping aircraft must pass its upward/sideways motion to the
missile. Snapshot this once at release; later aircraft acceleration cannot change
a missile already in flight. The seeker initially points along the launch rail,
which can differ from the inherited flight direction.

The first-pass authored profile uses 100% velocity inheritance and zero additional
rail-ejection velocity. Those are agent-selected fitted values. Keep the existing
source launch-speed helper in the compatibility profile: it selects and clamps
a scalar using `launchRetard` and `initialSpeed`; it does not implement this
vector-additive rule. Do not relabel the new interpretation as recovered FA code.

For a bounded game boost estimate, use source acceleration over elapsed powered
time, capped by a fitted motor velocity-gain budget:
`max(0, altitude-adjusted source maximum speed - source initial speed)`.
The budget is a game reuse of imported values, not their recovered meaning.
Apply no gain before ignition; stop adding at burnout. Accumulate the gain once,
in the current thrust direction, while existing turn limits constrain steering.
For an unsteered estimate, missile velocity is the launch aircraft vector plus
this accumulated forward boost vector. Steering must not increase speed by
itself. After burnout, use source deceleration toward source coast speed, without
instant speed jumps or continued thrust. Keep the old absolute powered-speed
command in the compatibility profile rather than applying both laws together.

Use the same motion predictor for launch suitability, intercept, time-to-go and
in-flight movement. Predict against the latest **observed** target position and
velocity in the same coordinate frame. Account for approaching, receding,
crossing, climbing and descending targets. Bound prediction by remaining guidance
and object lifetime; show debug `EST --` and HUD `0%` when
interception is not predicted.
A target's motion changes the intercept; it never adds propulsion to the missile.

For the target directly ahead, closing speed is missile speed toward the target
minus target speed away along that line. More generally use the projection of
`missile_velocity - target_velocity` onto the missile-to-target sight line.
Cockpit aircraft-to-target closure is a separate readout using aircraft velocity.
Do not add that aircraft closure again after already inheriting aircraft speed.

Synthetic example, not a catalog weapon: aircraft speed 600 ft/s and accumulated
forward motor gain 1,000 ft/s give a straight-flight missile speed of 1,600 ft/s.
A target approaching at 300 ft/s gives 1,900 ft/s closure; receding at 300 ft/s
gives 1,300 ft/s. A perpendicular 300 ft/s target requires lateral lead, not
1,900 ft/s missile speed. Starting with an aircraft at 300 ft/s instead gives
1,300 ft/s missile speed at the same accumulated boost. A side-slip case must
retain the launch's sideways component rather than replacing it with nose heading.

## Uncued launch and narrow IR search

**Opinionated behavior requested by John, 2026-09-17.** Accepted air-to-air weapons with
an independent onboard seeker, A/I, get an explicit `BORESIGHT` mode alongside
`CUED`. Here “active internal seeker” means an enabled seeker; I and E remain
passive sensors. Supported radar S still needs launcher support. Held and
laser/designator rows gain no capability from the generic switch.

Implemented control: a rebindable `weapon-seeker-mode` action (no default key) and
the clickable mode label in the upper-right weapon diagnostic panel switch modes;
no existing key is silently reassigned. The panel is hidden by default, and its
labels take clicks only while it is shown.
Arming an independent air-to-air missile with no designation automatically enters
BORESIGHT when guidance is enabled. IR does not require radar power.
Designating a contact returns to CUED. The explicit mode switch remains available
with a retained designation for radar missiles; selected-track priority prevents
that override for IR missiles. ; (FA's unlock key, see [keyboard](keyboard.md)), L and the
diagnostic panel's RELEASE LOCK button clear the aircraft designation and
mounted seeker, without redirecting airborne shots. L is an earlier host key,
kept alongside. Snapshot
the mode at launch; changing modes later cannot retask missiles in flight.

In BORESIGHT, an armed, operational, loaded weapon can fire without designation,
installed FLIR, radar power or seeker lock. IR searches forward on the rail
and after release. Active radar guidance searches only after release, enabling
its seeker immediately and requiring acquisition before reporting PITBULL.
A separate prelaunch estimate can identify a provisional bore contact. With a lock, retain that target; without one, fly on
and search until acquisition or guidance expiry. Keep normal station, bay, trigger
and ammunition gates. Missing target data cannot produce a fake range inhibit:
omit `IN RNG` for an uncued release. Minimum range is advisory and never blocks release.
Other envelope cues still warn without blocking uncued release.
The trigger opens an internal bay, with or without a cockpit designation ([F-22 bays](aircraft-animation.md#f-22-main-weapon-bays)).

The fitted bore is a **5-degree circular half-angle** for independent seekers,
limited by the imported seeker volume. John requested a smaller bore on 2026-09-17; the latest "5%" wording is
interpreted as five degrees. The half-angle interpretation preserves the existing
meaning of the setting. It follows the rail axis before launch and the IR missile nose afterward,
independent of camera look and zoom. Active airborne search uses the imported
seeker volume without that five-degree cap.
Mounted bore IR stays inside the circle even after acquisition. Released seekers
use imported tracking limits after acquisition.
No source cone value is overwritten. Keep normal terrain, range, target-class
and signature/emission eligibility checks, including for unknown uncued contacts.

Uncued acquisition is a weapon-seeker rule authorized here, not aircraft combat
AI. Evaluate only contacts observed by that seeker. For IR, select the strongest
eligible heat-quality score; ties use smallest off-axis angle, shortest range,
then stable target ID. Active radar chooses the strongest directional radar
signature divided by `100 * (1 + (range / nominal_range)^2)`, then the same ties.
For IR and active radar, multiply selection score by
`1 - 0.75 * clamp(off_axis / bore_half_angle, 0, 1)^2`.
This agent-fitted centre preference gives the edge one quarter of the centre's
weight without making a much stronger edge return disappear.
Emitter seekers retain angle, range and ID ordering. These rules are agent-fitted.
Require 0.25 seconds of continuous eligibility to lock.
Switching a candidate restarts that dwell. Mounted bore IR can switch to a
stronger eligible return; a released missile keeps its acquired identity. During loss memory, retain the identity but show `MEMORY` instead of a
confirmed lock; suppress lock tone until reacquisition completes. Existing fitted 2-second memory applies after loss. No team label can
make a physically visible return disappear; known-friendly warnings may use
existing IFF, but unknown contacts must not gain hidden identity information.

### Fitted heat quality and tone

The current numeric rule is defined once under [infrared aspect and
acquisition](#infrared-aspect-and-acquisition). It separates shorter head-on
reach from signal quality, so an eligible all-aspect return can complete dwell.
Terrain masking and the seeker's gimbal still reject observations. These are
fitted game numbers, not retail measurements.


Selected armed missiles produce local seeker audio. IR growl now follows the
same fitted percentage displayed on the HUD, with quieter tracking and louder
lock. The recovered original air-to-air recording is `&IR1.11K` for both
states. Surface IR uses the original radar-named search/lock pair without a
radar power requirement. Radar gain retains the original implementation's
quality curve. The [sound specification](sound.md) owns the recording evidence;
the [audio guide](../audio.md#seeker-growl) owns gain, fade and mute rules.
Default maximum remains 0.30. The earlier IRTRY/IRLOCK assignment was fitted
and is superseded by the reviewed executable mapping.

Every seeker cue plays only while the seeker is actively tracking a return;
with nothing in the seeker it is silent. A radar missile in boresight plays
its lock cue at `40% + 60% * quality` on its bore return, with or without a
designated target. Any radar missile falls silent while its tracked target is
inside the weapon's minimum range. John, 2026-09-23.


Only the selected mounted seeker produces this tone. Silence it on safe, empty,
failed station, pause, leaving flight or switching away from IR/radar missiles; pause must also
freeze acquisition timers. After firing, stop the spent seeker's tone. A newly
selected remaining round begins its own search and dwell. Do not keep sounding
lock from an airborne missile as though the next round had acquired it.

## Weapon HUD delivery

**Opinionated cleanup requested by John, 2026-09-17.** Armed missile selection
shows short weapon/count, readiness warnings and a bare `n%` hit estimate.
The later [HUD layout](hud-layout.md) moves these rows below the expanded
ladder and limits AGL/vertical speed to non-weapon ILS guidance.
John's annotated layout request reduces the HUD text and fixed layout by 15%
from the previous size (scale 0.85 to 0.7225). Angular cues retain their actual
world alignment and five-degree bore geometry. Weapon/count and percentage
start at reference x=207, aligned with the speed box's left edge. The range scale
is 52 reference pixels tall at x=390, y=230..282, inside the altitude tape.
The altitude box's left border is x=401, two pixels beyond the former tick endpoint
x=399, matching the speed-side gap. Its text starts at x=405. Tape extent and lower readout coordinates
follow the expanded [HUD layout](hud-layout.md). TARGET DESTROYED is omitted
from the HUD; its simulation release inhibit and debug status remain.
Suppress the redundant BORE READY
message, but retain release-inhibiting warnings and CUED IN RNG. Use the imported first `si_names` string for the HUD,
even when loadout menus use the second, longer description. Manual pp. 83-84
establish the readout meanings; our probability rule and layout are fitted.
Stall, engine-off and crash warnings remain visible. Safe retains flight
readouts for missile selection, with AGL/VS restricted to active ILS, and hides missile range, probability, diamond
and ARM cues. The selected target's square or off-HUD chevron is independent of
master arm, selected weapon and current radar observation; its presentation-only
selection does not supply missile support. See the shared
[gun and target-cue specification](gunsight-targeting.md). Safe guns retain their
ammunition/SAFE text but hide the firing pipper.

All weapon symbology is drawn in the same aircraft-forward HUD layer as flight
symbology. Circle, labels, estimate and range scale translate and fade together
when looking away, and zoom together. Clip in forward HUD coordinates before
applying head-look, never against a stationary screen-centred HUD rectangle.

BORE shows a blinking diamond on one provisional contact, without creating a
target box for that provisional return. A separately selected HUD target may
still have its own square or edge chevron.
It blinks at 2 Hz with 50 percent duty, using simulation ticks so pause freezes
it. This is an estimate, not an aircraft designation or seeker lock. Active radar
launches still have no preassigned target and acquire independently after release.
IR's real dwell and tone remain independent of this immediate provisional cue.
Choose the provisional contact using the same centre-weighted signal score and
stable tie rules as acquisition, within the actual bore, range and terrain gates.
No candidate means `0%` and no diamond or range scale.

When a candidate exists, show the imported minimum and engagement-dependent estimated maximum on the weapon range scale.
Its triangular target marker blinks with the provisional diamond, clamped to
an endpoint for targets outside the launch envelope. A cued, acquired radar
seeker diamond still blinks when launch-ready. Inferred bore cues never grant
launch permission or aircraft sensor support.

### Fitted estimated hit calculator

The original probability calculation is unknown. John requested a working
estimate on 2026-09-17; this is an agent-authored heuristic, not a calibrated
retail percentage or a guarantee. It scores current observation quality,
centring, launch envelope and simulated intercept margin:

- No current observation, an observation outside the launch envelope, or no
  intercept before guidance/removal expiry gives zero percent.
- `signal = clamp(observation_quality, 0, 1)`.
- In bore, `centring = 1 - 0.75 * clamp(off_axis / bore_half_angle, 0, 1)^2`.
  CUED uses 1 because its seeker is slaved to designation.
- `r = clamp((range - minimum) / (maximum - minimum), 0, 1)`;
  `envelope = 1 - 0.75 * r^2`.
- `life` is the smaller of guidance lifetime and removal time, in seconds.
  `margin = clamp(1 - 0.6 * intercept_seconds / life, 0, 1)`.
- Round `95 * signal * centring * envelope * margin` to an integer percentage.
  Cap at 95. These numeric weights are agent choices.

The intercept uses current observed velocity and the existing propulsion model,
so target closure affects the estimate. Future evasive manoeuvres, target intent,
weather and future countermeasures are not predicted. Recompute against the
same provisional contact used by the diamond and range scale. Do not alter the
missile's physics or random damage outcomes to force agreement with this cue.

Mode, actual seeker state, release button, `R`, signed `C`, `EST` flight time,
target aspect angle (`ASP`, in degrees),
and three recent shot diagnostics stay in the upper-right weapon diagnostic panel.
The panel is hidden by default and shown with **Escape → Pref → Weapon
diagnostics?**, a saved preference (opinionated, requested by John on 2026-09-23).
Unavailable numeric debug values use `--`. Only while the panel is shown does a
large upper-right instrument move down 104 reference pixels to clear it. The forward HUD omits BORE READY and retains CUED IN RNG; estimated chance
cannot confer a lock.

Use original HUD styling and fonts. Add seeker-state readouts and deterministic
sound controls to the app boundary; do not let the renderer or audio thread choose
targets or determine lock. Update the input, flight-controls, radar and architecture
guides when these controls and behaviors are implemented, not as shipped features
in advance. No new artwork or sound assets are required for this planning pass.

## Minimum engagement and target role

**Opinionated revision requested by John, 2026-10-15.** Imported minimum
range is information for the player and shot discipline for AI. It does not
inhibit a player missile release, suppress seeker acquisition, or disqualify a
close hit. This supersedes the earlier fitted launch-origin qualification.
Normal seeker geometry, signal, turn limits, propulsion and fuze arming decide
whether a released missile hits. No additional straight-flight or arming time
is invented. The HUD range scale and IN RNG still use the imported minimum;
radar tone keeps its existing below-minimum silence. AI still checks its own
minimum-range envelope before firing.

Weapon role is explicit, separate from seeker type and target damage category.
AGM65G, AGM65A, AGM84E, AGM45, AGM88, AGM84A, AM39, AS16, AS7 and AS15 require
surface targets (the list `missiles::SURFACE_ROLE`). Other reviewed implemented
profiles require airborne aircraft. Of the surface weapons the infrared ones and
the anti-radiation ones fire in BORE as well as on a designation (slice W2,
[air-to-ground](air-to-ground.md#missiles-and-the-harm-boresight-cue)); the
radar ones use the A-G track for supported cueing. Player active surface
missiles can instead launch maddog; unsupported player surface SARH shots
wait for later illumination under the player release rules below. Aircraft that land or
are assigned another damage-test category do not become surface targets.
The manual's air-to-ground and IR sections support Maverick's surface role and
its separate lock tone. Surface IR uses imported thermal signature without
fighter exhaust aspect/throttle multipliers, an **agent-fitted** contrast model.
AGM-65 remains unavailable against the practice aircraft rather than borrowing
its A2A targeting workflow. The surface designation channels
([air-to-ground](air-to-ground.md#ground-contacts-and-channels)) are built (G1)
and the surface missiles fly from them (W2).

The imported AGM65G minimum is 500 feet, whereas the manual table states 0.8 nmi.
This unresolved source discrepancy needs record/unit research. Keep the reviewed
imported value; do not replace it with an invented real-world value.

## Range, motor and tracking lifetime

### Hits on destroyed aircraft

Requested by John on 2026-10-02: guns and missiles can hit a destroyed aircraft
while its falling body is still present. Radar/visual selection and mounted
seekers can retain that body. Impact consumes the projectile and produces its
normal hit effect, including with friendly fire disabled, but applies no further
damage, pilot injury, system fault or kill credit to the wreck. Its
[splash](#splash-damage) still reaches anything else nearby. Once impact or airburst removes
the wreck, its old position is not a collision body. The simulation owns both
presence and collision; the renderer does not decide what is hittable.

### Independent lifetime rules

Keep four independent concepts:

1. **Launch envelope:** imported altitude/angle limits,
   with predicted intercept for advisory cues, not a maximum-range trigger gate. The imported
   nominal launch maximum does not cap the engagement estimate. BORESIGHT explicitly permits uncued
   release as specified above. Being in range does not promise a hit.
2. **Seeker envelope:** onboard acquisition/retention limits govern observations
   after launch. Losing aircraft lock does not extend them.
3. **Motor:** ignition delay, powered burn and coast are separate phases. Burnout
   changes speed/turn behavior; it does not automatically remove the missile or
   stop its seeker. The authored boost/inheritance rule above reuses imported
   acceleration and altitude values; retain powered/unpowered turn parameters.
   Keep the original speed interpretation in the compatibility profile. The fitted maneuver loss below supplements coast deceleration; it is not a
   physical drag model or a source of real-world range values.
4. **Guidance lifetime:** a separate `guidance_lifetime_s` starts at launch and
   ends steering and reacquisition permanently. Signal loss alone does not. Call this guidance lifetime in
   the UI, not battery life, since the source does not establish battery meaning.
   Agent-selected fitted fallback: equal to the source-derived removal lifetime.
   Profiles may explicitly shorten it; do not lengthen object life implicitly.
   An expired missile continues unguided until collision or cleanup. A shorter
   synthetic profile must prove that these are genuinely separate timers.

The source `trackT` is parsed but unused in live combat. Its units and purpose
are unknown here. Do not convert it into tracking duration or battery capacity.
A lock-loss memory timer is also separate from total guidance lifetime.

Do not kill a missile when its traveled distance reaches the launch maximum.
Actual reach emerges from motion, target movement, guidance and lifetime. Add
repeatable headless reach probes at launch altitude 10,000 feet and launcher
speeds 300, 600 and 900 ft/s against stationary targets and approaching, receding
and crossing fixtures at 600 ft/s. Add matched climb and side-slip launches.
Sample 25%, 50%, 75% and 100% of each weapon's launch maximum, respecting minimum
range. Report hits, misses and expiry, not a fabricated universal effective range.

For SA14 and SA6, imported removal precedes motor cutoff. Preserve that
compatibility behavior and report it; do not silently fix the source data.
The long burn values in the table are likewise the current game's timing choice.

## Splash damage

Decided by John on 2026-10-10: every weapon whose record carries a collateral
radius and percent does splash damage, whatever fired it. That is every
air-to-air and air-to-ground missile, every bomb and rocket, the SAMs and the
flak. The authored [AC-130 105 mm correction](ac130-linked-guns.md#105-mm-fragmentation-splash)
also enables splash, with armor resistance. Other AC-130 guns retain zero splash.

What a player sees:

- A missile that bursts on or beside an aircraft also hurts every other
  aircraft within its radius, so a wingman in close formation with the target
  can be damaged or killed by the shot meant for his leader, and a missile
  bursting near the player's own aircraft hurts it.
- A bomb or missile that hits the ground hurts every ground object, ship,
  vehicle and parked aircraft within its radius, and any aircraft low enough
  to be inside it. A near miss on a target is no longer harmless.
- The damage falls off in a straight line with distance, from the record's
  percent of its damage at the burst to nothing at the radius edge. Distance
  is measured to the target's surface: its box for a ground object, its body
  sphere for an aircraft, so a large building is reached as soon as its near
  wall is.
- A burst in the air or on an aircraft passes the share on whole. A burst on
  the ground (the terrain, the sea, a ground object or a ship) passes on 70
  percent of it for a weapon whose percent is 100 (every bomb) and half for
  any other.
- The aircraft or object a round struck directly takes only that direct hit.
- Bombs and unguided stores can catch the shooter in their own blast, as the
  manual warns (p. 126). That damage credits no one: no kill, strike or
  friendly-fire count. Their [launch grace](#launch-grace) still applies.
  Missiles instead follow John's permanent
  [launcher exclusion](missile-energy.md#motion), in every weapon mode.
- With friendly fire off, splash spares everything of the shooter's side,
  including aircraft, ground objects and parked aircraft. With it on, other
  aircraft remain vulnerable; a missile's own launcher remains excluded.
- A splash kill is credited to the shooter in the debrief, the replay and the
  multiplayer score, exactly as a direct kill is, and an ownship's splash
  kills count on its kill tally. Strikes, radio hit calls and jolts follow.
- Splash that only damages a bystander aircraft does not make the shooter its
  last attacker, because the tally holds one hit for the shot and that hit is
  the aircraft it struck; if the bystander is lost later, by colliding with its
  wingman for instance, nobody is credited
  ([one record](debrief.md#kills)). Agent decision, 2026-10-16.
- A surface unit's splash reaches aircraft and the other side's ground
  objects and ships, never its own side's or neutral scenery, whatever the
  friendly fire setting ([surface against
  surface](surface-defenses.md#surface-against-surface)).

Retail radii and percents (FA 1.02F records):

| Weapons | Radius | Percent |
| --- | ---: | ---: |
| Most air-to-air missiles (AIM-7, AIM-9, AIM-120, AA-2 to AA-12, MICA, R-530, R-550, PL-7, PL-10), the anti-radar missiles, most air-to-ground missiles, AM-39, most SAMs | 750 ft | 35 |
| AIM-54, AAM-L, AS-14, AS-16 | 1,000 ft | 35 |
| AGM-65A | 650 ft | 30 |
| AT-12 | 200 ft | 35 |
| Harpoon (AGM-84A, AGM-84E) | 1,000 ft | 100 |
| SA-2 | 1,200 ft | 35 |
| Rockets: LAU-10, LAU-61, B-8; B-13 | 250 ft; 350 ft | 35 |
| Mk 82 and Mk 82 HD; Mk 82 P3; Mk 82 HDP6; Mk 82 AIR | 500; 800; 1,000; 1,100 ft | 100 |
| FAB-250, FAB-500, FAB-1000 | 600, 900, 1,200 ft | 100 |
| Mk 84, GBU-10, GBU-27, GBU-30 | 1,000 ft | 100 |
| GBU-29 | 300 ft | 100 |
| GBU-28, GBU-29/P | 100 ft | 35 |
| Cluster bombs: Mk 20, RBK-250, CBU-87, CBU-97 (CBU89), RBK-500 | 1,600, 1,500, 2,200, 2,500, 2,500 ft | 100 |
| KS-12, KS-19 flak | 750 ft | 35 |

Worked examples: an AIM-7M (170 damage to aircraft) bursting at its 100 ft
fuze beside its target does 35 percent, 59 points, to an aircraft whose skin
is at the burst, 21 percent (35 points) to one 300 ft beside the target, and
nothing past 750 ft. A Mk 84 (400 damage) landing 200 ft from a vehicle
(near side 185 ft away) does (100 - 18) x 70 / 100 = 57 percent, 228 points.

Edge cases:

- A record with a radius of 0 or a percent of 0 (guns, the AEMP-1, which has
  a radius but no damage, and the cheat weapons) does no splash.
- A round defeated by a jammer, a round that runs out of life in the air and a
  surface gun round that passes its target do no splash. A flak shell bursts
  at its fuze, its time fuze or the end of its life, with splash.
- Collateral damage never harms a weapon or decoy in flight.

Spec-derived from the original's collateral routine
([format notes](../formats/weapons.md#collateral-damage)): who is reached,
the straight-line falloff to the target's surface, the struck target
excluded, and the ground share. Spec-derived from the manual (p. 126): the
shooter can be caught in its own blast. The missile exception is opinionated,
requested by John on 2026-10-26; bombs retain the manual rule. Fitted by the agent: which
direct hits count as ground bursts (TORE: a hit on a ground object or ship;
the original's split between its two direct-hit calls was not traced); the
splash applied at once (the original delays each target's share by about
2 ticks plus one per 500 feet, which no player can see at these ranges); an
ownship's splash damage taking the usual damage roll, as a direct hit does.
Opinionated (John, 2026-10-10): splash for every weapon and friendly fire
deciding whom it spares. Agent decision: self-inflicted splash credits no
one. The original's routine spares the shooter, and the shooter's side
except one unidentified object type, unless the shooter carries an
unidentified flag; see [unknowns](#splash-unknowns).

### Launch grace

Missiles use the permanent [launcher exclusion](missile-energy.md#motion)
requested by John on 2026-10-26. They cannot acquire, hit or blast-damage their
own firing aircraft, even after separation, on return, or at a ground impact.
This applies to Energy, Spec and Compatibility weapon modes. Another aircraft
in the path or blast remains vulnerable under the usual friendly-fire rule.

Other stores retain John's 2026-10-13 launch grace. A store inside its firing
aircraft's contact sphere (28 ft for an ownship), widened by its fuze radius,
cannot hit that aircraft directly. An airburst inside the aircraft's own
contact sphere spares it. Once outside, an unguided store or bomb can return
and hit its launcher, and its blast outside the sphere can damage it.
Ground bursts are not covered by this grace: a bomb dropped too low or a
rocket fired into the runway can still damage its firing aircraft. Ground,
sea, ground-object, ship and parked-aircraft hits use the ground blast share.
Other drop-launched stores retain the fitted one-second separation grace
after ignition. Guns never hit their own shooter directly.

The combat step applies these rules to the player, AI and online peers.
The contact and blast tests read owner identity and current geometry; they
need no new checkpoint or network state. Arming time (`fuze_arm_t`) remains
separate: an unarmed store does not explode on any target. A bomb that leaves
the sphere and bursts on another target back inside it still gets the airburst
grace, because that rule follows the burst point rather than flight history.

### Splash unknowns

- The original's collateral routine skips the shooter, and objects of the
  shooter's side unless the target is of object type 4, except when the
  shooter carries a flag (object byte 0x10, bit 0x80); neither is
  identified. The manual says a player can be caught in his own blast, so
  the flag may mark the player's aircraft. TORE follows the friendly fire
  setting instead, with permanent missile launcher immunity; bombs and unguided
  stores can still hurt the shooter as the manual describes. Next step: name object type 4 and the flag from the object creator.
- Which of the original's two direct-hit collateral calls (full share, or
  the ground share) serves which kind of target.

## Drop launch, sag and cruise profile

**Spec-derived, 2026-09-28.** John asked for missiles that drop before their
motor lights, and for the lob, to behave as they do in the game. The original's
behaviour comes from three record flags and two record fields; the evidence and
addresses are in [the missile format notes](../formats/missiles.md#release-sag-and-cruise-profile).

- **Ejection.** Air-launched missiles (record flag 0x8) leave with a 32 ft/s
  downward push, added to the inherited aircraft velocity.
- **Sag.** While the motor is unlit, before ignition and after burnout, a
  missile with record flag 0x4 sinks. The sink grows at 32 ft/s² to at most
  80 ft/s and moves the missile straight down, apart from its flight path; the
  nose does not follow it. Lighting the motor stops the sink at once. A missile
  whose motor lights on the rail therefore loses its push immediately.
- **No steering before ignition.** Until the motor lights, fins are locked:
  the missile keeps the launch rail's direction and inherited speed, whatever
  its target does.
- **Cruise profile** (record flag 0x20). While it has a target, the missile aims
  at a point above the target, chosen by its horizontal distance from the target:
  at or beyond `cruise1Dist` it aims `cruise1Alt` above the target; from
  `cruise2Dist` to `cruise1Dist`, `cruise2Alt`; inside `cruise2Dist` it homes
  directly. Distances and altitudes are 256-foot steps. The height is above the
  target (or its predicted intercept), not above the ground or the launch point.

**Fitted, requested by John (2026-10-11):** every missile with the sag
flag holds its actual path, including the downward sink, onto the intercept
point. This includes air-to-air missiles and SAMs. Surface-target profiles and
laser, coordinate and command air-to-ground guidance keep the same correction
introduced in FX-M (fitted, agent decision). Steering subtracts the current sink
from vertical velocity in both live flight and fly-out prediction. The motor,
32 ft/s² sag, 80 ft/s cap, locked fins before ignition and cruise heights are
unchanged. This correction is not a recovered original steering rule. The
[air-to-ground baseline](../baselines/air-to-ground.md#class-8-infrared-air-to-ground-missiles)
records the Maverick reach; the [air missile sag baseline](../baselines/missile-sag.md)
records air-to-air and SAM validation. The range envelope remains
terrain-independent; the terrain-aware fly-out time follows the clearance rule
below. Neither is a promise of a hit.

What a player sees, from the retail records:

| Missile | Motor lights | Drop before it lights | Cruise profile |
| --- | ---: | ---: | --- |
| AIM-54, AA-9, AIM-7, PL-10 | 2 s | about 124 ft | AIM-54 and PL-10: 5,120 ft above the target until 3.3 nmi, then dive in |
| AA-6, AIM-7E | 1 s | about 48 ft | none |
| AAM-L (Su-35) | on the rail | none | 5,120 ft above the target until 3.3 nmi, then dive in |
| AIM-120, AIM-9, MICA, R-550, AA-11, AA-12 | on the rail | none | none |
| AGM-84 (catalog) | 2 s | about 124 ft | 1,024 ft above the target, pop up to 3,072 ft inside 3.3 nmi, direct inside 5,120 ft |
| AM-39 (catalog) | 2 s | about 124 ft | 1,024 ft, 256 ft inside 3.3 nmi, direct inside 5,120 ft |
| AGM-65, AS-7 | 1 s | about 48 ft | 1,024 ft above the target, direct inside 7,424 ft |

AAM-L, the Su-35's long-range anti-AWACS missile, lights its motor on the rail
in the retail record, so it lobs but does not drop. It keeps that retail timing.

How the original steers toward the cruise point is unresolved: it may point at
it or climb to its height. **Fitted, requested by John as the lob:** an
air-to-air missile climbs toward the cruise height over a 2 nmi horizontal
look-ahead, so a long shot pitches up about 23 degrees, levels 5,120 ft above
the target and dives in from 3.3 nmi, about 14 degrees. Surface missiles aim at
the cruise point itself. The same motion drives the HUD reach and intercept
estimates, so they include the drop, sag and climb. The sag's time base follows
the documented 120 Hz service; the original's tick unit is inferred. Whether an
unlit missile loses speed is unknown; it keeps its speed here. The lob gains no
thinner-air benefit, since the fitted motion has no altitude drag law.

### Cruise terrain clearance

**Opinionated, John, 2026-10-11 (surface close-out Q7):** cruise-profile
missiles climb over terrain along their route, then resume their record's
profile. This applies to every record with a cruise profile, including
Maverick, Harpoon, Exocet, SLAM, AS-7, AS-15, laser missiles, AT-2 and airborne
lob profiles. A weapon without a cruise profile keeps its existing motion.

**Fitted, agent decision, 2026-10-11:** each guided 120 Hz step samples the
horizontal line to the remembered intercept for the next 2 nmi (12,152 ft),
or to the target if nearer. Evenly spaced samples are at most 256 ft apart,
including the current position and the far endpoint. Only terrain more than
1 ft above the lower ground elevation at the missile and target endpoints
counts as an intervening obstacle. Flat ground and sea retain the record's
profile, including low sea cruise and the terminal descent. The clearance goal
is 256 ft, with an extra 64 ft allowance for sampling and turning. The combined
320 ft command tapers linearly to zero over the last 5,120 ft to the target.
The steepest line needed to clear those samples overrides the record's aim
only when it requires a steeper climb or a shallower descent. For terrain
above the missile, shorten the climb distance by two seconds of current
horizontal speed, with a 256 ft minimum, to allow time to turn. This is a
steering command, never an altitude snap. Motor, turn limits, ignition and
sag still apply. Once the samples permit it, the record's aim resumes.

Flight and the HUD's terrain-aware time-to-impact fly-out use this same
rule, including record-based laser and command missiles. The launch-zone
scale, maximum range, favorable band and IN RNG remain range and energy
estimates with no terrain penalty, as requested by John. Lock visibility is
separate, described in [air-to-ground](air-to-ground.md#terrain-at-lock-and-launch).
The energy envelope can therefore show an otherwise reachable shot while
MASK or a missing track prevents a lock or launch.

The terrain-aware fly-out samples the terrain on every step to impact, so it
is not run every tick (agent design, review fix 2026-10-12). A guiding cruise
missile works out its time to impact on its first guiding tick and then every
15 ticks (8 a second, staggered by projectile id), counting down in between.
Other guided weapons still work it out every tick. The HUD's launch fly-out
time for a selected cruise weapon is kept for 15 ticks while the station,
target and launch mode stay the same. Both are time cues only; nothing in
flight reads them. In a probe over rolling hills the guiding missile's cost
fell from about 550 to 37 microseconds a tick, with the same time to impact to
a tenth of a millisecond, and the HUD fly-out from about 2.5 to 5.7 ms on every
frame to the same cost eight times a second.

Measured checks are in the [terrain validation](../baselines/surface-terrain.md).

These clearance numbers are not recovered original behaviour or real missile
performance. Sampled look-ahead cannot guarantee avoidance of very narrow
obstacles, a ridge reached before ignition, or terrain exceeding the missile's
turn or energy limits. No new seeker visibility or loss rule is applied to a
missile already guiding. Retail comparison is unavailable.

## What exists today and what changes

At reviewed commit `60f2863`, live combat already applies launch/seeker range
gates, motor phases, coast speed, turn limits and removal. It uses fitted pursuit.
AIM120/MICA guide independently immediately after launch; R530 requires support.
There is no explicit midcourse/activation state, no total guidance timer, and
loss currently clears the target permanently without reacquisition. Onboard
steering gates do not yet use the full shared RCS detection model. The launcher
currently supplies scalar speed and attitude, not its world velocity vector;
guided release requires a designated target. That paragraph records the pre-update baseline. The current implementation
adds vector release, finite boost, seeker-owned observations, activation,
guidance lifetime and reacquisition, plus the mounted seeker, HUD and tone.
See the feature baseline for validation and remaining approximations.

The plan adds the A activation phase, seeker-owned observations, explicit
loss/reacquisition rules, a guidance timer and readable state events. Preserve
the existing weapons profile as an explicit compatibility option when adding
the opinionated activation profile. This is a proposed weapons choice, not a
change to any flight adapter. Update combat recording configuration/fingerprints
so replays cannot silently mix the old and new rules. Record launcher velocity,
launch mode, seeker observations and heat configuration needed for deterministic
playback; old tapes retain explicit compatibility semantics.

## Acceptance contract

Synthetic tests must cover all four types, per-missile target ownership, two
simultaneous shots at different targets, support loss before/after activation,
radar/IR/emitter channel separation and signal shutdown. Test activation just
outside, exactly at and just inside each configured activation distance, converted
with 6,076 feet per nmi. Cover all nine A rows, including thresholds other than
five nmi, to catch a hardcoded five-mile implementation. Activation with no visible
target must remain `ACTIVE SEARCH`. A lost aircraft track must not feed target
updates; moving the hidden target must not move the activation point. S/I/E and
held rows must not receive an active-radar transition from a default value.
Boresight A launches activate immediately even outside their cued threshold;
I/E enable their own passive seekers without emitting radar.

Check exact range/angle edges, ignition and burnout boundaries, guidance expiry,
2-second memory expiry and cleanup separately. Test reacquisition without
resetting guidance age, terrain masking, lower RCS, jammer-only emissions and
emitter filtering. Repeat at 30/60/144 render frames per second with the same
120 Hz simulation, pause/resume and record/replay. Verify missile timers beyond
the old 16-bit clock wrap without reproducing that implementation artifact.

Also validate the synthetic launch/closure examples above against actual motion
and the predictor. Compare faster/slower aircraft, head-on/tail/crossing targets,
climb, side-slip, finite burn, coast and impossible intercepts. Launcher velocity
must be inherited exactly once; target closure must never increase missile speed.

For uncued launch, test no designation, radar/FLIR absent, no target in cone,
contact entry after launch, competing heat sources, circular 5-degree bore edges,
0.25-second dwell, loss/retention thresholds and the 2-second memory boundary.
Check radar/jammer power cannot substitute for IR heat. A below-threshold or terrain-masked contact
must never acquire or produce a lock tone. Test safe/empty/failed/bay gates,
post-shot mounted-seeker reset, mode switching and the two distinct search cones.

HUD captures must show cone geometry aligned with simulated contacts at multiple
aspect ratios and zoom settings, including offscreen/clipped cones. Verify audio
by deterministic state/envelope checks and listening: muted or absent audio must
leave acquisition and HUD behavior unchanged. Include original-style cues,
estimated probability, target-free readouts and multiple shots in the visual pass.

First playable acceptance covers current A2A defaults; S uses R530 and A uses
AIM120/MICA plus the roster's other active candidates. I uses the existing A2A
stores. E uses synthetic explicitly controlled emitter fixtures until ground
weapon support is separately implemented. Keep guns, bombs, compatibility flight
paths and existing loadouts unchanged. No retail comparison is available and no
retail parity claim follows from these checks.

## Implementation decisions

Agent decisions, 2026-09-17: AGM45/AGM88 fixture profiles accept radar
transmissions only; jammer homing requires an explicit profile opt-in. This is a
fitted conservative receiver policy, not recovered band evidence. Wide 0x7fff
angles impose no limit on that axis; other axes use independent spherical angles.
Seeker radar range uses the shared RCS/aspect square-root law with reference 100;
active-seeker notch rejection uses the shared Advanced preset with a 60 ft/s
radial half-width and a 0.45 center range factor at full terrain-relative clutter.
Weapon-specific notch tuning and jammer rejection remain unspecified. The bounded intercept estimate simulates the shared propulsion, limited turns
and fitted maneuver losses at 120 Hz. Lead updates every 0.1 seconds. See
[engagement-dependent range](#engagement-dependent-maximum-range).

## Radar cursor screen boundary

**Opinionated, requested by John on 2026-09-17.** The radar crosshair replaces
the OS pointer across the entire black instrument screen, not just the inset
contact plot. In the 162 by 160 instrument window, this is x in [12, 150) and
y in [20, 134), the screen given by the [bezel geometry](instrument-bezel.md). Lines stop at the screen edges and retain a four-pixel central
gap. The bezel and bottom buttons keep the normal pointer. This applies when
the radar is off or sensor data is unavailable too. Contact projection and
selection tolerance do not change. Scaling uses the displayed instrument bounds.

## Player release advisories and maddog

**Opinionated, requested by John on 2026-10-15.** MAX RANGE and a failed
intercept solution are advice, not player missile trigger inhibits, including
the compatibility weapon adapter. The HUD keeps the range scale, out-of-range
marker and MAX RANGE. SAFE, ammunition, station failure, launcher death,
projectile capacity, cadence and bay delay still apply. Surface target-role,
terrain and emitter restrictions remain separate; lack of A-G support is
also advisory for player radar missiles.

A player may release an A2A missile with no target or usable lock. An unlocked
IR release starts the existing BORE search with no inherited target, within
its five-degree circular search cap and imported seeker envelope. It can
acquire a different aircraft, or fly out without acquisition. Existing flare
seduction remains available after acquisition. SARH launches without support
fly without steering until the launcher illuminates a target; a designated identity
is retained, and an initially targetless shot can take the launcher's later
illuminated designation. It never steers from hidden target coordinates.
This also applies when the radar was off at release. An unsupported player
surface SARH release is recorded as BORESIGHT, retains that recoverable
state, and uses later A-G illumination. It keeps its first acquired identity.
Supported-at-launch CUED surface shots and AI shots retain their separately
specified A-G retarget/loss policy.

An active-radar player missile without supported cueing, including radar off,
no designation, no lock or a target outside the launch acquisition cone,
starts active BORE search immediately (maddog). Its airborne search uses the
imported zone-0 seeker field of view and range, without the cockpit's fitted
five-degree cue cap. This applies by guidance classification, including
AIM-120, AIM-54C, AA-12, MICA-EM and AAM-L. Motor, turning, acquisition dwell
(30 ticks, 0.25 seconds), signature, terrain, notch and lifetime rules remain.
The seeker selects an available return deterministically by signal quality,
then angular centering, range and ID. It has no side/IFF filter: a friendly
can be acquired. Collision and damage still follow the friendly-fire setting. After acquisition, the existing same-target memory
and reacquisition rules apply. No return means continued search until expiry.
These selection details reuse the fitted seeker, not measured real hardware.

The recorded guidance tree already stores BORESIGHT/CUED and ACTIVE SEARCH,
ACQUIRING, PITBULL, MEMORY, LOST and EXPIRED. The replay banner uses that
current recorded seeker state before older guidance-event labels. Active radar
BORESIGHT is labeled MADDOG using the existing tree guidance description. No new
record format or state is needed. Targetless active launches retain Fox three.
AI employment keeps its minimum, maximum, lock and support discipline.

## IR independence and retail-reference readouts

**Opinionated revision requested by John on 2026-09-17.** Armed A2A IR missiles
use the selected cockpit track in CUED mode when one is designated. This overrides
bore candidate selection, including a stronger return in the bore circle. A radar
track supplies the identity to the IR seeker; it does not grant an IR lock or
bypass the weapon's heat, seeker-volume, terrain or launch-envelope checks.
Clearing designation returns the mounted IR seeker to BORESIGHT on the next
120 Hz simulation tick. With no designation, IR bore works even with radar power
off. The mode toggle cannot override an existing designation for IR missiles.
Released missiles retain their own target identity.
Surface missiles retain their separate designation rules. Interpret the requested
"5%" reduction as a five-degree circular half-angle, an agent interpretation.
IR bore candidates must satisfy the imported launch envelope and have a finite
intercept prediction before displaying the blinking inferred diamond. Radar bore
candidates are additionally limited by the selected scope range and the aircraft
radar's imported tracking maximum. These are prelaunch limits, not restrictions
on an already released independent seeker. Missing aircraft radar gives no radar
bore candidates. Radar-off active missiles launch maddog; SARH waits for illumination. No DUMB
label appears in the lower HUD. IR guidance and tones are independent of power.

Use the supplied retail screenshots as presentation references, not proof of
retail probability or timing formulas. ARM sits above count and short weapon
name. The percentage shares its row with IN RNG, blinking at the existing fitted
2 Hz only for an observed, release-ready contact with a positive intercept
estimate. Place the range scale just inside the altitude tape. For radar-guided
weapons with a TWS observation or inferred bore candidate, show R (nmi), C
(signed knots) and A (aspect degrees, L/R from observed motion; -- if unavailable)
below altitude. Debug retains its detailed values. The supplied images establish the desired placement; numeric geometry and blink
timing remain fitted.

## Engagement-dependent maximum range

Implementation revision requested by John on 2026-09-17. The displayed maximum
is an **agent-fitted kinematic estimate**. The imported nominal launch maximum
is not a cap. Active radar shots can be cued outside onboard seeker range and
acquire later; other guidance types retain their seeker range for acquisition, not release.  Use launcher world velocity, rail
attitude and altitude, observed target velocity, and the target's bearing and
relative altitude. Target heading affects heat/signature; target velocity supplies
its approach, crossing and recession geometry. Future maneuvers are unknown.

Predict at 120 Hz using the same motor budget, ignition/burn/coast timing, turn
limits and steering velocity rotation as live flight. Steering uses a bounded
constant-speed lead solution recomputed every 12 ticks; if no positive solution
exists, steer toward the last measured target position. Use exact limited angular
rotation, including opposite headings. The requested nose heading is the unit
vector `nose + desired_flight_path - current_flight_path`; below 1 ft/s use the
desired flight path directly. This fitted correction handles inherited slip or
climb instead of assuming nose direction equals missile velocity. Both live
flight and prediction use it. A fitted turn-energy factor of
`exp(-0.03 * angle_radians^2 / dt)` multiplies missile velocity each turn. There
is no extra maneuver loss in straight flight. This coefficient is an agent choice,
not a measured aerodynamic or retail value. Source coast deceleration remains.

The prediction succeeds when the swept relative path passes within 25 feet of
the constant-velocity target before guidance/removal expiry. This conservative
fitted tolerance is independent of actual target size and fuze radius. For max
range, keep observed bearing and target velocity fixed. Bound the search by
`(launch_speed + motor_gain_budget + target_speed) * lifetime_seconds + 25 feet`,
where lifetime ends at the earlier guidance/removal time. This conservative
travel ceiling cannot be displayed directly as reach. For non-active-radar
profiles, also cap the search at prelaunch seeker maximum. Sample 16 distances
from imported minimum to the search ceiling, then refine the outermost
successful interval with 10 bisections. Show zero if none succeeds. Refresh the
range estimate every 60 simulation ticks or immediately when contact/station
changes. Hit estimate uses that maximum and the predicted time; IN RNG requires
an actual predicted intercept. Imported minimum is advisory; altitude and angle limits remain release constraints.
CUED MAX RANGE is advisory and comes from failure to predict an intercept, not the old nominal
maximum. Actual seeker acquisition/retention volumes remain unchanged. A radar
bore search still needs a prelaunch observation inside the selected scope and
radar tracking limits.

Prediction assumes immediate usable guidance and constant target motion. It
includes the documented drop, sag and cruise profile, but omits future seeker
loss and forces absent from the current missile model. The energy envelope
omits terrain; the HUD time-to-impact fly-out uses the shared
[cruise clearance](#cruise-terrain-clearance). It does not promise a kill. Compatibility
motion stays unchanged. The 25-foot prediction tolerance and turning-loss rule
must be validated against live synthetic trajectories; do not label them retail.

## Dynamic HUD range guidance

Implementation mode, requested by John on 2026-09-17. One simulation-owned
range cue supplies imported minimum, estimated maximum, observed target range
and in-range status to the HUD scale, target marker, radar-ready diamond and
IN RNG label. Air-to-air in-range requires release readiness, a predicted
intercept and a target between the range endpoints. Surface in-range uses the
predicted intercept and range endpoints separately from lock and launch
warnings, as specified in [terrain at lock and launch](air-to-ground.md#terrain-at-lock-and-launch).
TERRAIN MASK remains on the readiness row while IN RNG blinks on its own row. A rounded hit percentage does not determine
physical range validity. A cued target outside range retains its range marker at
the corresponding end of the scale. No target or safe state supplies no range
cue; an impossible maximum displays `--`, not a misleading zero-mile envelope.

### Favorable firing-range bars

John clarified that the ideal firing box is a highlighted band on the vertical
range scale, not a nose-steering cue. He specified two horizontal endpoint bars. Its selection rule is **agent-fitted**:
sample 17 equally spaced distances from `min + 0.10 * (estimated_max - min)` to
estimated maximum. At each distance, retain current observed bearing, velocity,
signal quality and centring. Require minimum/altitude/angle launch geometry, a simulated
intercept, and at least 70 on the existing fitted hit-estimate scale. Highlight
the longest contiguous qualifying interval, omitting isolated samples or no
qualifying interval. This reserves space above minR and below marginal reach.
It is a favorable recommendation, not a calibrated probability or no-escape zone.

Recompute with estimated maximum at 2 Hz and immediately on target/station/mode
change. Draw two six-pixel horizontal bars just inside the range axis at the upper
and lower bounds of that interval, with no connecting outline. Keep the target triangle visible, with bore blinking unchanged. No valid
observation, SAFE, radar-off radar release or impossible prediction produces no
band. The band does not change missile physics or firing permission.

[Weapon/NAV selection and bore-audio availability](weapon-navigation-selection.md)
apply to the player controls and mounted seeker audio.

## Actor-owned launch integration

Reviewed AI missiles use the same seeker activation, acquisition, propulsion,
last-intercept memory and expiry rules as player missiles. Each supported shot
reads only its firing actor's current fire-control observation. Player support
cannot support an AI shot. AI seekers can acquire the player as target ID 0;
no missile seeker may acquire its own launcher. The explicit compatibility
weapon path retains its prior steering apart from this permanent owner exclusion.

The [shared threat service](ai-awareness.md#missile-awareness-and-defense) reads
actual missile state for warning onset. Countermeasures can decoy a spec-guided
missile only while its own enabled seeker has an acquired observation of the
releasing aircraft. An inactive active-radar seeker is not decoyed by a chaff
burst. Successful decoy preserves the physical coasting body and retires its
lock; the body can remain visible.

## Air-to-ground weapons

Specified in [air-to-ground](air-to-ground.md); the infrared, anti-radiation,
active radar and supported radar classes are built (slice W2), the bombs fall
(B1), and the rest is not built yet. The surface
roles above gain player-side designation (A-G radar, FLIR, HARM page, sight,
visual). The inventory of 40 real air-to-ground records in twelve
[weapon classes](air-to-ground.md#weapon-classes) lists each record's seeker,
ranges and carriers. New guidance kinds join the four above: **laser** (rides a
laser spot, falls blindly without it, never reacquires as in retail),
**coordinate** (the JDAMs: retail JDAMs are "optical", home on the target object
and are fire and forget; GPS flight to the point designated at release is John's
opinionated treatment) and **command** (the AT-2 rides the designation).
A HARM gets a boresight cue like air-to-air BORE, and a Maverick may fire in
BORE at the hottest ground object near the bore. The rules for the HUD range
scale of a guided bomb, the HARM cue and the laser box are in
[the HUD section](air-to-ground.md#the-hud-in-air-to-ground-modes).
