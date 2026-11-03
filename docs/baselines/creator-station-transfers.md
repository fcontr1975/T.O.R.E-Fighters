# Creator station transfer validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation validation, 2026-10-26, on `su35-validator` from `fd430b9c`
(`0.2.2-rc1` with the paired-stores merge). The gameplay contract is
[even paired loads](../spec/stores-on-pylons.md#even-paired-loads).
This corrects a validator assumption; no gameplay or simulation rule changes.

## Reproduced failure

The imported creator loadout sweep stopped at the Su-35 with
`station transfer did not conserve imported ammunition`. Its first external
weapon is `AAML.JT` on centreline source row 5, with one store. Row 6
is also configured with `AAML.JT`, with two stores on a fuselage pair
([source rows](../formats/aircraft-ordnance.md)). Catalog refill leaves row 5
at one; the probe deliberately clears row 6 from two to zero before taking
the conservation snapshot and dragging. Rows below are
zero-based source ordinals, not display order.

| Aircraft/store | Source row | Destination row | Before drag | After drag | Result |
| --- | --- | --- | --- | --- | --- |
| SU35.PT / AAML.JT | 5, single-sided, 1 store | 6, paired, capacity 2 | [1, 0] | [1, 0] | Correct refusal: the source cannot supply a pair |

The validator combined two assertions: the destination must become nonempty,
and the sum of source and destination counts must stay constant. The sum
stayed at one. Only the nonempty requirement failed. Moving the single would
violate the current spec; expanding it to two during a drag would create a
store. The saved-single migration rule does not apply to a station transfer.

## Conservation checked

The checker snapshots the load after catalog refill and destination clearing.
Tank loads are cleared before this isolated weapon-transfer sweep so deliberate
tank replacement cannot be mistaken for lost mass. For each compatible
same-page destination of the first external weapon row:

- A paired endpoint requires exactly two stores to move. If source quantity or
  destination capacity is below two, every saved-load field must remain equal.
- Other transfers move the existing quantity step, limited by supply and
  capacity: one through capacity 100, ten through 300, and 100 above 300.
- A successful transfer subtracts exactly the amount it adds, preserves the
  moved weapon's identity, every unrelated station and the source layout
  maximum, and preserves fuel, tanks, Cheat state and total weight. Only the
  destination's weapon, layout maximum and quantity may change along with the
  source quantity.

Catalog refill, replacement of occupied stations and old-save migration are
separate operations, not conservation claims of this probe. Cross-page drags
are counted as skipped because both cards cannot receive the gesture together.
See [the creator probe](../testing/lane-menus.md#the-creator-probe) for operation.

## Roster results

All 37 selectable identities passed the creator's loadout checks. The expanded
same-page drag sweep checked 48 transfers on 24 aircraft: 45 successful moves
and three correct refusals. No cross-page case was skipped in this run.

| Aircraft/store | Source row and count | Empty destination row and capacity | After drag | Reason for refusal |
| --- | --- | --- | --- | --- |
| MIG21.PT / AA2.JT | Row 5, paired, 2 | Row 4, single, 1 | [2, 0] | Destination cannot accept two |
| SU25.PT / B13.JT | Row 6, paired, 2 | Row 5, single, 1 | [2, 0] | Destination cannot accept two |
| SU35.PT / AAML.JT | Row 5, single, 1 | Row 6, paired, 2 | [1, 0] | Source cannot supply two |

The MiG-21 and Su-25 cases are additional destinations the old first-target-only
check did not reach. Successful examples include F18.PT AIM120.JT row 5 to
rows 6 and 7, each `[2, 0]` to `[0, 2]`, and RAFALE.PT AGM65G.JT row 5 to
rows 6 and 7, each `[4, 0]` to `[2, 2]`. Identities remain F/A-18D and Rafale C.

AH64.PT, MIG17F.PT and F4J.PT exercised catalog removal/refill but had no
compatible second row for their first external weapon. Ten identities had
no applicable external weapon drag: C130.PT, AC130.PT, E3.PT, IL76.PT,
E2.PT, V22.PT, MI24.PT, CH47.PT, B747.PT and A310.PT. Their other creator
loadout checks passed. This is a sweep of the validator's selected source
weapon per aircraft, not an exhaustive drag of every weapon on every row.

## Validation

On Linux with Rust 1.91.1 and locked dependencies, heavy checks ran sequentially
at nice 15 with four build workers and four test threads. The battery used two
jobs and one window slot, with an isolated imported profile.

- Both new synthetic transfer tests passed. Six UI cases cover successful moves
  and both directions of pair refusal. Negative cases reject lost or duplicated
  stores, unexpected refusal, changed identity, unrelated ammunition changes,
  changed fuel and a split pair.
- App tests: 2,346 passed, 49 ignored. World tests: 1,040 passed, three ignored.
- Workspace clippy with warnings denied, app build, formatting, source/app asset
  checks and documentation checks passed.
- Python tools: 516 run, five skipped. All 40 battery selection tests passed.
- Focused battery: 40/40 passed. Includes `menus-validate-creator`, all
  `menus-loadout-*`, `menus-ordnance-*`, `menus-tanks-*`, `replay-stores-*`,
  `menus-derived-stations` and `qmc-wing-loadout`.
- The full creator validator started 10,028 setups, refused one with its normal
  runway message and reported zero problems. It rendered 637 pictures and
  checked 147,000 creator input events plus 76,000 screen input events, all
  with zero problems. The creator scenario took 1,088.8 seconds under this
  throttle; the complete focused battery took 1,165 seconds.

## Evidence and limits

The failing run used the task's isolated imported profile and a diagnostic-only
change to the error message. The reproduction log is
`creator-before.log` in the main checkout's local scratch directory
`.local/next-round/0.2.2-rc1/su35-validator/`. The passing creator log is
`battery/20261026-151010-su35-transfer/logs/menus-validate-creator.log`;
that run's `summary.md` lists all 40 scenarios. `report.md` names the other
check logs and the local commit. No retail bytes are committed.
Retail execution and retail comparison remain unavailable; this validates the
existing opinionated pair rule, not original-game parity.
