# Bridge placement and asset audit

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode, 2026-10-25. This follow-up starts from the merged bridge
correction at `80ad1ce2`. The [behavior specification](../spec/bridges.md) owns
the rules and remaining unknowns. The imported profile identifies FA.EXE 1.02F,
SHA-256 `e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c`.
No retail executable was run. Retail visual comparison was unavailable.

## Placement and approaches

The earlier audit found 511 buried pieces among 624 layout placements in 75
layouts. Bottom grounding and shared assembly support corrected the burial.
The follow-up review found that flattening whole corridors to their lowest
sample cut a deep trench beside the Ukraine suspension bridge. It also found
that four destroyed END types kept their intact appearance.

The current audit checks all 75 layouts and four bridge-bearing templates,
including repeated theater scenery: 862 placements in 79 cases. Physical
support is measured at the connected assembly's feet. A suspended MID's
painted shadow is excluded, since it cannot support a bridge. Separate checks
cover road/deck burial, END toe gaps and constructed approach grades. The final
strict run has zero flagged placements and zero missing textures. Its greatest
physical-foot support gap is the existing 8 ft source-part difference.

Only two physical crossings need the new filled-bank treatment, repeated in
Ukraine's alternate layouts and the QUBRI scene. Other bridges keep their
previous origins, source angles and scale. The printed origin comparison
changes 63 repeated placement rows and leaves 799 unchanged.

| Ukraine crossing | Previous maximum corridor cut | New bank support | New deck height | New cut | Maximum measured terrain approach grade |
| --- | ---: | ---: | ---: | ---: | ---: |
| `0x40000082`, large suspension bridge | about 352 ft | 352.94 ft | 920.94 ft | 0 ft | 8.008 percent |
| `0x4000007c` | about 47.5 ft | 47.38 ft | 615.38 ft | 0 ft | 8.001 percent |

The bridge decks retain one assembly height, with filled support beneath both
banks and open river terrain beneath the middle. END road toes retain their
8 ft source clearance. New outward terrain approaches use the specified
8 percent grade. Existing END mesh ramps retain their original steeper shape.
The audit's 8.1 percent acceptance limit allows world-coordinate float rounding.
Side and river-facing earthwork slopes are 1:4, not drivable road approaches.

The before images supplied by the reviewer and the new side, along-corridor
and north-end views are local evidence. Before images are the reviewer's
supplied views; after images use explicit side, along and north camera
presets. Framing differs, so these are visual comparisons, not matched-pixel
tests or retail comparisons. Raised painted shadows are
subdivided and projected onto the terrain so their corners cannot suspend a
flat shadow across the river.

## Assets and destroyed ends

The imported pack contains all twelve bridge types and their textures. All use
structure category `0x0100`, weapon damage class 2, explosion 35 and crater 0.
The following face counts describe source assets, before generated END ruins:

| Type | Shape | HP | Intact / broken faces | Damage branch | Textures |
| --- | --- | ---: | ---: | --- | --- |
| BR1END.OT | BR1END.SH | 500 | 42 / 42 | false | _BR1END.PIC |
| BR1MID.OT | BR1MID.SH | 500 | 18 / 27 | true | _BR1MID.PIC, _BR1MIDD.PIC |
| BR2END.OT | BR2END.SH | 500 | 30 / 30 | false | _BR2END.PIC |
| BR2MID.OT | BR2MID.SH | 200 | 17 / 32 | true | _BR2MID.PIC, _BR2MIDD.PIC |
| BR3END.OT | BR3END.SH | 500 | 36 / 36 | false | _BR3END.PIC |
| BR3MID.OT | BR3MID.SH | 50 | 18 / 19 | true | _BR3MID.PIC, _BR3MIDD.PIC |
| BRD1.OT | BRD1.SH | 400 | 151 / 116 | true | _BRD1.PIC, _BRD1D.PIC |
| BRD2.OT | BRD2.SH | 500 | 171 / 145 | true | _BRD2.PIC, _BRD2D.PIC |
| BRD3.OT | BRD3.SH | 300 | 89 / 73 | true | _BRD3.PIC, _BRD3D.PIC |
| BRD4.OT | BRD4.SH | 600 | 198 / 181 | true | _BRD4.PIC, _BRD4D.PIC |
| BRDEND.OT | BRDEND.SH | 100 | 28 / 28 | false | _BRDEND.PIC |
| BRDMID.OT | BRDMID.SH | 100 | 31 / 12 | true | _BRDMID.PIC, _BRDMIDD.PIC |


The eight span types keep their source damage branches. BR1END, BR2END, BR3END
and BRDEND now use the existing deterministic generated building ruin at zero
HP, as requested by John. Positive HP keeps intact art. The original bottom
and placed datum remain. No persistent bridge fire or smoke was added.

The source has no separate bridge ruin, falling animation or named DEBRIS.SH
reference. No convoy rerouting or river navigation behavior was added. Those
original behaviors remain unknown. Explosion types 35/36/37 and their existing
presentation remain unchanged.

## Evidence and validation

Evidence is kept outside Git in
`/home/john/Development/T.O.R.E-Fighters/.local/next-round/0.2.2-rc1/bridges-followup/`.
The scratch `report.md` records the final command results and capture paths.
`before/placements.tsv` is the unmodified branch baseline. Final audits write
`placements.tsv`, `assets.tsv` and `banks.tsv`. The bank table records cuts and
sampled road grades. No extracted images, source geometry or retail derivatives
are committed. Reusable commands are in [development](../DEVELOPMENT.md).

Synthetic tests cover unchanged flat crossings, filled banks, the 8 percent
terrain grade, shared contact/rendering triangles, shadow projection, all four
END ruins, deterministic geometry and HP/reverse-replay transitions. The
`bridge-audit` battery exercises the grading checks. `bridge-end-ruins` captures
all four END types and requires intact positive-HP art, changed zero-HP art,
nonempty geometry and identical live/replay geometry. `a2g-bridge-destruction`
retains its real Maverick strike and recorded render checks.

Final Linux validation passed: 7,172 Rust tests (107 ignored); workspace
clippy with warnings denied; workspace build; formatting; 505 Python tests
(5 skipped); documentation; repository and app/extract binary asset scans;
and the wrapped GPU smoke test on an NVIDIA RTX 4070. The interrupted test
run was discarded as acceptance evidence and the full workspace was rerun.

All eleven selected battery scenarios passed: `bridge-audit`,
`bridge-end-ruins`, `a2g-bridge-destruction`, `surface-grounding`,
`surface-grounding-sites`, `surface-grounding-radar-render`,
`surface-ground-pads`, `net-surface-loopback`, `ai-ils-terrain-ukr-a8`,
`ai-regress-terrain-f14-dive` and `ai-regress-terrain-f18-hill`.
The pad audit covers 329 worlds and 49,934 placements with zero problems.
The site audit covers all sixteen theaters with zero unread resources or
problems. The Maverick strike matches 1,267 recorded render snapshots, with
zero missing or differing frames. No existing probe expectation was edited.

There are 68 final PNGs in the three angle batches and the END battery
capture. All 29 intact/destroyed pairs differ; all ten captured half-health
views exactly match intact. Live/replay geometry comparisons pass throughout.
Representative captures of all four END types were visually inspected.
Windows/macOS builds and retail visual comparison were not run.

No serialized field, event, protocol version, replay version or checkpoint
version changes. Terrain, bridge origins, contact bounds and target aim points
change for the two sloping crossings. Rebuilding older recordings uses the new
geometry. The integration lead owns the common release compatibility decision.
No golden checksum assertion was changed. END ruins are HP-driven presentation.
