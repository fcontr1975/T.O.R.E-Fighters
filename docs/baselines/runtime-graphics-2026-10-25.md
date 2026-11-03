# Runtime graphics rebuild validation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. This is validation of opinionated host graphics options,
not a retail comparison. Base: `df84c7de` on `msaa-rebuild`, from `0.2.2-rc1`.
Linux x86_64, NVIDIA GeForce RTX 4070, NVIDIA 615.78.08, Vulkan, wgpu 27.0.1.
No simulation, protocol, replay format, default adapter or flight mode changed.

## Reproduction and fix

Before changing production behaviour, the new offscreen regression failed on
Off to 2x with the player's exact error: the `Sun visibility` compute pipeline
expected binding 0 in `Spotting aid world depth`, but the assigned layout had
binding 1. The earlier Off frames completed. This reproduces the primary
Windows report on Linux with a different NVIDIA GPU; it does not reproduce the
player's 32-bit Windows environment.

`SimRenderer::set_graphics` now refreshes the lens flare's measurement pipeline
with the new depth layout and single-sample/multisample shader entry point.
Its probe layout is explicit and unchanged. Visibility storage, probe bindings,
per-camera timestamps and snap flags, palette uploads, backing images and the
single-sample composition pipeline remain valid and are retained. This keeps
the existing fade semantics rather than resetting the effect on each change.

