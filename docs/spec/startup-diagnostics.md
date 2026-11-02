# Startup diagnostics

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. **Opinionated** host behavior, requested by John on
2026-09-23: diagnostics built into the executable, visible startup errors on
Windows, and persistent logs on Windows, macOS and Linux. Storage limits,
file names, diagnostic switches and platform interfaces below are agent
decisions. These are not claims about Fighters Anthology behavior.

## Every launch

Logging starts before argument parsing, preferences, media, controllers, audio,
the event loop and graphics. The log records UTC Unix timestamps, elapsed time,
severity, build version, commit identity, target, OS/architecture, executable
location, working directory and resolved data location. It does not dump the
environment, media contents or frame-by-frame simulation state.

Startup stages record a start and successful completion with elapsed time.
They cover settings, asset loading/import, aircraft, audio, terrain, menus,
input, window creation, graphics adapter/device and first presentation. A
failure identifies the unfinished stage and the available error cause chain.
Recoverable failures, including unavailable audio, remain recoverable warnings.
No graphics or flight adapter selection changes as a side effect of logging.

## GPU setup steps and the watchdog

**Opinionated**, requested by John on 2026-11-02 after issue #8, where a driver
died during `Renderer::new` and the log could not say on what. The line budget,
threshold and intervals are agent decisions, labeled **fitted** below.

The game names each driver-reaching call before making it: every shader module
and render or compute pipeline it creates, whenever that happens (first-run
canvas, game renderer, lens flare, vegetation, city fill, smoke, effects, CRT,
cockpit, UI text, and the rebuilds after an anti-aliasing change or theater
load), plus the instance, surface, adapter, device and surface configuration
steps. All of them go through `crates/tore-app/src/gpu_setup.rs`, and a Clippy
rule rejects direct wgpu calls outside tests. A line reads
`GPU setup: render pipeline 'Menu'`. The adapter's name, IDs, backend, type,
driver and driver info get a line of their own before the device is requested,
which the later `Session system` record repeats.

Each line reaches the OS before the call. The session log has no buffer in the
process: each record is a direct file write, so it survives the process crashing
on the next instruction (not a power cut). A step line waits up to 100 ms for
another thread's log write instead of dropping out, as an ordinary record would.

A call that takes 1 s or more (**fitted**) gets a timing line, because a slow
driver compile is evidence of its own. A session writes at most 2000 step lines
(**fitted**; about 65 per launch, measured on Linux), then one line saying so.
That bounds them to about 200 KiB of the 5 MiB routine log. Crash reports and the
watchdog keep naming the current step past the budget.

A watchdog thread tells a hang from a crash. While a stage or a GPU step outside
a stage has been running long enough, it writes
`Watchdog: still in stage 'X' after 30 s; last GPU step: in render pipeline 'Menu' (running 29 s)`.
The first line comes at 30 s and each later one after twice the wait, for at most
8 lines per stage: 30 s, 1, 2, 4, 8, 16, 32 and 64 minutes (**fitted**). A process
that hung keeps saying so; one that crashed stops. A stage that legitimately waits
on the player, such as the first-run import screen, spends those 8 lines and stops.
The watchdog only logs. It never kills the process or opens a dialog, it does not
touch the simulation, and it does not keep the process alive at exit. It stops
once the process is reporting a failure.

## Graphics session and slow frames

**Opinionated**, requested by John on 2026-10-25. Rate limits, threshold,
summary bins and phase boundaries are agent decisions. No simulation behavior,
flight adapter or graphics adapter selection changes.

Before configuring the game surface, one `Session system` record identifies the
adapter name, vendor/device IDs, backend, device type, driver and driver info,
surface format and present mode, window pixels and scale, logical CPU cores,
OS/architecture, graphics options, frame limit and shared worker pool sizing.
The pool is lazy: its configured worker count is not a claim that those threads
are currently active. Changes to graphics/presentation are logged separately.
A CPU adapter produces a software-rendering warning with driver and Windows
High performance advice. An integrated adapter with an enumerated discrete GPU
also warns. Enumeration does not establish that the other GPU can present to
this window. Selection still uses the existing high-performance request.

`--frame-log [MS]` enables slow-frame logging, with 50 ms when omitted.
`TORE_FRAME_LOG=on` or a positive millisecond threshold enables it too; the
command line overrides the environment. `off` or `0` disables it. Thresholds
must be finite, greater than zero and no greater than 60000 ms. Escape > Debug
> **Slow-frame log** toggles it for the session, starting at 50 ms. This local
row works in joined games too. Like the other Debug settings it is not saved.
The existing Back to defaults row still resets only the AI test settings.

A completed presentation whose wall interval exceeds the threshold writes one
`Slow frame` record, at most ten in a one-second window. The next completed
frame after that window reports any suppressed slow frames. Every sixty
seconds of presented frames there is a `Frame log minute` summary of all frames,
including fast and suppressed frames: mean, maximum, percentile upper bounds
and the five phases with the highest accumulated time (mean and maximum).
The fixed histogram uses 0.25 ms bins through 1000 ms and powers of two above
that, capped at 65536000 ms. Idle screens are not forced to redraw for logging;
there is no timer wakeup just to print a summary.

