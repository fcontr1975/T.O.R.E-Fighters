# CPU while the window is in use, away or hidden (0.2.2, 2026-10-24)

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Players on Windows 11 reported the game's CPU climbing at times, and when they
alt-tabbed away from it. This pass measured what the game does with the
processor in each window state, fixed what was spending it for no reason, and
measured again. The rules it measured are in
[Frame rate](../spec/graphics-options.md#frame-rate); the mechanisms are in
[ARCHITECTURE.md](../ARCHITECTURE.md) (frame pacing, the shared workers, and
"Sleep and wait accuracy on each system").

## What was found

- **An unfocused multiplayer flight woke the extra simulation workers.** The
  flight frame told the worker governor it was late whenever more than 100 ms
  passed between frames. An unfocused window draws ten frames a second by
  design, so every one of its frames counted as late and the four shared
  workers stayed fanned out for as long as the window was in the background.
  A single-player flight pauses when it loses focus, so only multiplayer was
  affected. Now only the time the frame pacer did not choose to wait counts
  (`Pacer::busy`).
- **A Windows host spun a fifth of a core.** The host loop (a game hosting a
  mission, and `tore-server`) sleeps to just before each 120 Hz tick and spins
  the rest. The Windows margin was 2 ms, sized for the 15.6 ms default timer
  resolution; the standard library sleeps on a high-resolution timer on
  Windows 10 (1803) and later, so it overshoots by about half a millisecond.
  Now 0.75 ms.
- **A failing surface could retry in a tight loop.** A frame whose surface gave
  no image (lost, outdated, timed out) asked for its own redraw at once; a
  surface that kept failing would draw whole flight frames back to back. It is
  now retried after 16 ms, doubling to 256 ms. Not reproduced: no surface
  failed in any run here; this is a guard.
- **Events could add frames to an unfocused window's ten a second**, for
  example the pointer passing over a background window. They now wait for the
  frame already due.
- **Controllers were polled every 8 ms in an unfocused or hidden multiplayer
  flight**, although the bindings ignore them there. They are now polled at the
  menus' rate.

Looked at and left alone: winit's `WaitUntil` on Windows already waits on a
high-resolution waitable timer and never spins; device events are only
delivered while focused (winit's default); menus draw only on input or news;
the input device thread rests between polls (`park_timeout`, see below) and
never spins; the head tracker blocks on its socket with a 50 ms timeout.

## Linux measurements

The game in the agent's hidden workspace (`tools/agent-run.sh`), 60 frames a
second (`TORE_FRAME_LIMIT=60`, so the timer paces it), no audio. A local
measurement build drove the window states by sending the game the same
`Focused` and `Occluded` events a desktop would (the patch is not in the
repository); "hidden" is `Occluded`, as X11 and macOS report it, which paces
like a minimised Windows window. CPU is the game process's, in percent of one
core, sampled from `/proc` once a second, the first 3 seconds of each state
left out. The machine was busy with other builds (load average 30 to 40 on 24
threads), so single runs vary by several points; the focused flight varies
most because the fight in the mission develops.

Multiplayer: the dedicated server's guide mission on this machine, the game
joined and flying.

| State | Before (two runs) | After (two runs) | Busiest threads, before / after |
| --- | --- | --- | --- |
| Focused, flying | 65.8, 65.6 | 50.9, 44.9 | main thread |
| Unfocused | 17.4, 13.9 | 16.1, 13.9 | before: main 10.8 to 13.7 plus four workers 3.1 to 3.5; after: main thread alone |
| Hidden | 9.8, 7.5 | 8.3, 8.3 | main thread (the session's 120 Hz ticks) |

The first "after" run had every change but the hold on an unfocused window's
redraw requests; the second had all of them. The focused flight's difference
is the mission's fight and the machine's load, not this pass: nothing on the
focused path changed.

Single player (`--free-flight`): focused 33.6 before and 25.5 after (again
the fight); unfocused 0.9 and 0.6, hidden 1.0 and 0.6 (the flight pauses
and its still picture is not redrawn). Main menu: focused 0.4 and 0.7,
unfocused 0.9 and 0.7, hidden 0.9 and 0.8.

On Linux the workers' wakeups cost little: the work they did moved back to the
main thread and the unfocused total is within the runs' spread. The change
there is four threads that no longer wake 120 times a second in the
background. The hidden flight's cost is the session's ticks, which multiplayer
must keep running.

## Windows measurements

The CI machine: Windows 11 24H2 (build 26100) in a 12-thread virtual machine
with no GPU, so the game cannot open its window there. A probe ran the host's
real wait (`tore_net::plan_wake` and `wait_until`) at 120 Hz for 10 seconds a
run with a busy tick of 1 or 2.5 ms, counting the time spent spinning.

| Spin margin | Tick work | Spinning, a tick | Share of a core | Late, 99th percentile | Worst |
| --- | --- | --- | --- | --- | --- |
| 2 ms (before) | 1 ms | 1.65 ms | 19.8% | 0.22 ms | 1.19 ms |
| 0.75 ms (after) | 1 ms | 0.62 ms | 7.4% | 0.02 ms | 1.24 ms |
| 0.4 ms | 1 ms | 0.12 ms | 1.5% | 0.14 ms | 1.65 ms |
| 2 ms (before) | 2.5 ms | 1.72 ms | 20.6% | 0.24 ms | 1.21 ms |
| 0.75 ms (after) | 2.5 ms | 0.23 ms | 2.7% | 0.08 ms | 1.48 ms |
| 0.4 ms | 2.5 ms | 0.19 ms | 2.2% | 0.27 ms | 1.21 ms |

Sleep overshoot on the same machine: 1 ms 0.53 ms typical (worst 0.62), 2 ms
0.55 (0.80), 4 ms 0.09 (1.75), 7.5 ms 0.12 (0.50). `thread::park_timeout`
does not use the high-resolution timer: 4 ms lasts 15.9 ms and 16 ms lasts
31.7 ms, so the controller thread there polls about 63 times a second in
flight rather than 250 (not changed by this pass).

With the new margin the repository's own probe
(`cargo test -p tore-realtime-native --test wait_probe -- --ignored`) on the
same machine: 120 Hz ticks late 2.3 to 8.1 µs on average, 0.55 to 1.09 ms at
worst across its four variants, as EF-X measured with 2 ms (0.2 to 3.7 µs,
1.2 ms). `tore-app` builds and passes clippy there; started with no imported
data it waits on its first-run screen at 0.0 percent of a core over 20
seconds (4 threads).

What could not be measured on Windows: the game's own window (no GPU),
focus changes, minimising, alt-tab from full screen, and the workers' cost
there.
