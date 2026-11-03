# Missile energy calibration and validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode, 2026-10-27. Model calibration, not retail
parity or a validated engineering flight envelope. Semantic model identity,
branch `ordnance-fixes` from `ba1bf7e4`, Linux x86-64. Input came from the isolated imported
profile for this task; no media bytes are committed. The shared offline probe
is `cargo run --locked --release -p tore-sim --example missile_energy_ranges
-- LOCAL_JT_DIRECTORY`. `--fit-class` exports fixed-budget two-condition candidates; the expanded
`--fit-motors` and `--fit-staged` searches also fit per-store motor values.
`--verify` checks explicit reference bands, role/family ordering, tail floors,
motor shape, powered cruise arrival, finite shots and local parameter
sensitivity. Neither changes source or runs during gameplay.

This baseline contains the current ordnance-fix motor, range-advice and
countermeasure pass. Unchanged stores retain their Phase 2.8 fits. Same-tick
inert threat removal is unchanged.
Class budgets and post-burn defeat are fixed inputs to the reach search, not
optimizer parameters. Defeat therefore affects every candidate's measured reach. The earlier Phase 3
study is invalidated, remains labelled pre-fix in local scratch, and has not
been resumed. This pass is calibration and regression validation only.

## Conditions and limits

A2A launcher velocity is 300 m/s, target 250 m/s, wings level, no wind or
jammer, flat sea-level ground. Altitudes are MSL. Head, beam and tail mean
180, 90 and 0 degrees target heading relative to launch heading. All distances
below are km of initial separation, not missile path length. Numbers are
rounded to 0.01 km. The sweep samples ranges by factor 1.5 from 100 m to
1000 km and adds the shared anchor/loft-transition probes, then bisects the last
successful interval ten times. Empty envelopes retry with factor-1.1 probes,
which recovered narrow AA8 beam intervals. It can still miss a
narrow disconnected island of success. It is not a mathematical proof of a
continuous no-escape envelope. Fitting and regression diagnostics
use twelve bisections instead of ten, so their last reported decimals can
differ slightly from the complete sweep without a physics change. Acceptance uses the verifier precision, not rounded display values. Retained
AA11/AA11B H35 is 26.851 km at verifier precision, inside the 26.850 km lower
gate; the coarser display sweep reports 26.846 km.

Physical reach uses the same 120 Hz motor, atmosphere, lift, drag, PN, loft,
cruise path and swept fuze kernel as live motion, with continuous ideal target
observations. Seeker-valid reach calls the live actor-owned guidance routine
each tick, including slew/FOV, memory, activation and terrain-Doppler rejection.
IR requires a valid mounted return at launch; target heat is military and both
signature fields are the explicit normalized 100 reference. Radar has ideal
continuous launcher support, so this column does not include a particular
aircraft radar's acquisition range or scan delay. There is no synthetic
percentage reduction from the physical result. N/A means the game profile is
held or uses the separate laser/command channel, not zero kinetic reach.
The ideal-observation trajectory is a reference, not an upper bound over all
possible guidance histories. Seeker memory and activation can change steering
and energy expenditure, so some seeker-valid maxima exceed that reference.
Together with the sampled search, this limits how literally either maximum
should be read as a continuous launch boundary.

