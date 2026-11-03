# Quick Mission objective placement audit

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation validation, 2026-10-25. Reference placement logic: `cf36587a`,
protocol 74, with audit-only instrumentation. The fixed run uses `7cf324c0`, before the follow-up placement policy.
The provided isolated import identifies FA.EXE 1.02F, SHA-256
`e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c`.
Only imported data was read. Retail comparison remains unavailable.

## Coverage and reproduction

Both surface runs generated **65,472 cases**: the 108 offered non-empty templates
plus a no-target area-defense selection in each of 16 theaters, all 16 AAA/SAM
strength pairs, all 11 separation choices and seeds 1, 2 and 3. Checks use the
completed scene, including airport replacement and foundations. The
[development command](../DEVELOPMENT.md#quick-mission-objective-placement-audit)
describes the invariants, tolerances, auxiliary checks and limitations.

The command generates every placement. Completed-scene geometry checks are
reused only for equal complete placement inputs after removing separation and
start metadata. Starts are calculated for the current distance. The 264-case
Falklands/Ukraine battery subset compares every finding, case and digest against
`--uncached`, which directly rebuilds every scene. Both modes matched.

The reference inspected 8,047 distinct scene inputs and reused 57,425; the fixed
run inspected 8,083 and reused 57,389. Raw placement repeats check determinism for
every strength pair, plus full scene repeats for every template/seed at zero
defenses. No nondeterministic result was reported.

For the large fixed run, only the data-reader crate was optimized:

```sh
cargo build --workspace --locked --config 'profile.dev.package.tore-formats.opt-level=2'
TORE_DATA_DIR=.local/dev-profile python3 tools/objectives_audit.py --output .local/objective-audit --seeds 3 --supplemental
```

This is a build-time performance setting, not a flight adapter change. Its
264-case output matched the ordinary development build exactly. The comparison
probe took 6.5 seconds with optimized readers, compared with about 15 seconds
without them. No dependency or build-profile change is committed.

## Surface findings

Rows below count checks across the matrix. The same object appears at multiple
distances and strengths. Footprint intersections include intentional airport
surfaces and hollow structures; coarse terrain classes can disagree with the
visible shoreline. These counts are not counts of distinct physical defects.

| Finding | Before | After |
| --- | ---: | ---: |
| Stale objective center after airport replacement | 5,808 | 0 |
| Defender start beyond 5.02 nm from the final objective | 352 | 0 |
| Unit footprint intersections | 36,124 | 35,288 |
| Scene-object footprint intersections | 473,913 | 473,913 |
| Ship center classified as land | 47,784 | 47,454 |
| Footprint terrain-class mismatch | 47,784 | 47,454 |
| Coarse cell slope exceeds 256 ft | 65,626 | 65,626 |
| Missing template/layout element | 10,010 | 9,457 |
| Fewer area units than rolled | 407 | 407 |
| Requested start separation shortened | 3,270 | 3,286 |
| Objective distance outside requested separation plus/minus 5.02 nm | 3,149 | 3,053 |
| Authored route segment crosses a different terrain class | 1,584 | 1,584 |
| Grounding geometry unavailable | 3,168 | 3,168 |
| Explicit unresolved support-placement reports | 0 | 90,596 |

Surface checking produced 698,979 rows before and 781,286 after.
The consolidated files also include the separate aircraft-start findings. The new explicit
support reports account for the larger after total. The strict contract count
fell from 30,844 to 25,036; every remaining strict failure is an added support
unit on a cell exceeding the coarse slope limit. This is only 27.6% of the
90,596 unresolved-support rows. The remaining rows include runway and object
clearance failures that the original strict counter did not count separately.
Do not interpret all unresolved support as steep terrain. Both full commands return
nonzero because those failures remain.

## Corrections and limits

- Final target positions now determine starts after airport replacement. In the
  Falklands airfield example, the old objective was 2,633 ft from the final target
  center. That initial change ranked final runway centers; the follow-up restores integer
  origin ranking to remove rendering trigonometry from the digest.
- Jitter reserves neighboring mobile units' fallback and accepted positions.
  Added trucks and radars avoid other units and search up to 128 candidates.
  Exhaustion is explicit; requested equipment is retained pending a fallback
  policy decision. No valid placement is claimed for that fallback.
- Ships must keep their footprints inside the map. Relocation checks full
  footprints, and carriers with deck aircraft remain water objects during group
  relocation. Fleet moves try 128 candidates within the existing distance and
  rotation limits.
- The four Kiev fleet aircraft still lack a supported deck. The Kurils can lack
  enough valid land for rolled area sites. Existing long-distance shortening is
  retained. These are recorded findings, not newly invented placements.
- The inspected Cuban submarines are visibly in water despite coarse-grid land
  classifications. Invisible `A_M1939.NT` barrage controllers account for the
  unavailable grounding geometry. The Vietnam cargo route crosses coarse land
  cells as already described in the [movement specification](../spec/surface-defenses.md#movement).
- The actual lane/waypoint plans for all three offered routed templates were
  compared before and after with three seeds and one surface-only tick. All nine
  pairs matched, and all 141 listed starts/waypoints per version were inside the
  documented map rectangles. Painted-road alignment, turning trajectories and
  live collision avoidance are not certified by this check.

## Verification and compatibility

Formatting, workspace Clippy with warnings denied, Rust tests (7,162 passed, 106 ignored), Python checks (505 run, five skipped), the workspace build,
documentation and all three asset guards passed. All 52 focused layout tests
passed. Eight selected surface battery scenarios passed; the full battery was
not run. The new detector scenario intentionally expects the 242 known
Falklands support-slope violations and requires explicit unresolved reports.

The wrapped display smoke test presented the requested screen successfully.
Twelve offscreen scene/map captures succeeded through `tools/agent-run.sh`.
Six contact sheets and all raw tables remain in the task scratch directory;
no retail-derived images or raw data are committed. The Baltic bridge deck
appears absent in both versions' captures and is left to the bridge audit.
The existing creator snapshot path ignores `--separation`; its captured map uses
its default 5 nm, so it is visual evidence only. Quantitative distance checks
come from the mission builder.

All 4,092 full mission-start cases completed in both versions. No aircraft-center
map-bound or terrain-clearance failure was reported. CAP-region edge findings
changed from 11,236 to 11,241; actual-separation findings from 264 to 265. Ground
layout refusals remained 561. All auxiliary grounding and airport audits passed
in both versions.

These changes alter seeded positions, starts and surface digests, including
multiplayer and replay reconstruction. Digests changed in 22,737 of 65,472 cases.
All 585 unit-count changes are explained by front-line availability (569 gained,
16 lost); parked-aircraft counts were unchanged. No wire or replay format, protocol
version, default flight adapter or compatibility mode was changed. The lead
must coordinate any version bump with the other branches. Grounding remains
exclusively in `tore_world::grounding`; that module was not changed.