Wall time runs from the previous presentation's completion to this one's.
The first frame starts at its accepted redraw, excluding startup. The
`idle_event_ms` remainder before redraw includes pacing, event handling,
focus throttling and OS scheduling. Network work there is charged to network
instead. `unaccounted_ms` exposes remaining work, rather than assigning it to
simulation or a wait without evidence. The CPU phase times are exclusive
main-thread wall milliseconds: nested phases are subtracted from parents,
and worker execution is included in the caller's wait, not summed CPU time.

Phases cover local simulation ticks, network work, terrain chunk preparation
and encoding, vegetation, cities/scenery, aircraft and stores, surface units,
effects/smoke, sky/stars, HUD, cockpit/panels/mirrors, overlays/text, remaining
render preparation, command encoding/finalization, queue submission, surface
acquire and present, GPU-result polling/mapping and canvas uploads. Timed render
work includes camera panels and mirrors.
AI is inside the shared simulation/network step and explicitly marked as such;
there is no separate AI figure or host-thread simulation figure. GPU time is
asynchronous earlier submitted work, labeled `gpu_previous_ms`, or unavailable.
It is not added to CPU phase times or presented as this frame's matching GPU
sample. Acquire/present are host API wall times, not a pure GPU execution time.

Counts use available CPU-side quantities across every camera drawn in the
frame: local ticks, network prediction tick advancement (which can include clock corrections), visible terrain chunks, placed vegetation (before GPU
culling), aircraft contacts, submitted surface vertices, routed surface units,
effect/smoke instances, mirror/panel views and draw calls in the world, city,
vegetation and effect passes. They are not unique objects across cameras.
Terrain is submitted as spatial chunks, not texture tiles. GPU-indirect city
and vegetation visibility is not read back for this log; placed instances and
draw calls must not be described as exact visible objects. Context includes
screen, view, theater, single-player/host/client role, focus and time scale.

Disabled hooks return at a relaxed atomic flag check, before clock reads,
thread-local access, counting, formatting or allocation. Enabled summaries use
bounded memory. Existing GPU/FPS measurement and ordinary startup logs are
independent of this switch. The normal session log retention and size cap
below still apply.

## Files and retention

Default log directories:

