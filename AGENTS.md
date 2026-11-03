# Agent instructions

This is the authoritative rules file for automated contributors. Agent behaviour
rules live here and nowhere else. Anything longer than a sentence or two belongs
in a linked document.

## Project intent

T.O.R.E-Fighters is a ground-up rebuild of Jane's Fighters Anthology in Rust.
The target is 1:1 gameplay parity **by expression of feature**: a player must
experience the same behaviour they experience in the original game. It is not a
recreation of the original program's code, control flow or internal structure.
Players bring their own retail copy; no retail bytes are committed to this
repository, and original modules are never executed, readers only interpret
reviewed, bounded data grammars. Sequencing lives in
[docs/ROADMAP.md](docs/ROADMAP.md); current parity status lives in
[docs/parity-plan.md](docs/parity-plan.md).

## The two modes of work

Every task is either research or implementation. Know which one you are in and
say so in your report.

### Research mode

Recovering behaviour from the original executable and media.

- Output is a **prose specification** in [docs/spec/](docs/spec/) describing what
  the game does, with the numbers a player would notice.
- Provenance is strict: record the build identity, the evidence and the
  unresolved branches. Naming patterns, screenshots and real-world aircraft
  knowledge do not establish original behaviour on their own.
- Do not guess. A missing fact is recorded as unknown with the next research
  step, never filled in with something plausible.
- Stop when a spec can be written. Full byte-level closure of a routine is not
  required, and is not the goal.

### Implementation mode

Turning a spec into working game behaviour.

- Read the spec first. Implement the described behaviour idiomatically in the
  existing crates, the way good Rust in this repository is already written.
- Test against the spec's numbers.
- Do **not** translate the original's control flow, call ordering, caches or RNG
  ordering. Those are implementation details of a 1990s DOS program, not
  player-visible behaviour.
- If the spec is missing a number you need, that is a research task. Say so,
  choose a documented value, and label it `fitted` or `opinionated`.

## Terms

- **Original behaviour** / **game behaviour**, what a player experiences in
  Fighters Anthology, described in prose in `docs/spec/`. This is the parity
  target.
- **Native**, a code path reconstructed from the original executable's control
  flow. This is a **provenance label only**. It is never a requirement, an
  acceptance gate, or a reason to block or revert working behaviour.
- When John says "match the original", "do what the game does", or "native
  functionality", he means **original behaviour**, not native provenance.
- If an instruction is ambiguous between the two, assume original behaviour and
  say so in your report.

## Spec granularity test

> Would a player notice if this were different?

Yes means it belongs in the spec, with numbers. No means it is a source-notes
footnote, or it is omitted.

## Provenance categories

Label components, not whole features; a feature may mix several origins.

- **spec-derived**, implemented from a behaviour spec in `docs/spec/`. **This is
  the default for gameplay code from now on.**
- **native**, reconstructed from the original executable's control flow. A
  description of where the behaviour came from, nothing more.
- **fitted**, an approximation authored because a behaviour is not yet
  specified, or because the host needs something the original did not have.
  Record the rule, the constants and the known difference.
- **opinionated**, a deliberate design choice, either requested by John or
  chosen by an agent. Record which, and the date if it was requested.
- **unknown**, insufficient evidence. Record the missing evidence and the next
  research step.

A `fitted` or `opinionated` component is acceptable as shipped behaviour. It does
not have to be replaced by a `native` one before acceptance. See
[behaviour provenance](docs/behavior-provenance.md).

## Standing constraints

- **No AI or autonomous behaviour work** unless John explicitly requests it.
- **Exact aircraft identities:** `F18.PT` is the F/A-18D and `RAFALE.PT` is the
  Rafale C. Never alias `F18C`, `RAFALEE` or `RAFALEF`, and never substitute a
  variant to make a test pass.
- **Retail comparison is unavailable** and is not a blocker. Do not turn that
  limitation into a claim of retail parity either.
- **Do not silently change default adapters or remove compatibility modes.**
  The legacy `--legacy-flight` path, the hybrid `--researched-flight` default
  (requested by John on 2026-09-16) and the restricted
  `--native-flight-tables` research path stay distinct.
- **Report to Jeeves at milestones.** The PM control file was retired on
  2026-09-16 and is being rebuilt; until it exists, report milestones in the
  session.
- **Agent decisions are recorded as agent decisions.** Never attribute an
  implementation choice to John because he requested the broader feature.
- Keep dependencies small: `winit`, `wgpu`, `pollster`, `cpal`. `tore-formats`
  has no dependencies. Formats, simulation and synthesis stay independent of the
  renderer.
- Preserve Linux, Windows and macOS support, deterministic headless execution,
  and fixed 120 Hz simulation independent of rendering.