Public facts and the deliberately fitted anchor conditions are separated in
[the data evidence](../formats/missile-energy-data.md#per-store-motors-and-class-control-budgets).
All conditional anchors are agent choices unless explicitly sourced. The
M/X and R73 high-head anchors have been restored to their earlier lower values. Physical head-on reach does not give a rear-only seeker
a head-on lock. Held records remain unavailable in gameplay.

## Complete A2A physical reach

Each cell is head / beam / tail, km. All 25 inventory records are included,
including held records measured diagnostically without enabling them. These
are the farthest sampled hits, not the advised HUD region. Gaps must be
read from the survey intervals; the range-advice check below covers their display.

| Store | Primary reference km | 500 ft | 5,000 ft | 15,000 ft | 35,000 ft |
| --- | ---: | --- | --- | --- | --- |
| AA10.JT | 73 | 23.39 / 10.43 / 5.72 | 25.77 / 12.60 / 7.18 | 33.08 / 17.88 / 11.17 | 64.73 / 37.30 / 25.11 |
| AA11.JT | 30 | 13.37 / 9.24 / 6.88 | 15.04 / 10.39 / 7.79 | 20.05 / 13.78 / 10.47 | 26.85 / 21.07 / 16.85 |
| AA11B.JT | 30 | 13.37 / 9.24 / 6.88 | 15.04 / 10.39 / 7.79 | 20.05 / 13.78 / 10.47 | 26.85 / 21.07 / 16.85 |
| AA12.JT | 50 | 24.56 / 13.54 / 8.80 | 29.38 / 15.76 / 10.31 | 45.84 / 21.74 / 14.32 | 62.16 / 38.08 / 27.12 |
| AA2.JT | 20 | 6.13 / 3.27 / 2.18 | 7.06 / 3.74 / 2.51 | 9.85 / 5.14 / 3.50 | 16.69 / 9.94 / 6.69 |
| AA6.JT | 30 | 11.76 / 7.68 / 5.46 | 13.12 / 8.61 / 6.18 | 17.16 / 11.30 / 8.26 | 32.84 / 21.45 / 16.13 |
| AA8.JT | 10 | 14.11 / 7.12 / 4.10 | 14.34 / 7.41 / 4.33 | 14.80 / 7.95 / 4.79 | 15.46 / 8.56 / 5.46 |
| AA9.JT | 120 | 12.25 / 6.36 / 2.54 | 42.42 / 14.00 / 6.82 | 51.53 / 22.25 / 11.74 | 120.03 / 51.01 / 30.09 |
| AAML.JT | 150 | 38.72 / 21.55 / 10.48 | 43.85 / 25.18 / 15.64 | 58.33 / 34.54 / 22.53 | 150.00 / 69.07 / 46.08 |
| AEMP1.JT | 50 | 24.55 / 13.49 / 8.79 | 29.36 / 15.71 / 10.30 | 45.83 / 21.71 / 14.30 | 62.15 / 38.06 / 27.11 |
| AIM120.JT | 60 | 32.65 / 16.51 / 10.31 | 40.14 / 19.17 / 12.06 | 52.69 / 27.05 / 17.19 | 60.00 / 36.48 / 25.75 |
| AIM54C.JT | 150 | 12.41 / 6.28 / 2.39 | 52.09 / 21.90 / 11.00 | 65.71 / 32.39 / 18.16 | 150.00 / 70.82 / 43.00 |
| AIM7.JT | 70 | 16.11 / 9.10 / 5.88 | 18.19 / 10.45 / 6.89 | 24.27 / 14.23 / 9.71 | 58.58 / 27.82 / 19.63 |
| AIM7E.JT | 35 | 12.86 / 8.17 / 5.73 | 14.44 / 9.21 / 6.53 | 19.04 / 12.15 / 8.77 | 36.77 / 23.17 / 17.18 |
| AIM9B.JT | 20 | 8.94 / 4.74 / 3.16 | 10.36 / 5.45 / 3.67 | 14.65 / 7.54 / 5.20 | 17.99 / 11.49 / 7.99 |
| AIM9M.JT | 18 | 15.53 / 8.80 / 5.52 | 15.95 / 9.32 / 5.95 | 16.80 / 10.29 / 6.79 | 18.01 / 11.50 / 8.01 |
| AIM9X.JT | 18 | 15.53 / 8.82 / 5.52 | 15.96 / 9.34 / 5.95 | 16.81 / 10.31 / 6.80 | 18.03 / 11.51 / 8.03 |
| MICA.JT | 60 | 23.61 / 11.32 / 6.29 | 26.51 / 13.12 / 7.53 | 37.37 / 18.52 / 11.34 | 60.00 / 35.36 / 23.69 |
| PL10.JT | 50 | 29.31 / 6.63 / 3.05 | 35.62 / 14.57 / 8.70 | 46.43 / 20.61 / 12.24 | 50.09 / 26.01 / 17.34 |
| PL7.JT | 14 | 14.63 / 7.84 / 4.63 | 14.82 / 8.06 / 4.82 | 15.18 / 8.44 / 5.19 | 15.69 / 8.78 / 5.69 |
| R27ER.JT | 117 | 48.93 / 15.05 / 7.38 | 49.98 / 16.62 / 8.40 | 56.68 / 24.23 / 13.35 | 117.00 / 57.57 / 37.54 |
| R27ET.JT | 104 | 31.64 / 17.08 / 10.97 | 36.79 / 19.81 / 12.73 | 51.91 / 27.78 / 17.87 | 104.13 / 55.45 / 35.85 |
| R27T.JT | 63 | 19.03 / 11.63 / 8.17 | 21.97 / 13.36 / 9.41 | 30.78 / 18.52 / 13.11 | 61.52 / 38.11 / 26.68 |
| R530.JT | 40 | 13.13 / 8.82 / 6.35 | 14.70 / 9.90 / 7.21 | 19.44 / 12.86 / 9.54 | 43.92 / 25.64 / 18.78 |
| R550.JT | 15 | 14.86 / 8.08 / 4.86 | 15.08 / 8.34 / 5.08 | 15.51 / 8.81 / 5.51 | 16.11 / 9.32 / 6.11 |

## Complete A2A seeker-valid reach

| Store | 500 ft | 5,000 ft | 15,000 ft | 35,000 ft |
| --- | --- | --- | --- | --- |
| AA10.JT | 23.39 / 10.43 / 5.72 | 25.77 / 12.60 / 7.18 | 33.08 / 17.88 / 11.17 | 64.54 / 37.29 / 25.11 |
| AA11.JT | 11.56 / 9.22 / 6.88 | 11.56 / 10.38 / 7.79 | 11.56 / 13.77 / 10.47 | 11.56 / 15.30 / 16.85 |
| AA11B.JT | 11.56 / 9.22 / 6.88 | 11.56 / 10.38 / 7.79 | 11.56 / 13.77 / 10.47 | 11.56 / 15.30 / 16.85 |
| AA12.JT | 24.56 / 13.50 / 8.81 | 29.38 / 15.72 / 10.31 | 45.85 / 21.67 / 14.32 | 62.16 / 38.02 / 27.12 |
| AA2.JT | 0.00 / 0.00 / 2.18 | 0.00 / 0.00 / 2.51 | 0.00 / 0.00 / 3.50 | 0.00 / 0.00 / 6.69 |
| AA6.JT | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A |
| AA8.JT | 5.78 / 7.08 / 4.10 | 5.78 / 7.38 / 4.33 | 5.78 / 7.65 / 4.79 | 5.78 / 7.65 / 5.46 |
| AA9.JT | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A |
| AAML.JT | 38.72 / 21.53 / 10.72 | 43.85 / 25.16 / 15.64 | 58.33 / 34.52 / 22.53 | 150.00 / 68.96 / 46.08 |
| AEMP1.JT | 24.55 / 13.42 / 8.79 | 29.36 / 15.62 / 10.30 | 45.83 / 21.53 / 14.31 | 62.15 / 37.92 / 27.11 |
| AIM120.JT | 32.66 / 16.44 / 10.31 | 40.15 / 19.10 / 12.06 | 52.69 / 26.92 / 17.19 | 59.98 / 36.40 / 25.75 |
| AIM54C.JT | 12.41 / 6.28 / 2.39 | 52.09 / 21.87 / 11.00 | 65.71 / 32.37 / 18.16 | 150.00 / 70.76 / 43.01 |
| AIM7.JT | 16.11 / 9.10 / 5.88 | 18.19 / 10.45 / 6.89 | 24.27 / 14.23 / 9.71 | 58.45 / 27.82 / 19.63 |
| AIM7E.JT | 12.86 / 8.17 / 5.73 | 14.44 / 9.21 / 6.53 | 19.04 / 12.15 / 8.77 | 36.61 / 23.17 / 17.18 |
| AIM9B.JT | 0.00 / 0.00 / 3.16 | 0.00 / 0.00 / 3.67 | 0.00 / 0.00 / 5.20 | 0.00 / 0.00 / 7.99 |
| AIM9M.JT | 9.64 / 8.78 / 5.52 | 9.64 / 9.29 / 5.95 | 9.64 / 10.26 / 6.79 | 9.64 / 11.48 / 8.01 |
| AIM9X.JT | 9.64 / 8.79 / 5.52 | 9.64 / 9.32 / 5.95 | 9.64 / 10.29 / 6.80 | 9.64 / 11.50 / 8.03 |
| MICA.JT | 23.61 / 11.29 / 6.29 | 26.51 / 13.09 / 7.53 | 37.37 / 18.50 / 11.34 | 59.98 / 35.27 / 23.69 |
| PL10.JT | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A |
| PL7.JT | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A | N/A / N/A / N/A |
| R27ER.JT | 48.93 / 15.05 / 7.38 | 49.98 / 16.62 / 8.40 | 56.68 / 24.23 / 13.35 | 115.20 / 57.52 / 37.54 |
| R27ET.JT | 11.56 / 15.30 / 10.97 | 11.56 / 15.30 / 12.73 | 11.56 / 15.30 / 17.88 | 11.56 / 15.30 / 18.28 |
| R27T.JT | 11.56 / 11.61 / 8.17 | 11.56 / 13.35 / 9.42 | 11.56 / 15.30 / 13.11 | 11.56 / 15.30 / 18.29 |
| R530.JT | 13.13 / 8.82 / 6.35 | 14.70 / 9.90 / 7.21 | 19.44 / 12.86 / 9.54 | 43.88 / 25.64 / 18.78 |
| R550.JT | 9.64 / 8.05 / 4.86 | 9.64 / 8.31 / 5.08 | 9.64 / 8.79 / 5.51 | 9.64 / 9.30 / 6.11 |

## SAM and powered A-G calibration

SAM launcher is stationary at 10 m MSL. The stationary air reference target
is at clamp(0.2*anchor_range_m,300,3000) m, an agent-selected condition.
The additional head/beam/tail cases use a 250 m/s airborne target. A-G launch
is 300 m/s at 1,500 m, except the slow command reference at 30 m/s and 100 m.
Ship strike launch is stationary at 10 m. Ground targets are stationary at
10 m. Distances are horizontal separation. Laser/command point predictions
use the existing 25 ft contact tolerance. Other targets use a 30 ft sphere
plus decoded fuze radius and arming delay. Aircraft launch minimums, radar
acquisition limits and weather remain separate gates.

Four numbers in a SAM cell are head / beam / tail / stationary; A-G has one
stationary value. N/A denotes a held or separate laser/command channel.
Physical-only rows make no seeker-valid claim. AS15/AT12 retain decoded mass
and range references because their real-world identities remain unresolved.
SAM aspect allowance is existing game policy, not a real-world claim.

| Store | Reference km | Launcher/target m | Physical km | Seeker-valid km |
| --- | ---: | --- | --- | --- |
| AGM45.JT | 40 | 1500/10 | 40.00 | 40.00 |
| AGM65A.JT | 27 | 1500/10 | 27.00 | 15.16 |
| AGM65G.JT | 32.18688 | 1500/10 | 32.19 | 18.23 |
| AGM84A.JT | 185 | 1500/10 | 186.11 | 186.11 |
| AGM84E.JT | 93 | 1500/10 | 93.00 | 91.41 |
| AGM88.JT | 48.28032 | 1500/10 | 48.28 | 48.28 |
| AM39.JT | 70 | 1500/10 | 71.48 | 71.48 |
| AS14.JT | 10 | 1500/10 | 10.00 | N/A |
| AS15.JT | 18.288 | 1500/10 | 18.29 | 18.29 |
| AS16.JT | 150 | 1500/10 | 150.00 | 150.00 |
| AS30.JT | 11 | 1500/10 | 11.00 | N/A |
| AS7.JT | 10 | 1500/10 | 10.00 | 10.00 |
| AT12.JT | 7.9248 | 1500/10 | 7.92 | N/A |
| AT2.JT | 4 | 100/10 | 4.26 | N/A |
| FIM92.JT | 4 | 10/800 | 7.19 / 1.69 / 0.58 / 4.00 | 7.19 / 1.68 / 0.58 / 4.00 |
| MIS.JT | 6.5 | 10/1300 | 10.77 / 4.39 / 2.17 / 6.50 | 10.77 / 4.39 / 2.17 / 6.50 |
| R440.JT | 10 | 10/2000 | 14.55 / 8.71 / 5.91 / 10.20 | 14.55 / 8.71 / 5.91 / 10.20 |
| SA13.JT | 5 | 10/1000 | 8.84 / 2.21 / 0.92 / 5.00 | 4.75 / 2.20 / 0.92 / 5.00 |
| SA14.JT | 4.5 | 10/900 | 7.96 / 2.21 / 0.85 / 4.50 | 3.79 / 2.21 / 0.85 / 4.50 |
| SA15.JT | 12 | 10/2400 | 18.27 / 9.60 / 5.83 / 12.00 | 18.27 / 9.60 / 5.83 / 12.00 |
| SA16.JT | 5.2 | 10/1040 | 8.95 / 3.02 / 1.33 / 5.20 | 2.77 / 3.02 / 1.33 / 4.44 |
| SA19.JT | 8 | 10/1600 | 12.38 / 6.37 / 3.66 / 8.00 | 12.38 / 6.37 / 3.66 / 8.00 |
| SA2A.JT | 29 | 10/3000 | 41.96 / 24.33 / 16.05 / 29.01 | 41.96 / 24.33 / 16.05 / 29.01 |
| SA3.JT | 25 | 10/3000 | 36.98 / 20.19 / 13.06 / 25.00 | 36.83 / 20.19 / 13.06 / 25.00 |
| SA6.JT | 24 | 10/3000 | 34.59 / 20.19 / 13.46 / 24.00 | 34.38 / 20.19 / 13.46 / 24.00 |
| SA7.JT | 4.2 | 10/840 | 7.49 / 1.99 / 0.72 / 4.20 | 2.81 / 1.99 / 0.72 / 4.20 |
| SA9.JT | 4.2 | 10/840 | 7.57 / 1.73 / 0.57 / 4.20 | 3.80 / 1.73 / 0.57 / 4.20 |
| SAN11.JT | 8 | 10/1600 | 12.38 / 6.37 / 3.66 / 8.00 | 12.38 / 6.37 / 3.66 / 8.00 |
| SAN3.JT | 30 | 10/3000 | 42.02 / 26.07 / 17.91 / 30.00 | 42.01 / 26.07 / 17.91 / 30.00 |
| SAN4.JT | 10 | 10/2000 | 15.67 / 7.63 / 4.37 / 10.00 | 15.67 / 7.63 / 4.37 / 10.00 |
| SAN7.JT | 25 | 10/3000 | 36.23 / 21.01 / 13.84 / 25.00 | 36.11 / 21.01 / 13.84 / 25.00 |
| SAN9.JT | 12 | 10/2400 | 18.27 / 9.60 / 5.83 / 12.00 | 18.27 / 9.60 / 5.83 / 12.00 |
| SEA_SPAR.JT | 26 | 10/3000 | 37.20 / 22.03 / 14.83 / 26.01 | 37.17 / 22.03 / 14.83 / 26.01 |
| SSN9.JT | 150 | 10/10 | 151.12 | 151.12 |

## Two-condition A2A comparison

H35 is 35,000 ft head-on; T5 is 5,000 ft astern, at the speeds above.
Each pair is H35 / T5, km. The first target is the existing primary reference;
the second is the measured round-2 T5. Per-store fuel, thrust and burn durations
are explicitly fitted game coefficients. Class time and the defeat rule were
not optimized. [All fitted parameters and provenance](../formats/missile-energy-data.md#per-store-motors-and-class-control-budgets)
have one home in the data document.

| Store | Targets H35 / T5 | Before, ba1bf7e4 | Current | Residual H35 / T5 |
| --- | ---: | ---: | ---: | ---: |
| AA10.JT | 73.000 / 6.473 | 64.734 / 7.180 | 64.734 / 7.180 | -11.32% / +10.92% |
| AA11.JT | 30.000 / 7.056 | 26.846 / 7.794 | 26.846 / 7.794 | -10.51% / +10.46% |
| AA11B.JT | 30.000 / 7.056 | 26.846 / 7.794 | 26.846 / 7.794 | -10.51% / +10.46% |
| AA12.JT | 50.000 / 10.767 | 52.037 / 10.443 | 62.161 / 10.317 | +24.32% / -4.18% |
| AA2.JT | 20.000 / 2.180 | 16.691 / 2.509 | 16.691 / 2.509 | -16.55% / +15.09% |
| AA6.JT | 30.000 / 6.811 | 32.841 / 6.176 | 32.841 / 6.176 | +9.47% / -9.32% |
| AA8.JT | 10.000 / 2.377 | 13.198 / 2.274 | 15.461 / 4.334 | +54.61% / +82.33% |
| AA9.JT | 120.000 / 6.816 | 120.027 / 6.823 | 120.027 / 6.823 | +0.02% / +0.10% |
| AAML.JT | 150.000 / 15.620 | 150.000 / 15.637 | 150.000 / 15.637 | +0.00% / +0.11% |
| AEMP1.JT | 50.000 / 10.752 | 52.144 / 10.462 | 62.146 / 10.302 | +24.29% / -4.19% |
| AIM120.JT | 60.000 / 12.058 | 60.000 / 12.058 | 60.000 / 12.058 | +0.00% / +0.00% |
| AIM54C.JT | 150.000 / 9.811 | 150.000 / 10.999 | 150.000 / 10.999 | +0.00% / +12.11% |
| AIM7.JT | 70.000 / 5.792 | 58.577 / 6.890 | 58.577 / 6.890 | -16.32% / +18.96% |
| AIM7E.JT | 35.000 / 8.061 | 36.768 / 6.530 | 36.768 / 6.530 | +5.05% / -18.99% |
| AIM9B.JT | 20.000 / 3.693 | 17.994 / 3.673 | 17.994 / 3.673 | -10.03% / -0.54% |
| AIM9M.JT | 18.000 / 5.938 | 18.010 / 5.947 | 18.010 / 5.947 | +0.06% / +0.15% |
| AIM9X.JT | 18.000 / 5.944 | 18.029 / 5.952 | 18.029 / 5.952 | +0.16% / +0.13% |
| MICA.JT | 60.000 / 7.527 | 60.000 / 7.530 | 60.000 / 7.530 | +0.00% / +0.04% |
| PL10.JT | 50.000 / 8.699 | 50.092 / 8.699 | 50.092 / 8.699 | +0.18% / +0.00% |
| PL7.JT | 14.000 / 5.474 | 15.691 / 4.819 | 15.691 / 4.819 | +12.08% / -11.97% |
| R27ER.JT | 117.000 / 8.402 | 117.000 / 8.402 | 117.000 / 8.402 | +0.00% / +0.00% |
| R27ET.JT | 104.000 / 12.755 | 104.128 / 12.730 | 104.128 / 12.730 | +0.12% / -0.20% |
| R27T.JT | 63.000 / 9.309 | 61.523 / 9.414 | 61.523 / 9.414 | -2.34% / +1.13% |
| R530.JT | 40.000 / 7.997 | 43.916 / 7.214 | 43.916 / 7.214 | +9.79% / -9.79% |
| R550.JT | 15.000 / 5.476 | 16.111 / 5.078 | 16.111 / 5.078 | +7.41% / -7.27% |

Nine records have explicit residual exceptions: AA10, AA2, AA8, AA12, AEMP1,
AIM54C, AIM7, AIM7E and PL7. John's usable-envelope decision supersedes AA8's
former head-only bound and adds the two climbing-store exceptions, while
retaining motor guards and role ordering. The default
verifier band is 89.5%..110.5% of the reference, with half a percentage point
for sampling tolerance. The exceptions below are bounds, not exemptions.
M/X now use the ordinary band; their former overshoot is no longer accepted.

| Store | Accepted H35/reference | Accepted T5/reference | Reason |
| --- | --- | --- | --- |
| PL7 | 0.875..1.125 | 0.875..1.125 | John-approved guarded motor compromise |
| AA10 | 0.86..1.105 | 0.895..1.13 | Best found bounded joint role/family fit |
| AA2 | 0.82..1.105 | 0.895..1.18 | Best found bounded two-condition fit |
| AA8 | 0.895..1.56 | 0.895..1.85 | Best found guarded fit under John's usable-envelope priority |
| AA12, AEMP1 | 0.895..1.25 | 0.895..1.105 | Close the shared climbing-sector collapse while retaining the tail band |
| AIM54C | 0.895..1.105 | 0.895..1.14 | 1.2 times decoded minimum is 10.9728 km, above old 9.811 km tail |
| AIM7 | 0.82..1.105 | 0.895..1.21 | Conflicting tail targets plus joint ordering |
| AIM7E | 0.895..1.105 | 0.79..1.105 | Conflicting tail targets plus joint ordering |

Round-2 Sparrow tails were 5.792 km for AIM7 and 8.061 km for AIM7E.
Even their 10% bands do not overlap in generation order. Equal relative
error meets around 6.74 km before the other constraints. John's conditional
permission for a real-world-supported reversal does not establish that these
game-fit targets are real-world equivalents, so the generation check remains.
The other search residuals are best found within the documented search bounds,
not proofs that no better fit exists. No per-store timer or geometry drag
factor was introduced to hide a residual.

## SAM and A-G before/after

The reference condition for each row is the stationary target and stated
heights in the preceding SAM/A-G table. All 34 ground-role records are shown.
Class assignment is explicit data. AS7/AS14/AS30/AT12 share ShortGround 30 s;
AS16 uses BoostGlide 600 s, retaining normal drag. AM39 and AGM84E use
the shared ShortCruise 300 s class approved in Phase 2.8.
These agent assignments use the lead's explicit permission; the class times
were fixed before motor fitting. The original nine class values remain.

| Store | Class / control s | Reference km | Round 2 km | Phase 2.5 km | Phase 2.6 km | Phase 2.7 km | Phase 2.8 km | Residual |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| AGM45.JT | FastGround / 90 | 40.000 | 39.998 | 42.735 | 40.000 | 40.000 | 40.000 | +0.00% |
| AGM65A.JT | FastGround / 90 | 27.000 | 26.998 | 27.387 | 27.000 | 27.000 | 27.000 | +0.00% |
| AGM65G.JT | FastGround / 90 | 32.187 | 32.186 | 33.811 | 32.187 | 32.187 | 32.187 | +0.00% |
| AGM84A.JT | Cruise / 600 | 185.000 | 184.953 | 189.788 | 183.074 | 186.111 | 186.111 | +0.60% |
| AGM84E.JT | ShortCruise / 300 | 93.000 | 93.010 | 173.696 | 93.011 | 99.166 | 93.000 | +0.00% |
| AGM88.JT | FastGround / 90 | 48.280 | 48.280 | 49.199 | 48.280 | 48.280 | 48.280 | -0.00% |
| AM39.JT | ShortCruise / 300 | 70.000 | 69.982 | 78.770 | 70.000 | 76.268 | 71.476 | +2.11% |
| AS14.JT | ShortGround / 30 | 10.000 | 10.010 | 33.726 | 10.000 | 10.000 | 10.000 | +0.00% |
| AS15.JT | FastGround / 90 | 18.288 | 18.296 | 39.043 | 18.290 | 18.290 | 18.290 | +0.01% |
| AS16.JT | BoostGlide / 600 | 150.000 | 149.954 | 76.236 | 150.000 | 150.000 | 150.000 | +0.00% |
| AS30.JT | ShortGround / 30 | 11.000 | 11.011 | 34.610 | 10.998 | 10.998 | 10.998 | -0.02% |
| AS7.JT | ShortGround / 30 | 10.000 | 10.006 | 22.702 | 10.000 | 10.000 | 10.000 | +0.00% |
| AT12.JT | ShortGround / 30 | 7.925 | 7.943 | 53.390 | 7.923 | 7.923 | 7.923 | -0.02% |
| AT2.JT | Command / 30 | 4.000 | 4.008 | 9.102 | 4.260 | 4.260 | 4.260 | +6.50% |
| FIM92.JT | SurfaceIr / 25 | 4.000 | 4.010 | 7.898 | 4.000 | 4.000 | 4.000 | +0.00% |
| MIS.JT | SurfaceRadar / 70 | 6.500 | 6.507 | 7.014 | 6.500 | 6.500 | 6.500 | +0.00% |
| R440.JT | SurfaceRadar / 70 | 10.000 | 10.014 | 16.161 | 10.198 | 10.198 | 10.198 | +1.98% |
| SA13.JT | SurfaceIr / 25 | 5.000 | 5.014 | 11.881 | 5.000 | 5.000 | 5.000 | +0.00% |
| SA14.JT | SurfaceIr / 25 | 4.500 | 4.511 | 7.445 | 4.500 | 4.500 | 4.500 | +0.00% |
| SA15.JT | SurfaceRadar / 70 | 12.000 | 12.003 | 10.158 | 12.000 | 12.000 | 12.000 | +0.00% |
| SA16.JT | SurfaceIr / 25 | 5.200 | 5.211 | 7.112 | 5.201 | 5.201 | 5.201 | +0.02% |
| SA19.JT | SurfaceRadar / 70 | 8.000 | 8.002 | 5.607 | 8.001 | 8.001 | 8.001 | +0.01% |
| SA2A.JT | SurfaceRadarLong / 120 | 29.000 | 29.003 | 18.214 | 29.006 | 29.006 | 29.006 | +0.02% |
| SA3.JT | SurfaceRadarLong / 120 | 25.000 | 25.002 | 16.332 | 25.000 | 25.000 | 25.000 | +0.00% |
| SA6.JT | SurfaceRadar / 70 | 24.000 | 23.995 | 15.065 | 24.000 | 24.000 | 24.000 | +0.00% |
| SA7.JT | SurfaceIr / 25 | 4.200 | 4.208 | 6.713 | 4.202 | 4.202 | 4.202 | +0.05% |
| SA9.JT | SurfaceIr / 25 | 4.200 | 4.210 | 7.768 | 4.200 | 4.200 | 4.200 | +0.00% |
| SAN11.JT | SurfaceRadar / 70 | 8.000 | 8.002 | 5.607 | 8.001 | 8.001 | 8.001 | +0.01% |
| SAN3.JT | SurfaceRadarLong / 120 | 30.000 | 29.991 | 18.645 | 30.000 | 30.000 | 30.000 | +0.00% |
| SAN4.JT | SurfaceRadar / 70 | 10.000 | 10.006 | 8.814 | 10.000 | 10.000 | 10.000 | +0.00% |
| SAN7.JT | SurfaceRadarLong / 120 | 25.000 | 25.002 | 15.540 | 25.004 | 25.004 | 25.004 | +0.02% |
| SAN9.JT | SurfaceRadar / 70 | 12.000 | 12.003 | 10.158 | 12.000 | 12.000 | 12.000 | +0.00% |
| SEA_SPAR.JT | SurfaceRadarLong / 120 | 26.000 | 26.000 | 18.100 | 26.006 | 26.006 | 26.006 | +0.02% |
| SSN9.JT | Cruise / 600 | 150.000 | 149.954 | 153.634 | 144.274 | 151.120 | 151.120 | +0.75% |

## Family, role and altitude checks

Family margins use all twelve altitude/aspect cells. Short-IR comparisons
use the three 5k/15k/35k tail cells. Low-altitude radar role comparisons use
5k head and tail. These checks do not assert universal real-world ordering.

| Newer / older | Minimum measured advantage |
| --- | ---: |
| AIM7.JT / AIM7E.JT | 2.529% |
| R27ER.JT / AA10.JT | 17.019% |
| R27ET.JT / R27T.JT | 34.280% |
| AIM9M.JT / AIM9B.JT | 0.175% |
| AIM9M.JT / AA2.JT | 19.647% |
| AIM9M.JT / AA8.JT | 37.263% |
| AIM9X.JT / AIM9B.JT | 0.388% |
| AIM9X.JT / AA2.JT | 19.901% |
| AIM9X.JT / AA8.JT | 37.379% |
| AA11.JT / AIM9B.JT | 101.540% |
| AA11.JT / AA2.JT | 151.785% |
| AA11.JT / AA8.JT | 79.857% |

| Active radar / older group | Smallest 5k head margin km | Smallest 5k tail margin km |
| --- | ---: | ---: |
| AIM120.JT | 14.366 | 4.844 |
| AA12.JT | 3.608 | 3.101 |
| MICA.JT | 0.741 | 0.316 |

| Store | H5/H35 | T5/H35 | T5 class floor km | 1.2 x decoded min km |
| --- | ---: | ---: | ---: | ---: |
| AA10.JT | 0.398 | 0.111 | 3 | 4.389 |
| AA11.JT | 0.560 | 0.290 | 1.5 | 0.732 |
| AA11B.JT | 0.560 | 0.290 | 1.5 | 0.549 |
| AA12.JT | 0.473 | 0.166 | 3 | 4.389 |
| AA2.JT | 0.423 | 0.150 | 1.5 | 1.463 |
| AA6.JT | 0.400 | 0.188 | 3 | 4.389 |
| AA8.JT | 0.927 | 0.280 | 1.5 | 1.463 |
| AA9.JT | 0.353 | 0.057 | 5 | 4.389 |
| AAML.JT | 0.292 | 0.104 | 5 | 10.973 |
| AEMP1.JT | 0.473 | 0.166 | 3 | 0.732 |
| AIM120.JT | 0.669 | 0.201 | 3 | 4.389 |
| AIM54C.JT | 0.347 | 0.073 | 5 | 10.973 |
| AIM7.JT | 0.310 | 0.118 | 3 | 2.926 |
| AIM7E.JT | 0.393 | 0.178 | 3 | 1.097 |
| AIM9B.JT | 0.576 | 0.204 | 1.5 | 1.463 |
| AIM9M.JT | 0.886 | 0.330 | 1.5 | 1.463 |
| AIM9X.JT | 0.885 | 0.330 | 1.5 | 1.097 |
| MICA.JT | 0.442 | 0.126 | 3 | 4.389 |
| PL10.JT | 0.711 | 0.174 | 3 | 2.926 |
| PL7.JT | 0.944 | 0.307 | 1.5 | 1.463 |
| R27ER.JT | 0.427 | 0.072 | 5 | 4.389 |
| R27ET.JT | 0.353 | 0.122 | 5 | 4.389 |
| R27T.JT | 0.357 | 0.153 | 3 | 4.389 |
| R530.JT | 0.335 | 0.164 | 3 | 2.926 |
| R550.JT | 0.936 | 0.315 | 1.5 | 1.463 |

The decoded minimum removes these nominal physical employment intervals:

- AA9.JT 500 ft tail: 2.540 km <= minimum 3.658 km
- AIM54C.JT 500 ft beam: 6.276 km <= minimum 9.144 km
- AIM54C.JT 500 ft tail: 2.393 km <= minimum 9.144 km

These are range-policy exclusions, not zero kinetic reach. The floor check
uses 5k tail, not the 500 ft deck. Altitude reversals above 10%: none.


## Envelope correction evidence

This is an implementation/balance pass on the remake, not real-world research.
The immutable before executable is `ba1bf7e4`. Both builds repeat the 500-cell
survey for AA8, AA12, AEMP1 and R27ER: eleven altitude cases, five horizontal
aspects, 200/300 m/s shooters against 250 m/s, plus a 150 m/s target subset.
Physical trials use ideal observations; seeker trials use live guidance.
Ranges below are horizontal separation. The first sweep uses factor 1.1 and
refines every sampled transition to 0.5 m. A denser factor-1.01 rescan covers
all 125 AA8 cells and the twenty AA12/AEMP1 5k-to-30k climb cells. These remain
finite samples, not proofs about every continuous geometry.

| Store | Empty physical cells before / after | Empty seeker cells before / after |
| --- | ---: | ---: |
| AA8.JT | 65 / 31 | 67 / 34 |
| AA12.JT | 11 / 0 | 11 / 0 |
| AEMP1.JT | 12 / 0 | 13 / 0 |
| R27ER.JT | 0 / 0 | 0 / 0 |

AA8's co-altitude empty seeker cells fall from 20 to zero across 75 cases;
all final sampled AA8 hit bands are single contiguous intervals. Its head-on
seeker limit remains about 5.78 km under the normalized target signature,
so the larger physical H35 exception does not create a 15 km IR head-on lock.
AA12 and AEMP1 have no empty surveyed sector after the fit. They still have
sampled loft/path gaps in six and nine seeker cells respectively, which the
region-advice HUD policy handles. R27ER's complete before/after survey files
are byte-identical: its physical gaps were not erased by a guidance change.

| Store / case | Shooter m/s | Aspect deg | Physical / seeker km before | Physical / seeker km after |
| --- | ---: | ---: | ---: | ---: |
| AA8.JT / co35000 | 300 | 90 | 4.667 / 4.509 | 8.559 / 7.650 |
| AA8.JT / co45000 | 300 | 90 | 0.000 / 0.000 | 8.521 / 7.650 |
| AA8.JT / co15000 | 200 | 90 | 0.000 / 0.000 | 5.618 / 5.578 |
| AA8.JT / co500 | 300 | 90 | 2.075 / 0.000 | 7.117 / 7.086 |
| AA8.JT / up5_15 | 300 | 90 | 0.000 / 0.000 | 5.505 / 5.469 |
| AA12.JT / up5_30 | 300 | 90 | 0.000 / 0.000 | 14.831 / 14.787 |
| AA12.JT / up5_30 | 200 | 135 | 0.000 / 0.000 | 5.271 / 5.261 |
| AEMP1.JT / up5_30 | 300 | 90 | 0.000 / 0.000 | 14.791 / 14.601 |
| AEMP1.JT / up5_30 | 200 | 135 | 0.000 / 0.000 | 5.227 / 4.922 |

The cause is insufficient energy in the earlier guarded fits, compounded by
pressure-limited turning and induced drag. In the 45k beam/3 km AA8 physical
trace at three seconds, available versus required G is 4.94 / 9.77 before and
7.11 / 11.25 after. The old 521 N sustain is below the 754 N drag force at
that moment; the new 1,463 N sustain exceeds its 1,060 N drag. The old shot
is defeated at 10.03 s, missing by 845 m; the new shot hits at 9.63 s. Lift
remains bounded and the new shot still has a temporary G deficit.

For the 15k beam/2 km AA8 shot from 200 m/s, the old flight is only Mach 0.728
at three seconds and is defeated at 7.34 s, 1,025 m short. The new flight is
Mach 0.961 at that sample, continues accelerating and hits at 9.20 s. The
5k-to-30k AA12 head shot from 200 m/s has no G deficit at the three-second
sample in either build; Mach rises from 0.849 to 1.690. A 15 km shot changes
from defeat with a 4,008 m miss to a hit at 24.20 s. AEMP1 shares this energy
failure and the same authored body dimensions, so it receives the same fitted
motor/drag data. Its fuze/seeker differences remain intact.

Counterfactual traces increased lift alone, removed post-burn defeat, extended
control alone, reduced induced drag or reduced PN gain. None alone repaired
the representative slow/climbing holes. Stronger motor impulse did. Those
counterfactuals are diagnostic, not shipping rules. AA8 retains its all-aspect
classification, 35 deg/s seeker slew, 5-degree instantaneous half-FOV and
45-degree gimbal; AA12/AEMP1 retain their 60-degree gimbals. In the deck AA8
1.5 km beam shot both builds have the same initial 0.2 s memory/acquisition
period and then a locked return. The old marginal-energy seeker flight is
defeated 141 m short; the new one hits at 5.35 s. This does not support a
seeker-setting change. Rear-only AIM9B/AA2 limits are untouched.

Remaining AA8 vertical exclusions, confirmed in the dense scan:

- 5k to 15k from 200 m/s: all five aspects remain physically unreachable.
  The 3 km beam trace still runs below the post-burn threshold and is defeated.
  From 300 m/s all five aspects now have a seeker-valid band.
- 5k to 30k: both shooter speeds remain physically unreachable at all aspects.
  The 7.62 km vertical separation consumes too much energy within the unchanged
  20 s class budget; many launch geometries also fail IR acquisition.
- 30k to 5k: seeker-valid shots remain absent except the 300 m/s tail case.
  Head/quarter shots can have physical reach but no initial IR return; the
  minimum slant distance already exceeds the normalized head-on acquisition
  range. Other turning shots exhaust energy or control time. The 6 km beam
  physical trace reaches the control limit rather than an invented seeker hit.
- 30k to 500 ft: both speeds remain physically unreachable at all aspects.
  The large turn/descent consumes the short control budget, and acquisition
  excludes many of these geometries independently.
- 15k to 500 ft: all ten tested aspect/speed combinations now have valid bands.

The local task folder contains the 500 paired cells, the 145 dense cells,
64 matched force/seeker traces and the bounded candidate searches. Before
trace reconstruction was checked against the frozen before executable for
identical outcomes and numeric differences below 0.0001. The complete current
range/force/motor audits contain 391, 1,002 and 59 rows respectively.

## Defeat and force diagnostics

The offline `--diagnostics` mode opts into available/required G, positive
G deficit, closing speed, Mach, control expiry and post-burn defeat at each
measured edge, 0.1% beyond and 10% beyond. These values do not create a G-based death rule.
Raw commanded G can exceed available G; applied steering stays bounded.
Ordinary motion and prediction skip diagnostic-only calculations.
The behaviour tripwire includes low-Mach countdown, reset, defeat transition
and inert contacts. Dedicated tests cover the separate warning publication rule.

## Motor-shape audit

Phase 2.8 closes the weak-sustain loophole with authored data and offline guards.
A2A and surface air-target stores require at least 1 g sustain thrust on mass at
sustain entry, where that ratio is lowest, or no sustain phase. Their combined
burn time is at most half the fixed class budget. Ignition is separate. Ground
stores are not subject to those two air-target limits because they do not use
post-burn Mach defeat. Full parameters have one home in
[the fitted data table](../formats/missile-energy-data.md#per-store-motors-and-class-control-budgets).

Every store is listed below, including unchanged records. A dash in the cap
column means the air-target burn cap does not apply. Initial acceleration is
still checked by the existing class floors. The Mach-one probe is the same
250 m/s, 5,000 ft launch for A2A and SAM records; it does not represent a
stationary SAM launch. No crossing within its 20 seconds is shown as None.

| Store | Class | Burn s | Control s | Burn/control | Air burn cap s | Sustain-entry g | Mach-one s |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| AA10.JT | MediumAir | 34.558 | 70 | 49.37% | 35 | 2.603 | 0.358 |
| AA11.JT | ShortIr | 4.733 | 20 | 23.67% | 10 | 34.563 | 0.292 |
| AA11B.JT | ShortIr | 4.733 | 20 | 23.67% | 10 | 34.563 | 0.292 |
| AA12.JT | MediumAir | 13.792 | 70 | 19.70% | 35 | 2.210 | 0.592 |
| AA2.JT | ShortIr | 1.758 | 20 | 8.79% | 10 | 33.702 | 0.350 |
| AA6.JT | MediumAir | 4.742 | 70 | 6.77% | 35 | 33.034 | 1.383 |
| AA8.JT | ShortIr | 6.083 | 20 | 30.42% | 10 | 3.599 | 1.008 |
| AA9.JT | LongAir | 69.000 | 180 | 38.33% | 90 | 1.089 | 2.958 |
| AAML.JT | LongAir | 37.792 | 180 | 21.00% | 90 | 2.661 | 2.558 |
| AEMP1.JT | MediumAir | 13.792 | 70 | 19.70% | 35 | 2.210 | 0.592 |
| AGM45.JT | FastGround | 8.000 | 90 | 8.89% | - | 6.342 | 0.450 |
| AGM65A.JT | FastGround | 8.000 | 90 | 8.89% | - | 5.659 | 1.533 |
| AGM65G.JT | FastGround | 8.000 | 90 | 8.89% | - | 5.428 | 1.550 |
| AGM84A.JT | Cruise | 962.000 | 600 | 160.33% | - | 0.623 | None |
| AGM84E.JT | ShortCruise | 298.883 | 300 | 99.63% | - | 0.466 | None |
| AGM88.JT | FastGround | 8.000 | 90 | 8.89% | - | 7.560 | 0.392 |
| AIM120.JT | MediumAir | 15.225 | 70 | 21.75% | 35 | 2.105 | 0.817 |
| AIM54C.JT | LongAir | 77.092 | 180 | 42.83% | 90 | 1.540 | 2.650 |
| AIM7.JT | MediumAir | 16.000 | 70 | 22.86% | 35 | 2.941 | 1.142 |
| AIM7E.JT | MediumAir | 5.950 | 70 | 8.50% | 35 | 14.037 | 1.350 |
| AIM9B.JT | ShortIr | 2.200 | 20 | 11.00% | 10 | 0.000 | 0.367 |
| AIM9M.JT | ShortIr | 5.892 | 20 | 29.46% | 10 | 7.655 | 0.858 |
| AIM9X.JT | ShortIr | 6.008 | 20 | 30.04% | 10 | 4.859 | 0.942 |
| AM39.JT | ShortCruise | 299.000 | 300 | 99.67% | - | 0.363 | None |
| AS14.JT | ShortGround | 7.342 | 30 | 24.47% | - | 4.518 | 1.808 |
| AS15.JT | FastGround | 8.000 | 90 | 8.89% | - | 2.507 | 2.250 |
| AS16.JT | BoostGlide | 113.883 | 600 | 18.98% | - | 0.924 | 3.400 |
| AS30.JT | ShortGround | 20.975 | 30 | 69.92% | - | 1.275 | 3.767 |
| AS7.JT | ShortGround | 6.125 | 30 | 20.42% | - | 4.019 | 1.850 |
| AT12.JT | ShortGround | 14.525 | 30 | 48.42% | - | 0.426 | None |
| AT2.JT | Command | 8.000 | 30 | 26.67% | - | 2.162 | None |
| FIM92.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.352 | 0.817 |
| MICA.JT | MediumAir | 27.975 | 70 | 39.96% | 35 | 2.395 | 1.992 |
| MIS.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 3.887 | 0.700 |
| PL10.JT | MediumAir | 11.158 | 70 | 15.94% | 35 | 1.107 | 2.775 |
| PL7.JT | ShortIr | 10.000 | 20 | 50.00% | 10 | 2.818 | 1.617 |
| R27ER.JT | LongAir | 82.808 | 180 | 46.00% | 90 | 1.399 | 0.600 |
| R27ET.JT | LongAir | 6.150 | 180 | 3.42% | 90 | 4.370 | 0.608 |
| R27T.JT | MediumAir | 3.167 | 70 | 4.52% | 35 | 16.906 | 0.225 |
| R440.JT | SurfaceRadar | 4.050 | 70 | 5.79% | 35 | 27.645 | 0.350 |
| R530.JT | MediumAir | 7.692 | 70 | 10.99% | 35 | 13.744 | 0.350 |
| R550.JT | ShortIr | 4.492 | 20 | 22.46% | 10 | 6.280 | 1.000 |
| SA13.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.019 | 0.867 |
| SA14.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.323 | 0.808 |
| SA15.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 5.019 | 0.558 |
| SA16.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.540 | 0.767 |
| SA19.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 5.540 | 0.525 |
| SA2A.JT | SurfaceRadarLong | 11.000 | 120 | 9.17% | 60 | 5.582 | 0.500 |
| SA3.JT | SurfaceRadarLong | 11.000 | 120 | 9.17% | 60 | 4.889 | 0.558 |
| SA6.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 5.851 | 0.483 |
| SA7.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.384 | 0.808 |
| SA9.JT | SurfaceIr | 11.000 | 25 | 44.00% | 12.5 | 3.119 | 0.858 |
| SAN11.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 5.540 | 0.525 |
| SAN3.JT | SurfaceRadarLong | 11.000 | 120 | 9.17% | 60 | 6.952 | 0.425 |
| SAN4.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 4.483 | 0.608 |
| SAN7.JT | SurfaceRadarLong | 11.000 | 120 | 9.17% | 60 | 5.640 | 0.500 |
| SAN9.JT | SurfaceRadar | 11.000 | 70 | 15.71% | 35 | 5.019 | 0.558 |
| SEA_SPAR.JT | SurfaceRadarLong | 11.000 | 120 | 9.17% | 60 | 6.081 | 0.475 |
| SSN9.JT | Cruise | 600.000 | 600 | 100.00% | - | 0.314 | 0.742 |

### Changed air-target motors

This comparison is `ba1bf7e4` to the current ordnance fix. Per-store fuel,
thrust, duration and drag are fitted game coefficients. All other motor data,
class times, the defeat rule, lift and seeker settings remain unchanged.
John accepted usable envelopes over the old range bands, with minimal practical
overshoot. The agent's bounded searches and refinement selected the smallest
head-on overshoot among candidates that passed the guarded sector checks,
not a proof of a global optimum. More than 40,000 bounded candidates were
screened across search/refinement runs. Requiring a hit at an arbitrary short
range overstated the needed motor change; the final selection instead requires
a nonempty band in each affected sector. The 8 g launch, 54 g dry-mass,
1 g sustain-entry and half-class burn guards remain enforced.

| Store | Burn s before / after | H35 km before / after | Mach-one s before / after | Boost kN / s after | Sustain kN / s after |
| --- | ---: | ---: | ---: | --- | --- |
| AA12.JT | 26.433 / 13.792 | 52.037 / 62.161 | 1.917 / 0.592 | 25.796 / 2.425 | 3.080 / 11.367 |
| AA8.JT | 7.092 / 6.083 | 13.198 / 15.461 | 2.917 / 1.008 | 3.751 / 1.317 | 1.463 / 4.767 |
| AEMP1.JT | 17.458 / 13.792 | 52.140 / 62.146 | 2.758 / 0.592 | 25.796 / 2.425 | 3.080 / 11.367 |

### What limits the A2A edges

Each cell is head / beam / tail. The code describes the result of a shot
0.1% beyond the sampled physical edge: D is kinetic defeat, C is control
expiry, G is ground contact, L is removal lifetime, O is another stop, and H
means the probe still hits. Combined flags are shown together. This identifies
the observed limiting event near the edge, not a proof of a continuous range
boundary. The additional 10%-beyond probe remains in the raw diagnostics.
The last successful shot can be briefly subsonic during the 30-tick defeat
dwell; the rule does not defeat a store on its first subsonic tick.

| Store | 500 ft | 5,000 ft | 15,000 ft | 35,000 ft |
| --- | --- | --- | --- | --- |
| AA10.JT | D / D / D | D / D / D | D / D / D | C / D / D |
| AA11.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AA11B.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AA12.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AA2.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AA6.JT | D / D / D | D / D / D | D / D / D | D / D / D |
| AA8.JT | C / C / C | C / C / C | C / C / C | C / C / C |
| AA9.JT | G / G / G | D / D / D | D / D / D | D / D / D |
| AAML.JT | D / D / G | D / D / D | D / D / D | C / D / D |
| AEMP1.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AIM120.JT | D / D / D | D / D / D | C / D / D | C / C / C |
| AIM54C.JT | G / G / G | D / D / D | D / D / D | D / D / D |
| AIM7.JT | D / D / D | D / D / D | D / D / D | C / D / D |
| AIM7E.JT | D / D / D | D / D / D | D / D / D | D / D / D |
| AIM9B.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| AIM9M.JT | C / C / C | C / C / C | C / C / C | C / C / C |
| AIM9X.JT | C / C / C | C / C / C | C / C / C | C / C / C |
| MICA.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| PL10.JT | D / G / G | D / D / D | C / D / D | C / C / C |
| PL7.JT | C / C / C | C / C / C | C / C / C | C / C / C |
| R27ER.JT | D / D / D | D / D / D | D / D / D | D / D / D |
| R27ET.JT | D / D / D | D / D / D | D / D / D | D / D / D |
| R27T.JT | D / D / D | D / D / D | D / D / D | C / C / C |
| R530.JT | D / D / D | D / D / D | D / D / D | D / D / D |
| R550.JT | C / C / C | C / C / C | C / C / C | C / C / C |

Across the 300 near-edge A2A shots: C 96, D 194, G 9, H 1. The H case is
AA8 at 500 ft, beam: the 2.077583 km shot still hits, about 2 m beyond the
sampled edge. At 10% beyond it is defeated at 9.992 s. This is a documented
sampling limitation; the report does not claim a precisely closed boundary
for that cell. The wider probe establishes kinetic defeat, not control expiry.


### Powered cruise limit and speed

John approved shared shorter cruise classes in Phase 2.8. Before fitting, the
agent assigned AM39 and AGM84E to ShortCruise 300 seconds. AGM84A and SSN9
retain Cruise 600 seconds. No timer is fitted per store. The guard requires
powered controlled hits at the reference, half and three quarters of it, and
three quarters of the measured edge and the edge itself, all at Mach 0.70..0.95.
The powered limit is independently searched using the powered-hit predicate.
All distances below are initial separation, not distance travelled.

| Store | Reference km | Phase 2.7 powered limit / edge km | Current powered limit / edge km | Half-reference Mach | Reference Mach | Edge Mach | Burnout / edge flight s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| AGM84A.JT | 185 | 186.130 / 186.128 | 186.128 / 186.128 | 0.915 | 0.929 | 0.929 | 964.000 / 600.008 |
| AGM84E.JT | 93 | 93.138 / 99.166 | 93.000 / 93.000 | 0.897 | 0.910 | 0.910 | 300.883 / 300.008 |
| AM39.JT | 70 | 70.009 / 76.289 | 71.497 / 71.497 | 0.701 | 0.705 | 0.705 | 301.000 / 300.008 |
| SSN9.JT | 150 | 151.165 / 151.155 | 151.155 / 151.155 | 0.820 | 0.822 | 0.822 | 600.000 / 599.958 |

The prior powered limits are the review7 measurements. Current limits use the
same physical kernel and standardized range conditions. Search resolution
can differ by a few meters; the exact measured edge is also checked directly
for a powered hit. The speed checks are samples, not continuous-flight bounds.

AS16 is explicitly BoostGlide 600 seconds, with unchanged motor, normal drag
and control duration. It is a boost-glide ground rocket, exempt from powered
cruise requirements. Ground glide completion remains an intentional behavior.

### Accepted boundaries and remaining approximations

AT12 retains its 550 m/s impulse coefficient, an accepted endpoint from the
old search and below the current staged rocket search's 600 m/s lower bound.
It still satisfies the common 400..6600 m/s verification guard. Its identity
is unresolved; this is an authored game value, not a sourced exhaust velocity.
The earlier claim that no coefficient had an exception was too broad.

The bounded range exceptions remain explicit in the reference table above.
John's ordnance-fix decision accepts usable-envelope fits with explicit residuals.
The [reference-band table](#two-condition-a2a-comparison) records the tight
bounds the agent selected around the verified fits. Reference values remain
visible; these exceptions do not revise source claims. Other bands are unchanged.
The air-target burn cap and sustain floor have no per-store exceptions.
Retained drag ceilings and fitted coefficients remain calibration choices;
all exact values and phase provenance are in the data table. AA11/AA11B's
retained 53.490 dry-mass g is close to the staged 53.5 g ceiling, an accepted
agent fit choice inside the 54 g guard and 10.85% below the old 60 g limit.
AGM84A retains a declared 25000 m/s coefficient with an explicit engine,
whose actual impulse/fuel coefficient is 11757.778 m/s. Both are inside the
sustain-only engine guard. Long cruise burn beyond control expiry is allowed;
no such exception applies to the air-target half-time cap.

Current search endpoints are accepted fitted choices within the common guard:

- AA11.JT: drag 2.999764.
- AA11B.JT: drag 2.999764.
- AA2.JT: fuel 0.499927.
- AA6.JT: fuel 0.499395, drag 2.999740.
- AGM84A.JT: fuel 0.500000.
- AIM7.JT: drag 3.000000.
- AIM7E.JT: fuel 0.500000.
- AIM9B.JT: fuel 0.120000, drag 3.000000.
- AIM9M.JT: fuel 0.120425.
- AS16.JT: drag 0.500000.
- AT12.JT: drag 3.000000, coefficient 550.000000.
- AT2.JT: fuel 0.120000, drag 3.000000.
- PL7.JT: fuel 0.120034.
- R440.JT: fuel 0.500000.

## Validation status

Current ordnance-fix validation, Linux x86-64, from `ba1bf7e4`:

- Workspace Clippy with warnings denied, formatting and documentation pass.
- The full workspace test command detects the expected wire golden change:
  `wire::golden_tests::wire_golden` fails because the readout gained its optional
  minimum. Protocol 75 is deliberately unchanged under the task instruction.
  The lead must bump the protocol and regenerate `tore-session/wire-golden.txt`
  at integration. The test and golden were not weakened or overwritten.
- Running the rest with that one named integration test skipped passes 7,309
  tests, with 107 existing ignored and one filtered. Four additional example
  tests pass the motor guards and explicit fit-exception boundaries.
- Workspace build, 541 Python tests (five skipped), source/app/extractor asset
  scans and the hidden-workspace rendering smoke test pass.
- All 36 selected ordnance/AI/countermeasure battery scenarios pass across the
  initial run and a focused rerun. The initial run passed 35. Its inert-defense
  fixture's only IR seed now hits, so it no longer exercises decoy passage.
  Adding the second deterministic seed restores that event while retaining
  the exact same active-before/cleared-on-inert-tick assertion. The rerun passes.
- All 59 reference/motor checks pass their explicit bands, class floors,
  minimum margins, family/radar-role ordering and local perturbation bounds.
  Control times, defeat rules and all other store fits remain unchanged.

Model identity changes from `2060610122284599500` at the base to
`3759883531439988201` for countermeasures, `501164167509140080` for range advice,
`3748787411052948283` after the three motor fits, `8371591701768486536`
for the unfloored chaff multiplier (decoy rule revision 3) and
`27838009745295643` for per-range region advice (prediction rule revision 7), then
`14178415726005986681` for the doubled chaff multiplier, 2.60 (decoy rule
revision 4, 2026-10-28). The behavior tripwire
trace remains `3894057758776089527`; its sampled stores are unchanged.
No protocol or replay version is bumped. Checkpoints gain minimum fields in
Envelope and RangeEstimate, and minimum/first fields in RangeSearch. The wire
estimates vector gains an optional minimum pair. Replay HUD samples gain
optional `min_ft`; decoy metadata gains optional `roll_bound` for basis points.
Older replay fields default to the decoded minimum and a 100-unit draw.

Windows/macOS and retail comparison were not run. The fitted ranges, sampled
bands and cache tolerances remain approximations. The invalid earlier Phase 3
study stays stopped. This task does not merge or push.

## Natural launch counts against both baselines

The retained gate is within +/-30% of reviewed Phase 2.5, plus a one-sided
floor of 70% of pre-overhaul Spec launches. The four theater fights run
14,400 ticks and the 23-AI fight 7,200. All cases have zero refused and zero
starved requests. Counts can change as the requested defenses change survival.

| Fight | Pre-overhaul Spec | Reviewed 2.5 | Current | Versus Spec | Versus 2.5 |
| --- | ---: | ---: | ---: | ---: | ---: |
| UKR | 11 | 13 | 11 | +0.00% | -15.38% |
| BAL | 12 | 13 | 11 | -8.33% | -15.38% |
| SPA | 12 | 13 | 12 | +0.00% | -7.69% |
| GRE | 12 | 13 | 11 | -8.33% | -15.38% |
| 23-AI UKR | 24 | 26 | 32 | +33.33% | +23.08% |

## Prediction cost

Runs use nice 5, sixteen Cargo/test workers, eight battery workers and at most
two wrapped windows. Heavy commands are serialized and check `free -g` first.
The work budget remains 16,384 quarter-step units per world tick. Frame readers
never advance prediction. Values below are milliseconds per world tick;
wall and CPU p95 are the worse of the cold and subsequent phases.

| Lane | Wall p95 ms | CPU p95 ms | Refused / starved |
| --- | ---: | ---: | ---: |
| natural UKR | 0.960 | 0.947 | 0 / 0 |
| natural BAL | 0.958 | 0.957 | 0 / 0 |
| natural SPA | 0.963 | 0.962 | 0 / 0 |
| natural GRE | 0.950 | 0.949 | 0 / 0 |
| natural big | 0.976 | 0.975 | 0 / 0 |
| turn 3 deg/s | 1.042 | 1.028 | 0 / 0 |
| turn 10 deg/s | 1.017 | 1.011 | 0 / 0 |
| AIM120 frozen, 1 stations | 1.355 | 1.348 | 0 / 0 |
| AIM120 moving, 1 stations | 1.526 | 1.329 | 0 / 0 |
| AIM120 moving, 8 stations | 1.561 | 1.362 | 0 / 0 |
| AA8.JT moving, 8 stations | 1.064 | 1.061 | 0 / 0 |
| AA12.JT moving, 8 stations | 1.034 | 1.030 | 0 / 0 |
| AGM84A.JT moving, 8 stations | 0.870 | 0.868 | 0 / 0 |

All measured p95 values stay below 2 ms. The held lanes run 2,400 ticks with
eight AI aircraft and sixteen replenished stores. They measure cost, not
universal cue availability. The player/turning-cue battery checks also pass.
The following matched table isolates the range-policy cost from motor changes.

## Contiguous range advice validation

**Current policy (lead decision, 2026-10-27).** The search below first
published only the first region. It now keeps up to four sampled regions and
the cue advises the one for the present range: the region containing it,
else the nearest below, else the first. On the same R27ER witness the search
finds two regions, 3.658..26.84 km and 43.31..47.99 km. 25 km advises the
first region and is IN RNG; 35 km (ground strike) advises the first region
without IN RNG; 45 km advises the far region and is IN RNG. The first region
publishes after 75 ticks and the complete search, including the far region,
after 486..498 ticks, at the same 16,315-unit peak work. AIM120 at 15,000 ft
head-on publishes after 142 ticks and completes after 326. On the A2G side,
the Mi-24 AT-2 from a 300 ft hover has a near region to about 0.99 km, ground
strikes, and a far region of 7,909..15,244 ft; `a2g-at2` again advises the
far region and releases at 8.63 s (8.53 s before this round). Cost on idle
Linux after the change, worse of cold and moving p95: AIM120 moving with 8
stations 1.061 ms, 1 station 1.017 ms, frozen 1.020 ms; AA8 moving 8 stations
1.069 ms; AA12 1.031 ms; zero refused or starved in every lane. The
historical first-region figures follow.

The ordnance-fixes range policy is the first sampled hit region outward from
the decoded minimum. The imported R27ER deck witness uses a 200 m/s shooter,
a 250 m/s head-on target and 500 ft altitude. The physical point answers are
unchanged: 25 km hits, 35 km hits ground, 45 km hits the target. The prior
maximum extended to the distant island. The new queue publishes a minimum of
3.658 km and an upper edge of 26.835..26.839 km for all three shot distances.
Only the 25 km shot belongs to that displayed region. The 35 km point fails
the AI gate; the valid 45 km point remains eligible for AI use but receives no
player IN RNG cue under the conservative first-region display policy.

These three searches complete in 111..124 world ticks. Peak work is 16,315
quarter-step units, within the unchanged 16,384-unit budget. There is no frame
reader flyout. Synthetic tests cover disconnected bands, a narrow current-shot
gap, decoded minimum and an exact-point miss under a broad envelope. The
focused simulation suite passed 2,181 tests with eight existing ignored.
Workspace all-target compilation and the 78-test Python flight/selection
subset passed. Network/replay minimum propagation is included in the full
[workspace validation](#validation-status).

Matched UKR cost runs use 2,400 ticks, eight AI aircraft, sixteen replenished
stores and one/eight candidate stations per actor. Values are the worse of
cold and subsequent p95, milliseconds per world tick:

| Lane | Before wall p95 | Before CPU p95 | Range fix wall p95 | Range fix CPU p95 |
| --- | ---: | ---: | ---: | ---: |
| frozen, 1 station(s) per actor | 1.366 | 1.362 | 1.355 | 1.348 |
| moving, 1 station(s) per actor | 1.317 | 1.314 | 1.526 | 1.329 |
| moving, 8 station(s) per actor | 1.619 | 1.380 | 1.561 | 1.362 |

All six runs recorded zero refused and zero starved requests. The new worst
wall p95 is 1.561 ms, below 2 ms. These measurements isolate the range-policy
change from the later fitted motor changes. Sampled bands and stale-geometry
tolerances remain approximations; the exact-point AI gate is unchanged.

## Countermeasure comparison

Implementation mode, ordnance-fixes from `ba1bf7e4`. The before executable was
frozen before edits. The after executable contains only the AI cadence and
chaff probability changes, with identical flight data and study geometry.
Each build ran 161,408 primary trials: main radar 59,136, reachable radar
78,848, long radar 8,064, scripted IR 12,288 and real-AI 3,072. Main scripted
cells use 64 seeds; AI cells use 32. The same seeds repeat across cells, so
pooled rates are descriptive, not independent confidence estimates. `hit`
means the actual ordnance hit event, not later aircraft-collision HP loss.

The comparison in this section measured the first implementation, where
the global 1.30 multiplier acted on the former effective whole-percent chaff
chance (1% became 1.30%). The lead then chose the unfloored chance; see
[unfloored chaff multiplier](#unfloored-chaff-multiplier) below for the
current values and a matched sample.
IR probabilities and scripted non-CM controls are unchanged, and all paired
scripted IR outcomes and non-CM control rows matched exactly. The AI controller
now spaces pairs across separate two-second evaluation episodes.

Each main row below pools head/beam/tail aspects and short/medium ranges,
384 trials per build. `cm-ai` is the scripted release schedule, not the real
AI controller. `beam+cm-ai` adds the scripted turn. Down is the look-down
geometry; its beam rows exercise notch plus chaff.

| Store | Geometry/profile | N each | Hit % before / after | Decoy % before / after | Decoyed then hit % before / after |
| --- | --- | ---: | ---: | ---: | ---: |
| AA10.JT | co / beam+cm-ai | 384 | 73.18 / 69.27 | 18.23 / 22.92 | 0.00 / 0.00 |
| AA10.JT | co / cm-ai | 384 | 67.45 / 64.58 | 17.97 / 22.40 | 1.82 / 2.34 |
| AA10.JT | co / cm-hammer | 384 | 64.58 / 63.02 | 22.66 / 24.74 | 2.08 / 2.08 |
| AA10.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 |
| AA10.JT | down / cm-ai | 384 | 11.46 / 10.68 | 3.39 / 4.17 | 0.00 / 0.00 |
| AA12.JT | co / beam+cm-ai | 384 | 88.80 / 87.24 | 3.12 / 5.21 | 0.00 / 0.00 |
| AA12.JT | co / cm-ai | 384 | 97.40 / 95.31 | 3.12 / 5.21 | 0.52 / 0.52 |
| AA12.JT | co / cm-hammer | 384 | 97.14 / 97.14 | 3.12 / 3.12 | 0.26 / 0.26 |
| AA12.JT | down / beam+cm-ai | 384 | 6.25 / 6.25 | 2.60 / 3.12 | 0.00 / 0.00 |
| AA12.JT | down / cm-ai | 384 | 96.61 / 94.53 | 3.39 / 5.73 | 0.00 / 0.26 |
| AIM120.JT | co / beam+cm-ai | 384 | 96.88 / 95.31 | 3.12 / 4.95 | 0.00 / 0.26 |
| AIM120.JT | co / cm-ai | 384 | 97.40 / 95.57 | 3.12 / 5.21 | 0.52 / 0.78 |
| AIM120.JT | co / cm-hammer | 384 | 97.14 / 97.14 | 3.12 / 3.12 | 0.26 / 0.26 |
| AIM120.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 |
| AIM120.JT | down / cm-ai | 384 | 48.44 / 47.40 | 1.56 / 2.60 | 0.00 / 0.00 |
| AIM7.JT | co / beam+cm-ai | 384 | 81.77 / 77.34 | 18.75 / 23.44 | 0.52 / 0.78 |
| AIM7.JT | co / cm-ai | 384 | 79.17 / 75.26 | 18.75 / 23.44 | 2.60 / 3.12 |
| AIM7.JT | co / cm-hammer | 384 | 76.56 / 74.22 | 21.35 / 23.96 | 1.82 / 2.08 |
| AIM7.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 | 1.56 / 2.34 | 0.00 / 0.00 |
| AIM7.JT | down / cm-ai | 384 | 5.99 / 5.99 | 5.73 / 7.03 | 0.00 / 0.00 |
| AIM7E.JT | co / beam+cm-ai | 384 | 72.66 / 59.11 | 30.21 / 43.75 | 2.86 / 2.86 |
| AIM7E.JT | co / cm-ai | 384 | 63.28 / 52.86 | 26.82 / 39.58 | 3.12 / 3.91 |
| AIM7E.JT | co / cm-hammer | 384 | 57.81 / 49.74 | 37.24 / 47.66 | 4.43 / 5.21 |
| AIM7E.JT | down / beam+cm-ai | 384 | 0.26 / 0.00 | 10.68 / 14.84 | 0.00 / 0.00 |
| AIM7E.JT | down / cm-ai | 384 | 6.51 / 5.21 | 11.72 / 16.15 | 0.00 / 0.00 |
| MICA.JT | co / beam+cm-ai | 384 | 96.88 / 95.05 | 3.12 / 4.95 | 0.00 / 0.00 |
| MICA.JT | co / cm-ai | 384 | 97.40 / 95.57 | 3.12 / 4.95 | 0.52 / 0.52 |
| MICA.JT | co / cm-hammer | 384 | 97.14 / 97.14 | 3.12 / 3.12 | 0.26 / 0.26 |
| MICA.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 | 0.26 / 0.52 | 0.00 / 0.00 |
| MICA.JT | down / cm-ai | 384 | 55.99 / 54.69 | 2.08 / 3.39 | 0.26 / 0.26 |
| R27ER.JT | co / beam+cm-ai | 384 | 66.93 / 63.54 | 17.71 / 22.40 | 0.00 / 0.00 |
| R27ER.JT | co / cm-ai | 384 | 64.06 / 61.20 | 17.45 / 21.88 | 2.60 / 3.12 |
| R27ER.JT | co / cm-hammer | 384 | 60.68 / 59.11 | 22.66 / 24.74 | 2.08 / 2.08 |
| R27ER.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 |
| R27ER.JT | down / cm-ai | 384 | 13.02 / 12.24 | 2.86 / 3.65 | 0.00 / 0.00 |

Real-AI controller results, 128 trials per row per build, pooling short/medium
and head/tail at co-altitude:

| Store | Skill | N each | Hit % before / after | Decoy % before / after | Decoyed then hit % before / after | Mean devices before / after | Mean rolls before / after |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| AA11.JT | novice | 128 | 60.16 / 60.16 | 23.44 / 23.44 | 8.59 / 8.59 | 1.83 / 1.83 | 0.92 / 0.92 |
| AA12.JT | novice | 128 | 99.22 / 96.09 | 0.00 / 3.12 | 0.00 / 0.00 | 5.50 / 5.45 | 2.75 / 2.73 |
| AIM120.JT | novice | 128 | 100.00 / 96.88 | 0.00 / 3.12 | 0.00 / 0.00 | 5.50 / 5.45 | 2.75 / 2.73 |
| AIM7.JT | novice | 128 | 85.94 / 83.59 | 15.62 / 18.75 | 2.34 / 3.12 | 4.64 / 4.62 | 2.36 / 2.34 |
| AIM9M.JT | novice | 128 | 81.25 / 81.25 | 19.53 / 19.53 | 0.78 / 0.78 | 2.13 / 2.13 | 1.00 / 1.00 |
| AIM9X.JT | novice | 128 | 96.88 / 96.88 | 3.12 / 3.12 | 0.00 / 0.00 | 2.05 / 2.05 | 1.00 / 1.00 |
| AA11.JT | average | 128 | 57.81 / 52.34 | 18.75 / 24.22 | 1.56 / 1.56 | 3.57 / 2.56 | 0.96 / 1.01 |
| AA12.JT | average | 128 | 97.66 / 94.53 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.94 | 2.75 / 2.97 |
| AIM120.JT | average | 128 | 100.00 / 96.88 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.90 | 2.75 / 2.97 |
| AIM7.JT | average | 128 | 86.72 / 81.25 | 13.28 / 18.75 | 0.00 / 0.00 | 5.37 / 5.15 | 2.23 / 2.55 |
| AIM9M.JT | average | 128 | 80.47 / 68.75 | 19.53 / 41.41 | 0.00 / 10.16 | 4.32 / 3.99 | 1.00 / 1.80 |
| AIM9X.JT | average | 128 | 96.88 / 96.09 | 3.12 / 5.47 | 0.00 / 1.56 | 4.03 / 4.00 | 1.00 / 1.97 |
| AA11.JT | experienced | 128 | 56.25 / 46.88 | 28.12 / 39.84 | 3.12 / 5.47 | 5.07 / 4.01 | 1.63 / 1.75 |
| AA12.JT | experienced | 128 | 96.09 / 92.97 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.94 | 2.75 / 2.97 |
| AIM120.JT | experienced | 128 | 100.00 / 96.88 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.94 | 2.75 / 2.97 |
| AIM7.JT | experienced | 128 | 85.94 / 82.03 | 14.06 / 18.75 | 0.00 / 0.78 | 5.78 / 5.22 | 2.36 / 2.55 |
| AIM9M.JT | experienced | 128 | 72.66 / 58.59 | 27.34 / 43.75 | 0.00 / 2.34 | 5.94 / 4.73 | 1.80 / 2.02 |
| AIM9X.JT | experienced | 128 | 96.88 / 94.53 | 3.12 / 5.47 | 0.00 / 0.00 | 6.00 / 5.02 | 1.97 / 2.45 |
| AA11.JT | ace | 128 | 66.41 / 46.09 | 20.31 / 40.62 | 0.00 / 0.00 | 5.41 / 4.25 | 1.11 / 1.88 |
| AA12.JT | ace | 128 | 95.31 / 92.19 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.94 | 2.41 / 2.97 |
| AIM120.JT | ace | 128 | 100.00 / 96.88 | 0.00 / 3.12 | 0.00 / 0.00 | 6.00 / 5.94 | 2.70 / 2.97 |
| AIM7.JT | ace | 128 | 89.06 / 81.25 | 10.94 / 18.75 | 0.00 / 0.00 | 5.78 / 5.34 | 2.12 / 2.59 |
| AIM9M.JT | ace | 128 | 75.78 / 59.38 | 24.22 / 43.75 | 0.00 / 3.12 | 5.91 / 5.44 | 1.38 / 2.31 |
| AIM9X.JT | ace | 128 | 96.09 / 95.31 | 3.91 / 4.69 | 0.00 / 0.00 | 6.00 / 5.61 | 1.34 / 2.72 |

The requested 30% buff is retained. Co-altitude scheduled chaff reduces active
radar hit rates by about two percentage points. Early SARH is more affected:
AIM7E falls from 63.28% to 52.86%. Active radar hammer results remain 97.14%
in these matched seeds; extra cartridges in one episode do not add rolls.
Look-down notch is still the dominant defense. There is no approved target
hit rate, so these measurements do not establish that any larger multiplier
would be better. The current increase does not obviously break the tested
balance. If stronger chaff-only defense is wanted, a separately approved 1.50
comparison would be a useful next experiment, not a validated recommendation.

For IR, Ace AIM9M hit rate falls from 75.78% to 59.38%, with mean evaluation
rolls rising from 1.38 to 2.31 while devices fall from 5.91 to 5.44. Ace AA11
falls from 66.41% to 46.09%. These results support the cadence fix; the small
AI sample does not establish a strictly monotonic skill curve. Full paired
CSV data, isolated per-skill traces and analysis remain in the local
`ordnance-fixes/` task folder. [Cadence evidence](ai-defense.md#current-countermeasure-timing-validation).

The final motor-data rerun adds 24,448 trials for the affected AA12 radar and
AA8 IR matrices, including all four AA12 AI skills. Geometry/seed keys and
initial ranges match the before and countermeasure-only builds. The following
rows separate the defense changes from the final fitted flight data:

| Store | Geometry/profile | N each | Hit % before / CM-only / final | Decoy % before / CM-only / final | Decoyed then hit % before / CM-only / final |
| --- | --- | ---: | ---: | ---: | ---: |
| AA12.JT | co / cm-ai | 384 | 97.40 / 95.31 / 95.57 | 3.12 / 5.21 / 4.95 | 0.52 / 0.52 / 0.52 |
| AA12.JT | co / cm-hammer | 384 | 97.14 / 97.14 / 97.14 | 3.12 / 3.12 / 3.12 | 0.26 / 0.26 / 0.26 |
| AA12.JT | co / beam+cm-ai | 384 | 88.80 / 87.24 / 95.31 | 3.12 / 5.21 / 5.21 | 0.00 / 0.00 / 0.52 |
| AA12.JT | down / cm-ai | 384 | 96.61 / 94.53 / 91.67 | 3.39 / 5.73 / 6.77 | 0.00 / 0.26 / 0.26 |
| AA12.JT | down / beam+cm-ai | 384 | 6.25 / 6.25 / 24.48 | 2.60 / 3.12 / 3.91 | 0.00 / 0.00 / 0.00 |
| AA8.JT | co / cm-ai | 384 | 11.72 / 11.72 / 33.07 | 61.20 / 61.20 / 73.96 | 7.29 / 7.29 / 8.85 |
| AA8.JT | co / cm-hammer | 384 | 17.19 / 17.19 / 38.02 | 68.23 / 68.23 / 68.49 | 5.99 / 5.99 / 7.55 |
| AA8.JT | co / beam+cm-ai | 384 | 2.08 / 2.08 / 25.26 | 80.21 / 80.21 / 76.04 | 1.30 / 1.30 / 1.30 |
| AA8.JT | down / cm-ai | 384 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 |
| AA8.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 |

For real-AI AA12 cases, final hit rates are 96.88% at each of the four skills,
with 3.12% decoy and no decoyed-then-hit in 128 shots per skill. The original
rates were 99.22%, 97.66%, 96.09% and 95.31% respectively. Stronger flight
increases some off-axis hit rates even with the chaff buff: co-altitude
beam+chaff rises to 95.31%, and look-down beam+chaff to 24.48%. AA8's increased
reach likewise raises its co-altitude scheduled-flare hit rate to 33.07% from
11.72%. Its IR probability equation is unchanged. These are intended flight
balance effects, not evidence of extra chaff or flare probability changes.


### Unfloored chaff multiplier

Lead decision, 2026-10-27: `CHAFF_EFFECTIVENESS_MULTIPLIER = 1.30` now acts on
the unfloored radar chance before the basis-point roll. A zero-notch active
seeker at effectiveness 35 rolls 1.99% for AA12 (1.53% before) and 1.59% for
AIM120 (1.23% before), and chances below 1% are buffed as well. Flares and the
Spec/Compatibility evaluators are unchanged.

The matched sample below uses the scripted schedules (not the real AI), 64
seeds per cell, short/medium ranges and head/beam/tail aspects pooled, 384
trials per row. Base is `ba1bf7e4`, built independently; "floored" is the
first implementation in this branch; "unfloored" is the current rule. AA12
rows also include its new motor fit, so its base column is not a chaff-only
comparison; AA12, AIM7 and R27ER were not rerun with the floored rule.

| Store | Geometry/profile | N | Hit % base / floored x1.3 / unfloored x1.3 | Decoy % base / floored / unfloored | Decoyed then hit % base / floored / unfloored |
| --- | --- | ---: | ---: | ---: | ---: |
| AA10.JT | co / beam+cm-ai | 384 | 73.18 / 69.27 / 69.27 | 18.23 / 22.92 / 22.92 | 0.00 / 0.00 / 0.00 |
| AA10.JT | co / cm-ai | 384 | 67.45 / 64.58 / 64.58 | 17.97 / 22.40 / 22.40 | 1.82 / 2.34 / 2.34 |
| AA10.JT | co / cm-hammer | 384 | 64.58 / 63.02 / 63.02 | 22.66 / 24.74 / 24.74 | 2.08 / 2.08 / 2.08 |
| AA10.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 |
| AA10.JT | down / cm-ai | 384 | 11.46 / 10.68 / 10.68 | 3.39 / 4.17 / 4.17 | 0.00 / 0.00 / 0.00 |
| AA10.JT | down / cm-hammer | 384 | 10.94 / 10.68 / 10.68 | 16.15 / 17.19 / 17.19 | 0.00 / 0.00 / 0.00 |
| AA12.JT | co / beam+cm-ai | 384 | 88.80 / - / 92.45 | 3.12 / - / 8.33 | 0.00 / - / 0.78 |
| AA12.JT | co / cm-ai | 384 | 97.40 / - / 92.97 | 3.12 / - / 8.07 | 0.52 / - / 1.04 |
| AA12.JT | co / cm-hammer | 384 | 97.14 / - / 95.57 | 3.12 / - / 4.69 | 0.26 / - / 0.26 |
| AA12.JT | down / beam+cm-ai | 384 | 6.25 / - / 23.44 | 2.60 / - / 5.47 | 0.00 / - / 0.00 |
| AA12.JT | down / cm-ai | 384 | 96.61 / - / 88.80 | 3.39 / - / 9.90 | 0.00 / - / 0.26 |
| AA12.JT | down / cm-hammer | 384 | 95.57 / - / 91.67 | 4.43 / - / 6.25 | 0.00 / - / 0.00 |
| AIM120.JT | co / beam+cm-ai | 384 | 96.88 / 95.31 / 93.75 | 3.12 / 4.95 / 6.51 | 0.00 / 0.26 / 0.26 |
| AIM120.JT | co / cm-ai | 384 | 97.40 / 95.57 / 94.53 | 3.12 / 5.21 / 6.77 | 0.52 / 0.78 / 1.30 |
| AIM120.JT | co / cm-hammer | 384 | 97.14 / 97.14 / 97.14 | 3.12 / 3.12 / 3.12 | 0.26 / 0.26 / 0.26 |
| AIM120.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 | 0.00 / 0.00 / 0.00 |
| AIM120.JT | down / cm-ai | 384 | 48.44 / 47.40 / 46.61 | 1.56 / 2.60 / 3.39 | 0.00 / 0.00 / 0.00 |
| AIM120.JT | down / cm-hammer | 384 | 48.44 / 48.44 / 48.18 | 1.56 / 1.56 / 1.82 | 0.00 / 0.00 / 0.00 |
| AIM7.JT | co / beam+cm-ai | 384 | 81.77 / - / 77.34 | 18.75 / - / 23.44 | 0.52 / - / 0.78 |
| AIM7.JT | co / cm-ai | 384 | 79.17 / - / 75.26 | 18.75 / - / 23.44 | 2.60 / - / 3.12 |
| AIM7.JT | co / cm-hammer | 384 | 76.56 / - / 74.22 | 21.35 / - / 23.96 | 1.82 / - / 2.08 |
| AIM7.JT | down / beam+cm-ai | 384 | 0.00 / - / 0.00 | 1.56 / - / 2.34 | 0.00 / - / 0.00 |
| AIM7.JT | down / cm-ai | 384 | 5.99 / - / 5.99 | 5.73 / - / 7.03 | 0.00 / - / 0.00 |
| AIM7.JT | down / cm-hammer | 384 | 3.91 / - / 3.65 | 16.15 / - / 17.19 | 0.00 / - / 0.00 |
| AIM7E.JT | co / beam+cm-ai | 384 | 72.66 / 59.11 / 58.07 | 30.21 / 43.75 / 44.79 | 2.86 / 2.86 / 2.86 |
| AIM7E.JT | co / cm-ai | 384 | 63.28 / 52.86 / 52.34 | 26.82 / 39.58 / 40.36 | 3.12 / 3.91 / 4.17 |
| AIM7E.JT | co / cm-hammer | 384 | 57.81 / 49.74 / 49.74 | 37.24 / 47.66 / 47.66 | 4.43 / 5.21 / 5.21 |
| AIM7E.JT | down / beam+cm-ai | 384 | 0.26 / 0.00 / 0.00 | 10.68 / 14.84 / 15.10 | 0.00 / 0.00 / 0.00 |
| AIM7E.JT | down / cm-ai | 384 | 6.51 / 5.21 / 5.21 | 11.72 / 16.15 / 16.41 | 0.00 / 0.00 / 0.00 |
| AIM7E.JT | down / cm-hammer | 384 | 3.39 / 2.08 / 2.08 | 25.78 / 30.21 / 30.21 | 0.00 / 0.00 / 0.00 |
| MICA.JT | co / beam+cm-ai | 384 | 96.88 / 95.05 / 91.93 | 3.12 / 4.95 / 8.07 | 0.00 / 0.00 / 0.00 |
| MICA.JT | co / cm-ai | 384 | 97.40 / 95.57 / 92.97 | 3.12 / 4.95 / 8.07 | 0.52 / 0.52 / 1.04 |
| MICA.JT | co / cm-hammer | 384 | 97.14 / 97.14 / 95.57 | 3.12 / 3.12 / 4.69 | 0.26 / 0.26 / 0.26 |
| MICA.JT | down / beam+cm-ai | 384 | 0.00 / 0.00 / 0.00 | 0.26 / 0.52 / 0.52 | 0.00 / 0.00 / 0.00 |
| MICA.JT | down / cm-ai | 384 | 55.99 / 54.69 / 52.86 | 2.08 / 3.39 / 5.21 | 0.26 / 0.26 / 0.26 |
| MICA.JT | down / cm-hammer | 384 | 55.73 / 55.73 / 55.47 | 2.08 / 2.08 / 2.34 | 0.00 / 0.00 / 0.00 |
| R27ER.JT | co / beam+cm-ai | 384 | 66.93 / - / 63.54 | 17.71 / - / 22.40 | 0.00 / - / 0.00 |
| R27ER.JT | co / cm-ai | 384 | 64.06 / - / 61.20 | 17.45 / - / 21.88 | 2.60 / - / 3.12 |
| R27ER.JT | co / cm-hammer | 384 | 60.68 / - / 59.11 | 22.66 / - / 24.74 | 2.08 / - / 2.08 |
| R27ER.JT | down / beam+cm-ai | 384 | 0.00 / - / 0.00 | 0.00 / - / 0.00 | 0.00 / - / 0.00 |
| R27ER.JT | down / cm-ai | 384 | 13.02 / - / 12.24 | 2.86 / - / 3.65 | 0.00 / - / 0.00 |
| R27ER.JT | down / cm-hammer | 384 | 11.98 / - / 11.72 | 16.15 / - / 17.19 | 0.00 / - / 0.00 |

Older SARH stores barely move from the floored rule because their chances
were already several percent. The active-radar stores now show a real
chaff effect at co-altitude: MICA `cm-ai` decoys rise from 3.12% to 8.07% and
hits fall from 97.40% to 92.97%. On 2026-10-28 John doubled the game-balance
value to 2.60; its chaff-only comparison is recorded separately.