The [test instructions](../testing/README.md#runtime-graphics-regression) own
the command and fixture coverage. Local logs are in John's task scratch folder:
`.local/next-round/0.2.2-rc1/msaa-rebuild/` in the primary checkout.

## Dependency audit

All production render/compute pipeline constructors in `tore-app` and both
surface acquisition sites were inspected. No additional missed rebuild was
found. The following table distinguishes source inspection from GPU coverage;
the offscreen regression exercises the shared world renderer, not every
possible effect or OS presentation transition.

| Resource or path | Runtime dependency and handling |
| --- | --- |
| `sim_renderer::Pipelines` | Rebuilt on sample changes: terrain, objects, airport solids/decals/lines, moving objects/decals, canopy depth/blend, sky, celestial, clouds, vapor, tracers, spotting light/dark passes, countermeasure sprites/glare/chaff, airfield lights and night glow. World materials and lighting use retained explicit layouts. Vapor's inferred-layout bind group is rebuilt. |
| World depth and colour targets | Sample changes clear all cached targets, including the depth bind group. Scale and window/output size are part of the cache key; main, mirror and panel targets are recreated on demand. Resampling remains single-sample with bindings made for each target. |
| Lens flare | Measurement was the missing dependency. It now follows the depth layout. Composition stays single-sample; its backing cache is keyed by output size. Thermal and sensor views omit the flare. |
| Smoke and effects | `set_samples` rebuilds smoke, airborne effect and ground-mark pipelines and clears their inferred-layout bind groups for the next prepare. CPU particle state and textures are not sample-dependent. |
| Vegetation and grass | `Renderer::rebuild` rebuilds vegetation colour, depth, legacy grass and both procedural-blade draw variants. Compute/cull and shadow layouts are sample-independent. Density changes replace capacity-dependent drawn buffers and main-stream bindings; panel streams are cleared. Cards, distance, detail, grass mode/density/distance are consumed by options, uniforms and mesh preparation. |
| City fill | Rebuilds full/detail/decal/fading/far pipelines on sample changes. Retained explicit material, lighting and instance layouts remain compatible. Options update cull distances and shadow reach. |
| Shadows | Shadow maps and casters are always single-sample depth. Quality changes recreate maps and their sampling bind group against the retained layout when map size changes; uniforms and active cascades/casters consume the new settings. Distance changes update lighting, vegetation and city fill. |
| Ground texture, terrain filtering and spotting strength | Per-view uniforms consume the current options. No option-dependent resource layout. Spotting shares the rebuilt depth layout and is active in main/mirror views, excluded from instrument sensor views. |
| Cockpit, mirrors, HUD, text and menu canvas | Composite pipelines are single-sample at the renderer's fixed output format. Mirror render-to-texture uses the same `SimRenderer` and its updated world targets. Cockpit geometry/projection and viewport follow window size. Canvas dimension changes recreate its texture/bind group. Text/FPS and leave-dialog overlays use fixed layouts. |
| Instrument panels, console, CRT and captures | Readback textures are keyed by dimensions and format, always single-sample. Their world views use `SimRenderer`. CRT composition builds its source/overlay bind group for each draw; console targets follow dimensions. |
| Replay viewer | Uses the same application renderer and Graphics screen settlement path as flight. Loaded scenery constructs its world renderer with the saved graphics choices. There is no separate replay pipeline cache to rebuild. |
| Surface format, frame limit and resize | Surface format is chosen when constructing the renderer and never changed by Graphics options, present-mode changes or resize. All format-dependent pipelines are constructed with it. Frame-limit changes reconfigure only when present mode changes, outside an acquired draw. Resize/lost/outdated recovery reconfigures dimensions, also outside an acquired frame. Pacing has no shader/layout dependency. |
| First-run presenter | Separate fixed-format, single-sample canvas blit, no world depth or MSAA. Resize changes surface dimensions and viewport. It has no runtime Graphics screen or dependent world pipelines. |
| Vegetation preview and timing | Preview uses the same world renderer with a fixed supported 4x count. GPU timestamps/query buffers are independent of sample count, output dimensions and format. |

## Secondary panic

The player log retains the original validation error and then a second fatal
entry from Vulkan swapchain destruction. In the installed wgpu 27.0.1 source,
`SurfaceTexture::drop` skips `texture_discard` when the thread is panicking.
In wgpu-hal 27.0.4, `Swapchain::release_resources` requires exclusive ownership
of acquire semaphores and panics if an acquired texture still holds one. This
explains the reported unwind sequence. The second panic happens inside a
destructor while unwinding, so Rust aborts the process ("panic in a destructor
during cleanup"): `main` never reaches its panic report, so the player gets no
error dialog naming the report (the fatal file and Windows event are written
earlier, by the panic hook).

Review follow-up: both presenters now hold their window surface in
`renderer::WindowSurface`, which leaves the surface to process exit when it is
dropped during a panic. A main-thread panic always ends the game, so nothing
needs the swapchain afterwards. This covers every acquired-frame failure, not
only this dispatch. A local check that panicked right after acquiring a frame
(Linux, Vulkan, RTX 4070, an uncommitted panic injected for the check, run
through `tools/agent-run.sh`) recorded three fatal entries and aborted with
status 134 before the change; after it, one fatal entry and exit status 101
through the normal panic report. Windows was not tested.

## Validation

- Before fix: the new GPU test failed with the exact reported validation error
  at Off to 2x (`gpu-before.log`).
- After fix: the GPU test passed all Off/2x/4x/8x/Off transitions for both
  formats, 150 submitted frames (`gpu-after.log`).
- `cargo test --locked -p tore-app -- --test-threads=8`: 2,337 passed,
  49 ignored, no failures (`crate-tests.log`). GPU tests remain opt-in.
- Workspace/all-target clippy with warnings denied: passed (`clippy.log`).
- Locked app build: passed (`build.log`).
- Formatting, documentation, source/binary asset scans and the 40
  battery-selection unit tests: passed.
- Windowed graphics battery: all 19 `menus-window-graphics*` scenarios passed,
  including the new runtime cycle and wrapped menu smoke. Two jobs, one window,
  isolated task profile. Summary and captures are under
  `battery/20261025-101153-msaa-rebuild/` in the task scratch folder.
- Separate first-run canvas graphics smoke: passed through `tools/agent-run.sh`
  with `--diagnostics-self-test=graphics` (`first-run-smoke.log`).

Windows 32-bit/DX12, macOS/Metal, retail comparison and deliberate
surface-unwind testing were not run. Workspace tests and the full battery were
not run, per this task's isolated-validation rules.