- **Grounding:** every ground object, unit, man and sprite stands on the
  bottom of its shape through `tore_world::grounding`; never ground an object
  any other way ([grounding](docs/formats/objects-and-shapes.md#grounding-every-object-stands-on-the-bottom-of-its-shape)).
- Never commit retail media, extracted art/audio/fonts or generated retail
  derivatives. `gameassets/`, `USNF-ATF/`, `.local/` and `target/` are local
  only. Use synthetic fixtures in committed tests.
- Never embed retail bytes with `include_bytes!` or build scripts. The importer
  reads user-owned media at runtime.
- `USNF-ATF/` is an ignored reference checkout. Use its recovered specifications
  and baselines, never its engine, its TypeScript runtime or its custom terrain
  system.
- Reuse original art, button pieces and fonts. The USNF-ATF menu has custom
  controls and is not the visual specification.
- Report honestly: state what was validated, what was not run, and what is still
  approximate. Do not commit or push unless asked.
- Summarize in plain English for a smart product manager: short sentences, no
  unexplained jargon, lead with what it means rather than how it works.
- **No em dashes.** Use a comma, a colon, or a new sentence. This applies to
  documentation, commit messages, and reports. `tools/check_docs.py` does not
  enforce it; keep to it anyway.

## Documentation rules

| Content | Home |
| --- | --- |
| Research facts: formats, byte layouts, decoded contracts | `docs/formats/` |
| Measured evidence: what was run, on what, with what result | `docs/baselines/` |
| Testing: how the battery and probes work, lanes, adding scenarios | `docs/testing/` |
| Behaviour specs: what a player experiences, with numbers | `docs/spec/` |
| Planning: milestones | `docs/ROADMAP.md` |
| Planning: current parity status and next feature (one page) | `docs/parity-plan.md` |
| Frozen research archives, kept for their evidence | `docs/research/` |

- **Nothing else grows a revision log.** `docs/research/` holds the frozen ones.
- **No per-predicate baseline files.** One baseline per feature or per validation
  pass, not one per recovered routine. This governs new baselines; the existing
  `baselines/native-strip-*` set predates the rule and is kept as evidence.
- **Link, do not duplicate.** A fact has one home; everything else points at it.
- **Before every commit, review [the feature matrix](docs/features.md)** and update
  affected rows, category checkboxes, status and remaining work in the same change.
  Keep it limited to manual-described features and opinionated additions.
- Keep the guides current in the same change that alters the behaviour they
  document: [aircraft import](docs/aircraft-import.md),
  [flight model](docs/FLIGHT-MODEL.md), [flight controls](docs/FLIGHT-CONTROLS.md),
  [input](docs/INPUT.md), [extraction](docs/EXTRACTION.md),
  [weather formats](docs/formats/weather.md),
  [objects and shapes](docs/formats/objects-and-shapes.md),
  [theater](docs/formats/theater.md), [architecture](docs/ARCHITECTURE.md).
- **Controls master list:** [docs/CONTROLS.md](docs/CONTROLS.md) lists every
  default keyboard key, mouse input and gamepad button. Any change that adds an
  action or changes a default binding updates it in the same change: add the
  action to `crates/tore-app/src/input_catalog.rs` (or change the gamepad
  defaults in `crates/tore-app/src/input.rs`), then run
  `TORE_UPDATE_CONTROLS_DOC=1 cargo test --locked -p tore-app controls_doc`.
  The test fails while the document is stale.
- **Keyboard map:** Updates to `docs/tore-keyboard-map.html` follow the linked
  [keyboard map conventions](docs/tore-keyboard-map-rules.md).
- Correct stale status claims in place. Do not append a success note beneath a
  contradictory summary.

## Prompts for other agents

When you write a task or review prompt for another agent (a subagent, a Codex
or Cursor run, a reviewer), John's rule since 2026-10-26 applies:

- Start every prompt by saying the work is on a video game: "We are building
  T.O.R.E Fighters, a Rust remake of the PC flight sim Jane's Fighters
  Anthology."
- Refer to weapons as ordnance: "A2A ordnance", "A2G ordnance", "stores", or
  the game's own record names (for example "the AIM9M.JT entry", "the MK82
  store"). Do not use real-world weapon category nouns in prompts; the code's
  own identifiers and file names may still be quoted as they are.
- Frame reviews as software quality work on game code: correctness,
  determinism, performance, tests and in-game behaviour. Do not ask an agent to
  research or validate real-world weapon performance.

## Headless development

From the repository root, use `TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --headless-flight 1200 --no-audio`, or the CPU menu snapshot commands in [headless development](docs/DEVELOPMENT.md#headless-development).
`--no-audio` alone does not disable the window; the linked guide covers isolated imports and the dev profile, while GPU captures and `--smoke-test` require a display.
For broad bug hunting run the battery, `python3 tools/battery.py --lane ai`, described in [testing](docs/testing/README.md). Add a scenario there for any bug you fix that a headless or wrapped run can reproduce.
Anything that opens a window (those, `--free-flight`, a plain launch) must go through `tools/agent-run.sh`, for example `tools/agent-run.sh cargo run --locked -p tore-app -- --smoke-test`, so John's active workspace is not covered. Never launch a window bare.

## Development checks

On a fresh clone, run `python3 tools/setup_dev.py` once. It points Git at the
committed `.githooks/` directory, which installs a pre-push hook that runs the
checks below and aborts the push if any fail. Git hooks are not version
controlled, so a clone without that step has no hook at all.

Run these before finishing, from the repository root. Full details and
platform-specific setup are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

While you iterate, run `python3 tools/quick_check.py` (about five minutes: format,
clippy and tests for the crates you touched, and the battery scenarios the change can
affect; see [testing tiers](docs/testing/README.md#testing-tiers)). It is an aid only:
the check list below stays the merge requirement.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
python3 -m unittest discover -s tools -p 'test_*.py'
python3 tools/check_assets.py
python3 tools/check_assets.py target/debug/tore-app
python3 tools/check_assets.py target/debug/tore-extract
python3 tools/check_docs.py
```

Every Markdown file under `docs/` carries the T.O.R.E header under its title.
`tools/check_docs.py` reports files missing it; `--fix` writes them. To reword it,
edit `HEADER` in that script, raise `HEADER_REVISION`, and run `--fix`: the old
block is replaced, never stacked.

Use `rust-toolchain.toml` and keep `Cargo.lock`; always validate with `--locked`.
For rendering changes also run `cargo run --locked -p tore-app -- --smoke-test`
on a display-capable host. Report any check you could not run.
