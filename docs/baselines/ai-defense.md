# AI missile defense and RWR validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. The original feature pass below covers the [defense contract](../spec/ai-awareness.md#missile-awareness-and-defense)
and [RWR presentation](../spec/rwr.md). Its source base was `5b2241f`. Committed
fixtures are synthetic; imported media and rendered derivatives remain local.

## Current countermeasure timing validation

The ordnance-fixes implementation pass starts at `ba1bf7e4`. All four skills
retain their reaction delay and device budget, but pair starts are at least
240 simulation ticks apart. The second cartridge remains 30 ticks later.
A synthetic all-skill regression checks three pairs and the existing reserve,
handoff and encounter tests check the same lower bound across transitions.
The focused simulation suite passed 2,179 tests, with eight existing ignored.

The following isolated imported flights use one case, seed 0, the short range
at co-altitude and the real AI controller. Elapsed time is sampled after a step:
the first `0.008` entry is tick 94 for the Ace AIM9M head shot. Its later pair
starts are ticks 334 and 574, exactly 240 apart, and the second devices are
at 124 and 364. All after traces with complete pairs have the same 30-tick
internal spacing and at least 240 ticks between starts. A shot can end before
the queued second device releases. These are release times, not warning times;
earlier skilled reactions remain independently covered by the unit tests.

| Skill | Store / aspect | Before release seconds | After release seconds |
| --- | --- | --- | --- |
| novice | AIM9M / head | 2.008, 2.258 | 2.008, 2.258 |
| novice | AIM9M / tail | 6.592, 6.842 | 6.592, 6.842 |
| novice | AIM120 / head | 1.733, 1.983, 3.983, 4.233 | 1.733, 1.983, 3.983, 4.233 |
| novice | AIM120 / tail | 3.042, 3.292, 9.042, 9.292, 11.233, 11.483 | 3.042, 3.292, 9.042, 9.292, 11.233, 11.483 |
| average | AIM9M / head | 1.208, 1.458, 2.708, 2.958 | 1.208, 1.458, 3.208, 3.458 |
| average | AIM9M / tail | 5.633, 5.883, 7.133, 7.383 | 5.633, 5.883, 7.633, 7.883 |
| average | AIM120 / head | 0.983, 1.233, 2.983, 3.233, 4.483, 4.733 | 0.983, 1.233, 2.983, 3.233, 4.983, 5.233 |
| average | AIM120 / tail | 3.058, 3.308, 7.058, 7.308, 10.233, 10.483 | 3.058, 3.308, 7.058, 7.308, 10.233, 10.483 |
| experienced | AIM9M / head | 0.25, 0.5, 1.25, 1.5, 2.25, 2.5 | 0.25, 0.5, 2.25, 2.5 |
| experienced | AIM9M / tail | 4.425, 4.675, 5.425, 5.675, 6.425, 6.675 | 4.425, 4.675, 6.425, 6.675 |
| experienced | AIM120 / head | 0.6, 0.85, 1.983, 2.233, 2.983, 3.233 | 0.6, 0.85, 2.6, 2.85, 4.6, 4.85 |
| experienced | AIM120 / tail | 3.033, 3.283, 6.033, 6.283, 8.742, 8.992 | 3.033, 3.283, 6.033, 6.283, 8.742, 8.992 |
| ace | AIM9M / head | 0.008, 0.25, 0.75, 1, 1.5, 1.75 | 0.008, 0.25, 2, 2.25, 4 |
| ace | AIM9M / tail | 3.992, 4.242, 4.742, 4.992, 5.492, 5.742 | 3.992, 4.242, 5.992, 6.242, 7.992, 8.242 |
| ace | AIM120 / head | 0.408, 0.658, 1.483, 1.733, 2.233, 2.483 | 0.408, 0.658, 2.408, 2.658, 4.408, 4.658 |
| ace | AIM120 / tail | 3, 3.25, 6, 6.25, 7.983, 8.233 | 3, 3.25, 6, 6.25, 8, 8.25 |

The matched countermeasure pass has 161,408 trials per build: scripted radar
64-seed matrices and IR controls, plus six stores at four AI skills with 32
seeds per cell. The [ordnance baseline](missile-energy.md#countermeasure-comparison)
contains hit, decoy and decoyed-then-hit rates. This tests the remake's fitted
rules, not retail parity or real-world performance.

## Behavior covered

Guidance-dependent warnings, frozen threat snapshots, visual-only passive
missile detection, own-ordnance plots, skill-dependent defensive commitment,
jink/notch/dive, bounded countermeasure bursts, actor-owned radar support and
actual active-seeker acquisition are connected to the live mission and RWR.
The explicit compatibility weapon mode retains its prior steering and B47 AI
warning route.

## Original feature validation

The original feature pass was validated on Linux with Rust 1.91.1 and locked dependencies.

| Check | Result |
| --- | --- |
| Formatting and workspace Clippy with warnings denied | Passed |
| `cargo test --workspace --locked` | 1,077 passed; 3 existing ignored tests |
| `cargo build --workspace --locked` | Passed |
| Python tool tests | 68 passed |
| Source and both executable asset scans | Passed |
| Documentation headers and diff whitespace | Passed |
| Imported roster probe, 3,600 ticks per aircraft/skill case | All 52 cases passed, zero dropped launches |
| Display smoke test | Passed on NVIDIA GeForce RTX 4070 using Vulkan |

Focused cases verify silent active midcourse, actual pitbull, immediate S
warnings, visual-only I/E acquisition, failed receivers, full 240-tick loss
grace, unknown/receding-threat fallback, Novice versus Ace timing, hold-fire
self-preservation, two-device debit at ticks 0 and 30, mixed visual bursts,
owner-specific radar support and no pre-pitbull chaff decoy. The existing
compatibility and deterministic replay tests also pass.

## Visual evidence and limits

The manual's printed pages 94, 95 and 131 were rendered and inspected. Runtime
capture with original art/font assets uses:

```sh
target/debug/tore-app --live-fire --aircraft f18 --weapon-slot 2 --combat-command target-distance:100000 --combat-command target-radar --combat-probe-ticks 720 --instrument-page 5 --instrument-layout small --capture-flight .local/ai-defense/rwr-emitter-missile-separated.ppm --no-audio --smoke-test
```

The inspected image shows the original frame/font, 50 NM range label, R
indicator, filled enemy emitter diamond and a distinct steady own-missile dot.
The close incoming CLI fixture overlays its missile on the center aircraft
mark after the probe setup, so those captures do not independently establish
both flash phases. Exact flash boundaries and symbol shapes have synthetic
raster tests. No running retail comparison or human encounter-balance review
was performed.

Flight response estimates, visual CPA classification, stale grace, own-ordnance
telemetry and active-seeker notch response are authored/fitted as specified.
Cloud/night visual limits and weapon-specific notch calibration remain open.
Passive unknown guidance does not acquire an invented IR label or decoy
susceptibility. Prelaunch painting/tracking emitter states have drawing support
but no fabricated sensor producer. Player audible-warning changes are out of
scope. Mission engagement policies are the next stage.
