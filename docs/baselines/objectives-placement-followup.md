# Quick Mission placement follow-up validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation validation, 2026-10-26. Reference: `45ab9914`, which merges
`0.2.2-rc1` at `fd430b9c` and retains reviewer documentation commit `42dd0a9f`.
This pass follows the review of `b83937cc`: loosen launcher/gun/radar grades,
restore ground-target runway-start distance choices, retry refused host seeds,
and avoid repeating terrain checks for changes such as wing skill.

Since 2026-10-28 the creator no longer runs the placement audit while the
player edits: it reads a fixed table measured offline with the same solver
([distance table baseline](qmc-distance-table.md)). The audit results below
are unchanged evidence.

The current behavior lives in the [creator specification](../spec/quick-mission-menu.md#keeping-the-enemy-on-the-map),
[vehicle footing policy](../spec/surface-defenses.md#jitter), and
[server seed policy](../DEDICATED-SERVER.md#surface-placement-seeds).
The 20% launcher/gun/added-radar limit is fitted, chosen to match other vehicles
under John's 2026-10-26 instruction to loosen the previous 10% limit. Tank limits,
map/water/clearance checks, supply reach and shared grounding are retained.

## Coverage

Fresh full before/after audits use the same isolated FA 1.02F import: sixteen
theaters, every offered target and no-target area defenses, sixteen AAA/SAM pairs,
eleven separations and three seeds, totaling 65,472 surface cases per run.
The separate start audit attempts 4,092 worlds with thirty aircraft each, counting
refusals separately. Grounding, airfield, parked-aircraft and merged-field checks
complete the pass. The [audit command](../DEVELOPMENT.md#quick-mission-objective-placement-audit)
describes the checks and geometry-cache validation. Retail gameplay comparison
is unavailable and no retail parity claim is made.

Local evidence is in the main checkout's
`.local/next-round/0.2.2-rc1/objectives-audit/`: `round2-before/`, `round2-after/`,
`round2-binaries.sha256` and `report.md`. The previous report and baseline are
preserved there. Binary copies are immutable. Heavy runs use priority 15, four
Cargo workers, four test threads, two battery workers and one wrapped window,
with at least 8 GB available before each heavy run. Runs are sequential.

## Audit results

| Measure | Before | After |
| --- | ---: | ---: |
| Surface cases | 65,472 | 65,472 |
| Surface finding rows | 464,794 | 466,381 |
| Strict contract findings | 44 | 0 |
| All vehicle footprint-slope findings | 154 | 0 |
| Unresolved vehicle rows | 154 | 0 |
| Complete placement-refused case gate | Not recorded | 0 |
| Full-world setups attempted | 4,092 | 4,092 |
| Worlds built | 3,796 | 3,826 |
| Placement refusals in full-world starts | 30 | 0 |
| Distance refusals in full-world starts | 266 | 266 |
| Start/CAP findings | 0 | 0 |

The full after matrix has no placement-refused cases, including authored vehicles
and retained front groups. Distance refusals remain separate: the requested
distance must fit for an airborne launch. The before inspector did not emit the
new complete per-case placement-refusal marker, so that before cell is unmeasured,
not zero. Its individual footing warnings and full-world refusals are recorded.

The total review-row count rises by 1,587. The retained coarse 256-foot cell-slope
check adds 1,937 rows as the relaxed footprint rule permits steeper positions;
154 per-footprint slope rows, 154 unresolved-unit rows and 42 missing-front rows
disappear. The coarse-cell measure is not the new per-unit grade limit. Vehicle
runway and object-clearance review counts remain unchanged.

Scene digests changed in 2,475 cases; unit counts changed in
8, parked-aircraft counts in 0.
All eight count changes concern optional Kuril QKSILO front groups: six cases
add eight front units and two cases lose a six-unit optional front. The latter
are seed 2, AAA none, SAM moderate/heavy, at 5 nm; the audit explicitly reports
that no site holds the two lines. The repair pass does not delete resolved
vehicles or support equipment. The complete case table records each change.
All supplemental commands completed with zero contract problems; their retained nonfatal observations remain in the logs.

Finding rows repeat objects across settings. Coarse shoreline classifications
and assembly intersections remain review findings. They do not justify moving
an authored object without rendered-water or solid-geometry evidence. The Spain
shelter-entrance radar and Vietnam painted-channel boat remain reported limitations
from the prior visual review; this pass does not alter that content.

## Regression checks

Workspace: 7,227 Rust tests passed, 107 ignored, zero
failures. Formatting, all-target clippy with warnings denied and the locked
workspace build passed. Python ran 526 tests with five skipped and no
failures. Source/app/extract asset guards and documentation checks passed.

The reviewer's exact 77-scenario selection passed 76/77 after four
map fixtures were corrected and rerun. Those fixtures had requested airport 9
(Odesa) while expecting L'viv, relying on the old target-row index shift. They
now request L'viv's actual ID 8 and keep the same named-airport assertions. The
additional ground regression also verifies that airport 9 stays Odesa. All five
corrected/extended scenarios passed. Both battle surveys now pass. The additional ground-target runway-start
snapshot and wrapped 300 nm launch passed. The known creator-validator result is
the inherited Su-35 ammunition-transfer failure.
The final ground-start image was inspected: Ground is selected and all eleven
choices, including 300 nm, are enabled.

Synthetic host tests exercise deterministic retries, the sixteen-attempt bound,
rollback on exhaustion, immediate propagation of unrelated errors, fresh resource
manifests, seed-0 startup, King changes, restart, rotation, preview validation and
client agreement with the accepted seed. The client does not retry locally.
Ground-start tests compare all eleven target-based choices with actual builds;
the snapshot battery checks the selected runway mode and a 300 nm launch. A
ready-cache test changes wing skills and orders without starting another worker.

The reviewer suite contains the known independent Su-35 ammunition-transfer
failure in `menus-validate-creator`. Its status is reported explicitly, rather
than weakening that check or claiming its later creator matrix ran.

## Integration

This changes surface placements and digests through the relaxed fitted grade
limit. Host fallback changes the accepted surface seed only after a placement
refusal; the existing mission text carries that seed to clients. No new wire
field, protocol bump or replay version bump is added here. The release lead
coordinates compatibility with the other merged work. Flight-adapter defaults
and aircraft identities are unchanged. Linux CPU validation is reported above;
Windows, macOS and retail gameplay comparison were not run.