| Platform | Directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\T.O.R.E-Fighters\logs` |
| macOS | `~/Library/Logs/T.O.R.E-Fighters` |
| Linux | `$XDG_STATE_HOME/T.O.R.E-Fighters/logs`, default `~/.local/state/T.O.R.E-Fighters/logs` |

`TORE_LOG_DIR` overrides the directory. An isolated `TORE_DATA_DIR` without a
log override stores logs in that profile's `logs` child. Unwritable or
unavailable storage falls back to a T.O.R.E directory under the OS temporary
directory, then stderr. The actual destination or inability to save is reported
in the failure message. Logs never require writing into the install directory.

Each process owns a uniquely named, stable session file. A separate empty
`.lock` sidecar holds its lifetime lock, so the log remains readable while the
game is running on Windows as well as Unix systems. Retain five recent closed session logs; simultaneous processes must not
overwrite each other's files. A session
keeps at most 5 MiB of routine records, then records that further routine output
was suppressed. Fatal reports have a separate 256 KiB file and remain available
after the routine limit. Retain five closed fatal files independently. Routine
records are capped at 16 KiB; each fatal reason/backtrace is capped at 32 KiB.
Retention probes sidecar locks so killed sessions can be reclaimed without
removing another live process's output. Unlocked sidecars are removed after
both their session log and fatal report have been removed. Maintenance lock contention waits at
most 20 ms before falling back or skipping cleanup. Records are bounded and
repeated backend warnings
must not produce unlimited disk use. Startup milestones and fatal reports are
written promptly without relying on normal process shutdown.

The most recent fatal summary is also written to `last-error.txt` in the data
directory for compatibility. Successful starts, help and version queries do
not delete a previous error. Each report includes its timestamp and session
log path to distinguish old failures. Logger failures do not stop gameplay.

## Errors and panics

Returned fatal errors and Rust panics are recorded with build identity, active
stage and available causes. Panic reports include thread, source location and
a best-effort backtrace without requiring `RUST_BACKTRACE`. Panic logging must
not wait for its own held mutex or recursively panic. Worker panics are logged;
normal worker failure handling determines whether the application must stop.
The app does not attempt to resume gameplay after a main-thread panic. A window
surface dropped during a panic is left to process exit rather than torn
down, because tearing down a swapchain with an unpresented frame can panic
again and abort the process before the report and dialog.

Interactive Windows failures use a native error dialog independent of winit,
wgpu and imported assets. macOS uses an AppKit alert on the main thread. Linux
uses stderr and persistent files, plus optional `notify-send` with a
10-second display hint and a 2-second delivery-process timeout. A message names the failed stage, reason and actual report location.
Headless, capture, validation and diagnostic modes never block on a dialog.
`TORE_NO_ERROR_DIALOG=1` suppresses dialogs for unattended launches.

Windows fatal reports also attempt an Application event with source
`T.O.R.E-Fighters`, event ID 1000, and the build, reason and report path. Event summaries put
paths first and limit the reason to 4 KiB; full available backtraces stay local. The MSI
registers the source and the executable's embedded message resource. The game
never asks for elevation to log. Event logging failure leaves file reporting
and the dialog available.

## Native crashes on Windows

**Opinionated**, requested by John on 2026-11-02 after issue #8. The wait limits,
the stack scan and the helper thread design are agent decisions. Native crashes are
**not** reported on Linux or macOS, which keep their OS crash reports.

An access violation, stack overflow, illegal instruction or other exception that
nothing handled writes the same reports as a Rust panic: the fatal report file,
`last-error.txt`, the session log, stderr where there is one, the Application
event (source `T.O.R.E-Fighters`, ID 1000), and the native dialog under the same
rules as other failures, including `TORE_NO_ERROR_DIALOG=1` and automated modes.
Retention is the same. A process writes one native report, of at most 16 KiB, on
top of the fatal file's usual limit. The report states:

- the exception code and its Windows name, the faulting instruction address, and
  for access violations whether it was a read, write or execution and at what
  address;
- the faulting module's path and the offset inside it;
- the active stage, the last GPU step and whether it had returned, and the adapter;
- when the failure happened during GPU setup, one line saying the graphics API can be
  changed. It says the next launch offers the choice only when this process held a
  [setup marker](graphics-options.md#when-a-launch-did-not-finish-graphics-setup) at
  that moment, and otherwise points to Pref > Graphics and `--graphics-api`. This line
  is also in the dialog, and in fatal reports of returned errors and panics on Windows;
- the thread: `main` or another, its OS ID, and its name in the fatal report;
- code addresses found on the crashed stack as module and offset. These are
  candidates and not a verified call chain: a stack also holds stale values.

It uses an unhandled-exception filter, so it sees only what nothing else handled.
Deliberate exceptions that drivers and libraries catch with vectored or `__try`
handlers run first and never reach it, and Rust panics keep their own hook. The
filter chains to any earlier filter and then returns `EXCEPTION_CONTINUE_SEARCH`,
so Windows still writes its own "Application Error" entry and any dump its settings
ask for. After device creation the game checks that a driver DLL has not replaced
the filter; if one has, a WARN says so and the filter goes back in front, still
chaining to the other.

A crashed thread may hold the heap lock and may have almost no stack, so the
filter only copies the facts into preallocated atomics and wakes a helper thread
started at launch, with its own 512 KiB stack. That also answers the stack overflow
case: the filter runs in the little stack that remains, which is at least the
20 KiB Rust's runtime reserves on the main thread and its own threads. Rust's own
stack-overflow handler is a vectored handler that prints a line and continues the
search, so the filter runs after it. No extra stack is reserved, and no thread's
stack size changes.

The helper writes in order of safety: the facts that need nothing from the OS's
loader, then the module and stack scan, which can wait on the loader lock, then
`last-error.txt`. The filter waits at most 10 s for that. The event and dialog come
afterwards and may allocate inside the OS. It waits a further 5 s for them, or
120 s while a dialog is open. Whatever a blocked helper has not finished by then
is lost, and the report so far and the Windows crash record remain.

Not caught, because Windows ends the process without running any filter:
fast-fail terminations (`__fastfail`, stack cookie failures, and some heap
corruption), `TerminateProcess`, and failures before the game's own code runs. A
debugger takes the exception first. Those cases still leave the last stage and GPU
step in the session log.

## Diagnostic and release checks

`--diagnostics-self-test` runs a media-free success check and exits zero.
`--diagnostics-self-test=error`, `=panic` and `=worker-panic` deliberately
exercise reporting and exit nonzero without dialogs. `=graphics` presents a
synthetic first-run canvas and exits; it needs a display/GPU but no retail data.
`=dialog` deliberately shows the native error UI for manual acceptance.
`=native-crash` and `=stack-overflow` deliberately crash on Windows and turn off
the Windows crash window for that run so a script does not wait on it; the exit
status is the exception code, 0xC0000005 and 0xC00000FD. `=native-crash-window`
makes the same crash from inside a window's redraw, with Windows' message dispatch
on the stack, and needs a display. On other platforms all three fail with a message
saying they are Windows only. Any self-test may be combined with
`--graphics-api VALUE`. Diagnostic failures are plainly labelled as deliberate tests.

Release builds carry version, commit and target metadata and retain matching
debug symbols as separate build artifacts. Package checks launch staged
executables with an isolated profile, validate deliberate failure reports and
check runtime library dependencies. Installed Windows shortcut/direct launches,
macOS Finder launches and Linux desktop launches remain explicit acceptance
checks; build success does not substitute for them.

Missing libraries, OS security blocking, process kills, native crashes on Linux
and macOS, and other failures before or outside this reporting may leave only the
last stage, or no application log at all. OS crash/event reports and installation
evidence are needed in those cases. No automatic upload or crash-dump collection is
enabled by this feature.
