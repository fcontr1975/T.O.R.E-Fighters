# Startup diagnostics validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. Validation on Linux, 2026-09-23, for the changes based on
`fb5cbd4504a9ee900cf3fa3d58dbd67eb6819df4`, including the Windows
live-log correction after the initial `42d7757` CI run. The authored behavior is in the
[startup diagnostics contract](../spec/startup-diagnostics.md); this is not
retail parity evidence. Local artifacts and raw logs stay under ignored
`.local/startup-diagnostics-validation/` and `dist/`.

## Observed results

The required workspace formatting, Clippy with warnings denied, tests, build,
Python tool tests, documentation check and source/debug executable asset scans
passed. New logger tests cover unique session paths, bounded routine output,
UTF-8 truncation, storage failures, lock contention and preserving live sessions
during retention. Tool tests include the Windows message-resource layout, which
Cargo does not discover automatically in a build script.

The real debug executable passed the media-free startup checker: success,
returned error, main-thread panic and worker panic, with a subsequent success
after each failure. Reports retained build identity, thread/backtrace details
and the most recent compatibility error. Profiles and log directories contained
spaces. The Linux runtime dependency gate passed for both executables.

A failed explicit log directory used the temporary fallback and reported paths
to files that existed. Blocking both destinations still returned a nonzero
status, printed the error to stderr and preserved the compatibility summary in
a writable data directory.

On the available NVIDIA GeForce RTX 4070 / Vulkan desktop, the synthetic graphics
check presented a first-run canvas and exited successfully. The normal
`cargo run --locked -p tore-app -- --smoke-test --no-audio` passed with
`TORE_DATA_DIR=.local/dev-profile`; its session log records adapter and driver,
all graphics stages, aircraft preparation and the first presented frame.
Existing backend warnings and an occupied optional head-tracker port were
reported without changing success. Removing display environment variables
caused the synthetic graphics check to exit 1 and write the failed
`diagnostic event loop creation` stage and the winit error into its reports.
The isolated 1,200-tick headless flight check also passed without a display
requirement or dialog. The deliberate Linux `=dialog` check returned exit 1,
saved its fatal summary and successfully submitted a desktop notification; the
notification's on-screen appearance was not inspected.

Full application `cargo check --locked -p tore-app` passed for
`x86_64-pc-windows-gnu`, `x86_64-pc-windows-msvc` and
`aarch64-apple-darwin`. The MSVC build script generated resources with five
icons, the group icon and event message table, verified by the existing resource
parser. These are compile checks, not native linking or OS runtime acceptance.

A release build stamped `0.1.0-diagnostics-check` and Linux packaging completed.
The staged binaries, recovered tar.gz executable and extracted AppImage launcher
passed startup checks, including deliberate errors and panic reports. Source,
binary, staged and package asset guards passed. Matching release debug data is
enabled; the workflow retains platform symbols as separate artifacts.

The first [GitHub package run](https://github.com/john-overton/T.O.R.E-Fighters/actions/runs/35910079610)
passed Linux and Apple Silicon builds and package checks. Windows reached the
logger tests and exposed error 33 when another handle read a log held under a
Windows byte-range lock. Session ownership now uses a separate sidecar, with a
regression that reads a live log and verifies retention still protects it.
This corrects a portability issue that Linux file-lock behavior did not expose.
The Intel Mac and corrected Windows native runs were still pending when this
fix was committed.

## Windows results for issue #8

Package 0.2.2-rc.5, commit `c461b4b3`,
[run 41537628612](https://github.com/john-overton/T.O.R.E-Fighters/actions/runs/41537628612).

**On the CI Windows VM, 2026-11-02.** It has no Vulkan and draws DirectX 12 on the
Microsoft Basic Render Driver. These passed: MSI install and uninstall; DirectX 12
chosen by default; the Vulkan failure wording; both native crash self-tests (exit
codes `-1073741819` and `-1073741571`, each with a fatal report and readable Event
Viewer entries); a 64 s smoke test; the import screen and Pref > Graphics choice;
the prompt after a dead launch, including a real kill; no prompt after a clean close
or in unattended runs; the `--graphics-api` flag; a saved choice on restart; and a
normal fullscreen launch through the prompt to the menu.

**On AMD, reported by the player in
[issue #8](https://github.com/john-overton/T.O.R.E-Fighters/issues/8#issuecomment-6602933295).**
Windows 10 LTSC IoT, RX 7600, Adrenalin 26.10.1. rc.2 crashed in `amdvlk64.dll`
compiling `City fill surfaces fading in`, the first pipeline to write the sample
mask. On 2026-11-03 the player reported rc.5 "working perfectly" under Vulkan, with a
screenshot of a flight showing terrain, trees, the HUD and the panels. The player
attached no log, and the screenshot shows no city.

Not measured: Vulkan on Windows on other AMD cards or drivers, and DirectX 12 on any
real graphics card. After the [naga patch](../../crates/vendor/naga/PATCHED.md),
`spirv-val` still rejects seven entry points, none of them on the sample mask: five
on atomics semantics (`fill_cull.cull` and `finish`, `simulation.blade_cull`,
`vegetation_cull.cull` and `finish`) and two on workgroup stride
(`sun_visibility.measure` and `measure_ms`). They have caused no reported failure.

## Remaining acceptance

Native Windows MessageBox and macOS AppKit alert behavior, MSI upgrade, both macOS
DMGs and Finder launches, and Windows shortcut launches must still be checked on those
systems. CI validates native builds and package payloads; follow-up native results
are available through the package run associated with the tested source commit. Linux desktop-menu installation and AppImage mounting on another
machine are also not proven by extracting and launching the payload here.

OS launch blocking and missing loader dependencies are outside Rust panic reporting.
No assertion is made about the cause of John's
reported Windows startup failure. No retail data was added to source or
packages, and no automatic report upload or crash-dump policy was enabled.
