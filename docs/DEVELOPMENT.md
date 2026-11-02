# Development environment

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Cockpit lighting captures may set `TORE_COCKPIT_LIGHTING=4,3,0,0`: instrument
level 1..5, flood 0..5, HUD colour 0..3 (green, amber, white, cyan), and HUD
brightness -256..256. This overrides local preferences for the run. Use
`TORE_WEATHER_TIME=12:00` or `00:00` with `--capture-flight`, through
`tools/agent-run.sh`. [Lighting spec](spec/cockpit-lighting.md).

## Baseline

Clone this repository and enter its root before running Cargo commands. Source code is under `crates/`, project documentation under `docs/`, and developer scripts under `tools/`. User-owned media belongs under ignored `gameassets/`; optional reference code under ignored `USNF-ATF/`; research output under ignored `.local/`. See [README](../README.md) for the short run workflow and [EXTRACTION](EXTRACTION.md) for the shared extraction script.

The initial host is an Apple Silicon MacBook Air M3 running macOS. Build natively as `aarch64-apple-darwin`; Rosetta is unnecessary. Development needs Rust, a native C linker, Git, and Python 3 for research/check tools. Building and unit tests need no retail media. Running the app needs imported Fighters Anthology menu and theater resources. There is no Node/Bun runtime, external synth, or Vulkan SDK requirement on macOS.

Rust **1.91.1** is intentionally pinned to match the compiler already present on the initial host. It is a reproducible starting version, not a claim to be the newest release. `rust-toolchain.toml` selects the minimal profile plus rustfmt and Clippy; `Cargo.lock` fixes resolved dependencies. Upgrade both deliberately and validate all three platforms.

## macOS setup

Install Apple's command line tools if Xcode or its tools are not already installed:

```sh
xcode-select --install
```

Install rustup using the [official installer](https://www.rust-lang.org/tools/install):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.91.1 --component rustfmt --component clippy
source "$HOME/.cargo/env"
```

If Homebrew Rust is already installed, the installer may report the existing installation. To keep it alongside rustup, prefix the `sh` installer command with `RUSTUP_INIT_SKIP_PATH_CHECK=yes`. Put `source "$HOME/.cargo/env"` after Homebrew's PATH setup in `~/.zshrc`. The initial host has an equivalent line configured; no Homebrew packages were removed.

Check tools from this repository:

```sh
command -v cargo
rustup show
rustc -vV
xcode-select -p
python3 --version
```

Cargo should resolve to `~/.cargo/bin/cargo` and the compiler host to `aarch64-apple-darwin`. Python 3.10+ is sufficient; if absent, `brew install python` supplies it. Python needs no third-party packages.

Build and run:

```sh
cargo build --workspace --locked
cargo run --locked -p tore-app
```

Expect native borderless fullscreen on the monitor the window would have opened on, showing Choose Activity, and a session-log entry such as `Renderer: Apple M3 (Metal, IntegratedGpu)`. The original 640 × 480 canvas scales proportionally and is letterboxed, so a 16:9 screen shows black bars either side. Close the window or use `? → Exit to Desktop`; Escape dismisses menus. On macOS, Command-Q also quits.

Alt-Enter switches between borderless fullscreen and the previous windowed size, on every screen: the menus, the Quick Mission creator, the locate screen and flight. F11 is not used for this, because it already opens the in-flight keyboard help. The choice is saved as `fullscreen` in `preferences-v1.conf`, which is now format version 5; a version 3 file still loads and starts fullscreen. `--windowed` starts in a 960 × 720 window for one run without changing the saved choice, and `--window-size`, `--smoke-test` and the captures keep their fixed-size windows as before.

On Windows a release build is a GUI application with no console window.
Debug builds keep terminal output. Every build writes startup logs; interactive
fatal errors use native Windows/macOS UI, and the Windows installer registers
Application events. The latest fatal summary survives successful starts. See
[startup logs and fatal errors](#startup-logs-and-fatal-errors) for locations,
limits and headless behavior.

Startup chooses randomly among all five original backgrounds; it does not run a timed slideshow. Force a variant for comparison with `--background CHOOSEV` (also accepts CHOOSEAC, CHOOSE3, CHOOSEU, CHOOSEM). The top bar moves to match each artwork's native origin. Hovering and keyboard focus are silent. An older menu-only cache requires re-import; the local default media is automatically used if available.

First launch imports automatically when it can find media on its own: the remembered source first, then `gameassets/fighters-anthology/`. Otherwise it opens the Locate Fighters Anthology screen, which lists detected sources, takes a typed path or a dropped folder, shows progress and reports failures in plain words; see [first-run import](spec/first-run-import.md). Pref reopens it as Re-import media. Use `--import <directory>` to refresh or choose other media from a terminal. `--import-only` imports and exits without opening a window/audio device, and a headless run with no usable source still fails with an actionable terminal error. Required media: reviewed `FA.EXE` alongside `FA_1.LIB` and `FA_2.LIB`; optional `FA_4B.LIB` and `FA_4D.LIB` supply flight and shell music. There is no native file dialog yet: drag-and-drop is being tried with players first.

Cache locations:

| Platform | Directory |
| --- | --- |
| macOS | `~/Library/Application Support/T.O.R.E-Fighters/` |
| Linux | `$XDG_DATA_HOME/T.O.R.E-Fighters/`, falling back to `~/.local/share/T.O.R.E-Fighters/` |
| Windows | `%APPDATA%\T.O.R.E-Fighters\` |

`TORE_DATA_DIR` overrides this directory for isolated checks, e.g. `TORE_DATA_DIR=.local/test-profile cargo run --locked -p tore-app -- --import gameassets/fighters-anthology --import-only`. Each import creates a versioned `menu-*.pack`; the latest valid pack is loaded, with fallback to earlier valid packs if a write was interrupted. `import-report.txt` records resource names and offsets. After a successful import or startup load, older numbered packs are automatically removed. An import is read back and validated before cleanup; failed imports leave earlier packs available. Cleanup leaves newer generations and unrelated files alone. See [cache retention](spec/import-cache.md). Imported resources never go into the executable.

The import keeps the Quick Mission ground target data (templates, surface units and their shapes) under their retail names ([contract](spec/import-cache.md#ground-target-data-slice-im1)). To prove a source imports completely, run `cargo test -p tore-import --test surface_data -- --ignored --nocapture` (about a minute each): it imports the install (`TORE_GAME_DIR`, default the `gameassets` link) and the 1.0 disc (`TORE_DISC_DIR`, default its `disc1` folder) into scratch folders under `TMPDIR` and checks that every template resolves.

## Linux and Windows

Linux: install rustup and a native toolchain. On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libxrandr-dev libasound2-dev python3
```

Running the app also requires a graphical session and a working Vulkan or OpenGL/EGL driver. CI builds and tests without creating a window.

An Omarchy x86_64 host has also passed local build, import and Wayland/Vulkan
startup checks with Rust 1.91.1; see the [Linux setup baseline](baselines/linux-setup.md).
The app requests a high-performance compatible GPU. On the tested AMD/NVIDIA
desktop this selects the RTX 4070; requesting the integrated AMD adapter produced
a blank visible window despite successful frame submission.
The renderer must be released in the event loop's exit callback, before its
display connection closes, and keep its window alive through GPU cleanup.

Windows: install rustup from the official installer and Visual Studio 2022 Build Tools with **Desktop development with C++** and a Windows SDK. Use the MSVC Rust host toolchain. Install Python 3 and Git. Run the same Cargo commands in PowerShell; use `python` instead of `python3` where appropriate. The renderer can use Direct3D 12 or Vulkan.

## Fresh clone

Run this once, before anything else:

```sh
python3 tools/setup_dev.py
```

It sets `core.hooksPath` to the committed `.githooks/` directory and reports any
missing tool. From then on `git push` runs the everyday checks below and aborts
the push if any of them fail, so a broken commit never reaches the remote.
Git hooks live in `.git/hooks/`, which is not version controlled, so every clone
needs this step. A single push can skip the hook with `git push --no-verify`.

The hook runs everything the CI job runs that a single machine can run. Other
platforms stay in CI, which builds and tests four targets when it is run:
Ubuntu 22.04, Windows 2022, macOS 14 on Apple Silicon, and macOS 15 on Intel.
Linux stays on the oldest supported Ubuntu LTS because the binary uses the
player's own glibc; a newer build host makes a program that will not start on
older distributions.
The `macos-15-intel` image is the last x86_64 macOS runner GitHub will offer and
retires in August 2027; after that, Intel coverage means cross-compiling
`x86_64-apple-darwin` from an Apple Silicon runner.

CI has no display, no GPU, and no retail media, so it proves the code builds and
the tests pass on each platform. It proves nothing about what appears on screen.
Run the window smoke test on a display-capable host for rendering changes.

## Everyday checks

While you work, iterate with the quick check, which runs only what your change can
affect in about five minutes: `python3 tools/quick_check.py`. See
[testing tiers](testing/README.md#testing-tiers). The list below is what must pass
before a merge or a push.

Run from the repository root:

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

On Windows, append `.exe` to both executable paths. `check_docs.py` verifies that
every Markdown file under `docs/` carries the current T.O.R.E header; `--fix`
writes missing ones. The pre-push hook and CI run all nine of these. Rust tests cover malformed formats, decompression, menu hit testing and interaction, PCM resampling, letterboxing, and extraction filesystem behavior using synthetic inputs. Python tests exercise the data guard. None requires a display, audio device, or retail files.

With a working desktop session, also run:

```sh
cargo run --locked -p tore-app -- --smoke-test
```

This uses the imported menu, a real 960 × 720 window (never fullscreen, so the presented frame keeps a known size) and GPU, prints the renderer, presents one frame without audio, and exits. It is not a headless simulation test. Normal mode waits while idle and schedules frames for short hover transitions and placeholder messages.

`--no-audio` silences a session. Normal playback uses the system's default output device, original PCM effects, recorded main/briefing playlists and the retail [situation scores](spec/flight-music.md) during flight when available. Device initialization failure is reported and the menu continues silently. Pref > Sound... in the main menu and in the paused flight menu opens the retail Sound/Music Prefs dialog: nine volume sliders and the channel swap switch, heard live while they move, kept by OK and saved to `sound-v1.conf` in the application data directory ([spec](spec/sound-prefs.md)). Music freezes on flight pause. No MIDI or synth is used. See [music behavior and limits](formats/music.md) and [combat audio](audio.md). Flight display preferences are restored from `preferences-v1.conf`.

A profile saved before the Sound dialog existed carries its old Music and
Effects switches over once: Music Off becomes both music sliders at off, and
Effects Off becomes Overall at off. Imported sample ownership or a silent
diagnostic run does not choose the setting. See the
[startup regression](baselines/menu-music-startup.md).

## Explore media and capture previews

```sh
python3 tools/explore_assets.py
mkdir -p .local/exploration
cargo run --locked -p tore-app -- --snapshot .local/exploration/menu.ppm
cargo run --locked -p tore-app -- --snapshot .local/exploration/pref.ppm --snapshot-state pref
```

The inventory records archive SHA-256 hashes, entry offsets, compression headers, and format counts without extracting everything. Snapshots render the native CPU menu canvas without a GPU/audio device; they are not window screenshots. States: `normal`, `hover`, `pressed`, `help`, `pref`, `multi`, and `controls`, `controls-keyboard`, `controls-mouse` and `controls-head` for the input configuration screen with a synthetic Xbox-layout gamepad, `controls-search` (the gamepad's actions searched for "fire") and `controls-search-keys` (keyboard actions on Shift keys), `controls-inv` (the invert box's tooltip showing), `graphics` for the Graphics options screen with the default settings (with no GPU to ask, it shows Off, 2x and 4x anti-aliasing as available and 8x as unavailable, and a 3840x2160 window for the render scale's sizes), `graphics-middle` and `graphics-bottom` with its list scrolled, `graphics-scale` with the render scale row focused, and `sound` for the Sound/Music Prefs dialog with sample levels and the swap switch at YES. `replays`, `replays-settings` and `replays-delete` show the Replays screen over a synthetic list of recordings (no recordings are read), with the auto-delete settings or the delete confirmation open. `bar-multi-hover`, `bar-replays-hover`, `bar-replays-pressed` and
`bar-replays-focus` inspect the main-menu top-bar alignment and highlight.
Snapshots default to CHOOSEV for repeatability; `--background` overrides it. On macOS, convert for viewing with `sips -s format png .local/exploration/menu.ppm --out .local/exploration/menu.png`. Keep all resulting retail derivatives ignored.

For the general extractor, use `python3 tools/extract_assets.py --dry-run` followed by `python3 tools/extract_assets.py`. It runs the standalone Rust tool in release mode without the app's window/audio dependencies. See [EXTRACTION.md](EXTRACTION.md) for filters, alternate source directories, and reports.

## Data guard

`tools/check_assets.py` scans tracked and non-ignored untracked files. It rejects Git-visible local media/reference directories, retail asset/cache extensions, plausible embedded EALIB archives, and recognizable standalone or embedded PIC headers. A format-name constant in the decoder is allowed; a plausible archive directory/sentinel is rejected. Pass explicit files/directories to inspect artifacts, including ignored outputs.

This is an initial guard, not proof that an artifact contains no retail derivatives. PIC has a structured header rather than ASCII magic. A `.tar.gz`, `.tgz` or `.tar` argument is opened and scanned member by member; MSI, DMG and AppImage are scanned as raw bytes only, which sees an uncompressed signature in the container but not a compressed payload. The guard does not identify converted art/audio. Run it against exact release contents before shipping; a debug executable check alone is not a release audit. The staged directory scan described under [Packaging](#packaging) is the real gate.

## Packaging

The release packages are built by `.github/workflows/release.yml` when a `v*`
tag is pushed, and by the same three scripts when a developer runs them by
hand. A push to any branch named `release-test/...` runs the build half only,
so the packaging path can be exercised without publishing anything; download
the five `packages-*` artifacts from that run with `gh run download`
(and `master-ubuntu-22.04`, the Linux `tore-master` for the Internet Lobby's
master server, [operations](MASTER-SERVER.md#downloading-a-release); and the
four `server-*` artifacts, the dedicated server's archives,
[downloading it](DEDICATED-SERVER.md#downloading-it)).
`workflow_dispatch` does the same once the workflow exists on `main`. A
dispatched run stamps `git describe` as the version, or the `version` input
when one is given, so a test package can carry the coming release's number
without a tag: `gh workflow run release.yml --ref BRANCH -f version=0.2.2-rc.1`.

CI (`ci.yml`, Rust baseline, and `network.yml`, Network loopback) no longer runs
on every push or pull request (John, 2026-10-23): the pre-push hook covers each
push locally, and the full matrix runs when it is wanted. Start it against any
branch from GitHub (Actions, the workflow, **Run workflow**, choose the branch)
or with `gh workflow run ci.yml --ref BRANCH` and `gh workflow run network.yml
--ref BRANCH`; watch it with `gh run watch`. Both also run on every `v*` tag. A
newer run for the same branch replaces one that is still queued or running.
Release packaging keeps its own triggers: `v*` tags, `release-test/...` branches
and `workflow_dispatch`.

The jobs run on GitHub's hosted runners, except on the platforms named in the
`TORE_SELF_HOSTED` repository variable, which run on the project's own
machines: [self-hosted runners](CI-RUNNERS.md).

Every script takes the version from `--version`, then from the tag the workflow
is running for, then from `git describe`, then falls back to `0.0.0-dev`. A
leading `v` is stripped. Packages are written to ignored `dist/`, and the
staged bundle each package is built from stays in `dist/stage/`.

The app carries its own version. The main menu shows the project badge with `vX.Y.Z` beside it in the
lower left corner and `tore-app --version` prints it. A release build reads
`TORE_BUILD_VERSION` at compile time; the workflow sets it to the tag (or to
`git describe` on a test branch) and refuses a tag that does not match the
version in `crates/tore-app/Cargo.toml`. A build without the variable reports
the crate version. Follow the [release checklist](RELEASING.md) before tagging.

Build the release binaries first, stamping the same version the package will
carry:

```sh
TORE_BUILD_VERSION=0.2.2 cargo build --release --locked -p tore-app -p tore-extract
```

CI and releases no longer build 32-bit Windows (dropped 2026-10-25). The
scripts still support it for a local build: run `rustup target add
i686-pc-windows-msvc` once, add `--target i686-pc-windows-msvc` to that build,
and give the packaging script the same target.

| Platform | Command | Produces |
| --- | --- | --- |
| Linux | `tools/package/package-linux.sh` | `dist/*.tar.gz` and `dist/*.AppImage` |
| macOS | `tools/package/package-macos.sh` | `dist/*.dmg` holding `T.O.R.E-Fighters.app` |
| Windows | `pwsh tools/package/package-windows.ps1` | `dist/*-windows-x86_64.msi` |
| Windows 32-bit, local builds only | `pwsh tools/package/package-windows.ps1 -Target i686-pc-windows-msvc` | `dist/*-windows-x86.msi` |
| Dedicated server, any of the three | `python3 tools/package/package_server.py [--max-glibc 2.35]` after `cargo build --release --locked -p tore-server` | `dist/server/tore-server-<version>-<platform>.tar.gz` (`.zip` on Windows) and its `.sha256` |

What each package contains:

- **tar.gz**: `tore-app`, `tore-extract`, `LICENSE`, `THIRD_PARTY_NOTICES.md`,
  `README.md`, the desktop entry and `tore-fighters.png`, which is the 256 px
  icon. Unpack anywhere and run.
- **AppImage**: `tore-app` only, with the desktop entry and the 256 px icon at
  the AppDir root under the name the entry's `Icon=` line asks for, plus a
  hicolor copy, so a desktop integrates it. `tore-extract` is a developer tool
  and stays in the tar.gz.
- **DMG**: `T.O.R.E-Fighters.app` plus a link to `/Applications`. The bundle
  carries `tore-app`, `tore-extract`, `tore.icns` and the three text files
  under `Contents/Resources`. `CFBundleIconFile` is `tore`.
  `CFBundleIdentifier` is `org.tore-fighters.app` and `LSMinimumSystemVersion`
  is 11.0, which is what the pinned toolchain targets on both architectures.
- **MSI**: a per-machine install into `%ProgramFiles%\T.O.R.E-Fighters`
  carrying both executables, `tore.ico` and the three text files, built with
  WiX v3 from `tools/package/tore.wxs`. Its `UpgradeCode` is permanent and
  `MajorUpgrade` replaces an older install rather than installing beside it.
  The 32-bit MSI installs into `%ProgramFiles(x86)%\T.O.R.E-Fighters`
  instead; the two share the `UpgradeCode`, so installing either replaces the
  other. The installer's pages and its shortcut choices are described below.
- **Dedicated server archive**: one folder named like the archive, holding
  `tore-server` (`tore-server.exe`), `server.conf` and `mission.txt` (the
  samples in `crates/tore-server/samples/`, which a `tore-server` test parses),
  `DEDICATED-SERVER.md`, `LICENSE` and `THIRD_PARTY_NOTICES.md`. The release
  workflow builds it on the Windows x86_64, both macOS and the Linux jobs, with
  a cargo command of its own so the game's binaries are built exactly as
  before, and uploads it as `server-<runner>` from `dist/server/`, apart from
  the game's packages. The script scans the staged folder and the archive,
  checks the runtime libraries, and runs `--version` (which must carry
  `TORE_BUILD_VERSION` when it is set) from the stage and from an unpacked
  copy. `tools/check_assets.py` scans a `.zip` member by member, as it does a
  tarball. The guide's [download section](DEDICATED-SERVER.md#downloading-it)
  is the operator's side.

None of these contain retail media. The app imports the player's own Fighters
Anthology copy at runtime; see [first-run import](spec/first-run-import.md).

### Application icon

The icon is the project logo, `docs/images/tore-fighters-logo.png`: a 1254 px
render of a round embroidered patch on a transparent ground. It is our own
artwork and carries no retail content.

`tools/package/build_icons.py` downscales it into the committed set under
`crates/tore-app/assets/icon/`. Regenerate it only when the logo changes:

```sh
python3 tools/package/build_icons.py
```

That needs ImageMagick 7 (`magick`) on `PATH`. Nothing else does: every build
and every packaging script reads the committed files, and CI never generates
icons. `--check` reports the committed sizes without writing, and
`--verify-ico` prints the entries of a finished `.ico`.

| File | Used by |
| --- | --- |
| `tore-16.png` … `tore-64.png` | macOS `.iconset`, and the DIB entries of `tore.ico` |
| `tore-128.png`, `tore-512.png` | macOS `.iconset` |
| `tore-256.png` | Linux tar.gz and AppImage, macOS `.iconset`, the PNG entry of `tore.ico` |
| `tore.ico` | the Windows executable, the MSI, both Windows shortcuts |
| `tore-64.rgba` | compiled main-menu badge, using the same 64 px project icon |

Downscaling uses a Lanczos filter, with a light unsharp pass at 64 px and below
so the small sizes stay legible. Two size decisions keep the committed set
under its 600 KB budget: 1024 px is not committed at all, because the source is
photographic and a lossless 1024 px PNG costs about 2 MB, and 512 px is
quantized to 255 colours, because it is only ever shown as a macOS Retina
512 pt icon. Every smaller size is full colour. The budget is enforced by the
script itself.

`tore.ico` holds 16, 32, 48 and 64 px as uncompressed 32-bit DIBs and 256 px as
a PNG, which is the layout Windows documents. The 256 px entry is the bytes of
`tore-256.png`, so the two cannot drift apart.

`tools/package/tore-fighters.desktop` is the Linux desktop entry, used by both
the tar.gz and the AppImage.

### The icon inside tore-app.exe

Explorer, the taskbar and Alt-Tab read an executable's icon from an embedded
`RT_GROUP_ICON` resource, so it has to be linked into the binary. The usual
answer is a crate such as `winres`, but that is outside the dependency budget
in [AGENTS.md](../AGENTS.md), so `crates/tore-app/build.rs` writes the resource
object itself.

The build script stamps source commit and target metadata on every platform.
On `windows-msvc` it also writes a Win32 `.res` file into `OUT_DIR`, holding
one `RT_ICON` per image, the `RT_GROUP_ICON` that names them and the Unicode
message table for diagnostic event 1000. It emits `cargo:rustc-link-arg-bins`;
`link.exe` accepts the file as if `rc.exe` had produced it. An unreadable icon
produces a warning and leaves the diagnostic message resource intact. Windows
GNU development builds retain the existing lack of embedded resources; release
MSIs use MSVC.

The byte layout was checked without a Windows host by running the same writer
over the committed `.ico`, parsing the result with
`python3 tools/package/build_icons.py --verify-res PATH`, and rebuilding an
`.ico` from the parsed resources: it came back byte for byte identical. That
the linker accepts the file is proven by the Windows job in
`release.yml`; that the icon then appears in Explorer is a manual check on an
installed build.

### The Windows installer

The MSI uses the WiX `WixUI_FeatureTree` dialog set: welcome, the licence, a
feature page with an install-folder Browse button, a confirmation page, then
the finish page.

The feature page offers three entries. **T.O.R.E-Fighters** is the program and
cannot be deselected. **Start menu shortcut** and **Desktop shortcut** both
start ticked and can be turned off. Each shortcut lives in its own component
keyed on a value under `HKCU\Software\T.O.R.E-Fighters`, which is what ICE38
and ICE43 require of a component holding a non-advertised shortcut. The
shortcuts are not advertised because an advertised shortcut cannot carry its
own icon.

The finish page offers a **Launch T.O.R.E-Fighters** checkbox, which runs the
installed executable through `WixShellExec` from `WixUtilExtension`, as the
installing user rather than as the elevated installer. `candle` and `light` are
both given `-ext WixUtilExtension` alongside `-ext WixUIExtension`; both ship
with WiX 3.14 on the `windows-2022` runner.

`ICE61` is the only suppressed validation check. It rejects
`AllowSameVersionUpgrades`, which we want so that reinstalling the same version
replaces the existing install instead of stacking a second copy.

One `tore.wxs` builds both MSIs. The script runs `candle` with `-arch x64` or
`-arch x86`; the 32-bit build picks Program Files (x86) and its own two fixed
shortcut component GUIDs, because a 32-bit and a 64-bit component must never
share a GUID.

### appimagetool

The script downloads `appimagetool` from its versioned 1.9.1 release (John,
2026-10-22; the moving `continuous` tag was rebuilt on 2026-10-04 and broke the
earlier pin) and checks it against a SHA-256 pinned in `package-linux.sh`, which
matches the digest GitHub lists for that asset. If the download ever stops
matching, the script refuses to run it; review the build and update the pin
deliberately. If the tool is
already on `PATH` that copy is used instead. Locally, no AppImage is a warning
and the tar.gz still succeeds; in CI it is a failure.

### Unsigned builds and first launch

No release uses Developer ID signing or notarization. macOS packaging applies
an ad-hoc signature to each executable and then the completed app bundle. This
seals its contents but does not identify a trusted developer. The standalone
macOS server is also ad-hoc signed. Windows and Linux packages remain unsigned.

- **macOS**: copy `T.O.R.E-Fighters.app` from the DMG to Applications and open
  it. If blocked, use **System Settings > Privacy & Security > Open Anyway**,
  then confirm **Open**. Recent macOS versions removed the right-click Open
  bypass. See [Apple's first-launch guidance](https://support.apple.com/en-us/102445).
- **Windows**: the executable links the C runtime statically
  (`.cargo/config.toml`), so no Visual C++ Redistributable is needed.
  SmartScreen warns about an unrecognized publisher. Choose
  **More info**, then **Run anyway**. This applies to the MSI itself.
- **Linux**: mark the AppImage executable (`chmod +x`) if the browser cleared
  the bit.

For a trusted project download that still will not open, remove its download
quarantine in Terminal, then open the app again:

```sh
xattr -dr com.apple.quarantine /Applications/T.O.R.E-Fighters.app
```

The **0.2.0 macOS download** needs this fallback when it says the app "is
damaged and can't be opened". That release has linker-signed executables but
no bundle seal covering its metadata and resources. The missing seal is fixed
in 0.2.1; it does not mean the 0.2.0 game data needs reinstalling.

Packaging signs the nested executables first, then the assembled bundle, and
does not modify it afterwards. It requires `Contents/_CodeSignature/CodeResources`
and runs `codesign --verify --deep --strict --verbose=2` on both the staged app
and the app mounted from the finished DMG; `codesign -dv --verbose=4` records
the signature details in CI. Server packaging verifies the staged binary and
the unpacked tarball copy the same way. Either verification failing stops the
release job. Agent decision: no hardened runtime (`--options runtime`), since
this repair needs an integrity seal without adding runtime restrictions.

### What CI validates, and what it does not

The release workflow runs formatting, Clippy, workspace tests, Python tool
and Windows resource-layout tests, documentation and asset checks. `ci.yml`
also scans the debug `tore-server` and checks its runtime libraries (the 2.35
glibc floor on Linux) on all four jobs, since it ships too. Neither runs a
32-bit Windows job any more (dropped 2026-10-25); a matrix entry that names a
`target` sets `CARGO_BUILD_TARGET` so every crate is linted, tested and built
for it. The release workflow builds release binaries with
matching debug information and retains PDB, dSYM or ELF symbols (the dedicated
server's too, on the jobs that build it) in separate
`symbols-*` workflow artifacts labelled by source commit.
These artifacts are not added to the retail-free game packages.

`tools/check_runtime_dependencies.py` rejects missing runtime libraries, dynamic
Windows Visual C++ runtime imports and non-system macOS libraries that are not
bundled. With `--max-glibc 2.35`, which CI and the release workflow pass on
Linux, it also rejects a binary that needs a newer glibc than Ubuntu 22.04
ships. It checks the build host, not every supported OS installation.
`tools/check_startup_diagnostics.py EXECUTABLE` exercises success, returned
errors and main/worker panics without media, using isolated paths with spaces.
On Windows it also crashes the executable on purpose and checks the native crash
report; elsewhere it checks that those modes say they are Windows only.
Packaging runs it on staged binaries and on the actual payload recovered from
the MSI, DMG, tar.gz and AppImage. MSI administrative extraction validates the
payload without changing an installed product. Source and package asset scans
remain required.

These checks do not prove installed desktop launches. Windows shortcut/direct
launches and Event Viewer messages, macOS Finder launches on both architectures,
and Linux desktop launches must still be checked on the resulting packages.
Graphics checks need a display and driver. The first-launch OS security prompts
are outside application logging. See [diagnostic evidence](baselines/startup-diagnostics.md).

## Troubleshooting

- Wrong Rust version: source `~/.cargo/env`, check `command -v cargo`, and run `rustup show` from this repository. Homebrew Cargo does not honor rustup overrides by itself.
- Linker/SDK error: check `xcode-select -p` and complete any pending Xcode first-launch setup.
- No graphics adapter or display: run from a logged-in desktop session with working drivers. Compilation and tests do not require a window; the smoke test does.
- Missing reference folder: the Rust app does not need it. Missing media: an existing valid cache still runs; otherwise import your own media. See [REFERENCES.md](REFERENCES.md).

### Release-check dependency

The optional [main-menu update check](spec/main-menu-presentation.md#updates-and-project-feedback)
uses `ureq` with defaults off and only its `rustls` feature, on a blocking worker.
This agent choice avoids an async runtime and system OpenSSL libraries. Its
bundled WebPKI roots and ring crypto backend preserve HTTPS certificate checking.
`serde_json` parses bounded API/cache input and `semver` compares release
precedence, including prereleases. `Cargo.lock` pins the resolved versions and
[third-party notices](../THIRD_PARTY_NOTICES.md#release-check-dependencies) records
their licences. No dependency is added to formats, simulation or synthesis.
Keep the runtime dependency check, including the release host's glibc 2.35
ceiling, when updating this stack. A binary linked on a newer Linux host still
needs validation in the release build environment.

### Menu presentation checks

The main menu uses the [badge, button and project-link layout](spec/main-menu-presentation.md).
Use `--snapshot-state update-available`, `update-focus` or `updates-off` with
`--snapshot PATH` to inspect the release links and preference without networking.
The ignored `updates::tests::live_api_manual_check` test makes one real request;
set `TORE_UPDATE_CHECK_EVIDENCE` to an ignored JSON output path before running it
explicitly with `--ignored --nocapture`. Normal tests use a local JSON fixture.
The first debrief page centers its result sentence under the heading.
`--quick-mission --snapshot-state debrief-success --snapshot PATH` previews the
success case; `debrief-1` retains the failed-mission preview. These CPU snapshots
use the imported fonts and artwork and must stay under an ignored local path.

### Startup logs and fatal errors

Every executable launch creates a diagnostic session before loading game data.
The log can be opened while the game is running, including on Windows; only a
separate empty sidecar is locked to protect active sessions from cleanup.
On an interactive failure, Windows shows a native dialog and macOS shows an
alert with the failed stage and saved-report location. Linux writes stderr and
files and attempts a desktop notification when `notify-send` is available.
Headless/capture checks do not show blocking dialogs. Windows MSI installations
also register the **T.O.R.E-Fighters** source under **Event Viewer > Windows
Logs > Application**, event ID **1000**. Event registration is installation
work; normal logging does not ask for administrator privileges.

Default directories and retention are in the
[diagnostics contract](spec/startup-diagnostics.md#files-and-retention).
`TORE_LOG_DIR` selects another destination. An isolated `TORE_DATA_DIR` puts logs
in that profile's `logs` child unless the log override is set. A failed default
location falls back to the temporary directory; the error message names the
actual report. `last-error.txt` in application data preserves the most recent
fatal summary, including its time and log path, even after a successful launch.

To verify reporting without game media, run:

```sh
tore-app --diagnostics-self-test
tore-app --diagnostics-self-test=error
tore-app --diagnostics-self-test=panic
tore-app --diagnostics-self-test=worker-panic
```

The three deliberate failures exit nonzero. `=graphics` presents a synthetic
canvas through the first-run renderer and exits, requiring a display/GPU.
`=dialog` deliberately displays the platform error message for manual checks.
On Windows, `=native-crash` and `=stack-overflow` really crash the process and
write the native crash report, and `=native-crash-window` does it from inside a
window's redraw. Their exit status is the exception code. Elsewhere they exit
nonzero with a message saying they are Windows only.
Set `TORE_NO_ERROR_DIALOG=1` for unattended normal launches. On Windows, use
`Start-Process -Wait -PassThru` when a script must wait for the release GUI exe
and inspect its exit code; its stdout is not a visible console.

If no session file appears in either the normal or fallback location, the
process may have been blocked before its own code ran. Check installation,
OS security messages and available system crash reports. On Windows a native
crash writes the same reports as a panic, naming the exception, the faulting
module and offset, the stage and the last GPU step
([contract](spec/startup-diagnostics.md#native-crashes-on-windows)); Linux and
macOS do not report native crashes, and nothing catches a process kill. Windows
Error Reporting, macOS Console crash reports and Linux journal/core reports provide
separate OS evidence where enabled. Do not confuse these with a proven application
cause.

The log's last `GPU setup:` line names the shader or pipeline being created when
the driver stopped. Repeating `Watchdog: still in stage` lines mean the process hung
rather than crashed, since a crashed process stops writing. On Windows the player
[chooses Vulkan or DirectX 12](spec/graphics-options.md#graphics-api) on the import
screen or in Pref > Graphics. `--graphics-api vulkan` or `dx12` chooses for one run,
and `--graphics-api dx12 --diagnostics-self-test=graphics` checks DirectX 12 without
game media. Share the timestamped fatal report and session log when reporting a
startup problem; logs can contain local paths and device names.

An editor with rust-analyzer is useful but optional. No global editor configuration is required.

### Diagnosing slow frames

In flight, open **Escape > Debug > Slow-frame log** and turn it on. Play
through the slowdown, then send the current session log from the locations in
[startup logs](#startup-logs-and-fatal-errors). It starts at a 50 ms threshold,
is local to this session, and can be turned off with the same row. It does not
change the graphics settings or keep a paused menu drawing.

From a terminal, use `tore-app --frame-log` or `tore-app --frame-log 20`.
`TORE_FRAME_LOG=on` or `TORE_FRAME_LOG=20` works too. `--frame-log off` overrides
the environment. For an isolated developer run on a hidden workspace:

```sh
TORE_DATA_DIR=.local/dev-profile tools/agent-run.sh target/debug/tore-app --free-flight --no-audio --frame-log 1
```

The always-on `Session system` line describes the adapter, driver, presentation,
CPU cores, worker pool and graphics settings. A software renderer warns that
graphics run on the CPU. On Windows, update the graphics driver and select
**High performance** for the game under **Settings > System > Display >
Graphics** if the log reports software rendering or an integrated GPU while a
discrete GPU is available.

`Slow frame` lines split CPU wall time by existing work phases and waits, with
counts and flight context. `idle_event_ms` includes frame pacing and focus
throttling; `unaccounted_ms` shows the remaining gap. GPU time is from earlier
work and AI is included in simulation/network. Counts distinguish placed
instances from GPU visibility. Output is limited to ten slow lines per second,
plus a suppressed count, with percentiles and worst phases every minute.
See the [measurement contract](spec/startup-diagnostics.md#graphics-session-and-slow-frames)
for exact boundaries and limitations. The existing 5 MiB session log cap still
applies, so capture the slowdown soon after enabling it.

## Terrain development loop

The main-menu Create Quick Mission action opens the original `QUIKMIS3.PIC` artwork with editable briefing fields and standard/custom weapons selection. The terrain inspection camera is now a CLI diagnostic. This supports airborne missions with straight-flying practice aircraft; combat AI and mission objectives remain open. Launch it with `--quick-mission`, or skip to the world with `--viewer`.

```sh
cargo run --locked -p tore-app -- --quick-mission --smoke-test
cargo run --locked -p tore-app -- --viewer --smoke-test
cargo run --locked -p tore-app -- --viewer --no-audio
mkdir -p .local/theater-research
cargo run --locked -p tore-app -- --quick-mission --snapshot .local/theater-research/quick.ppm
cargo run --locked -p tore-app -- --capture-terrain .local/theater-research/terrain.ppm
```

The quick-mission snapshot is a headless CPU image. `--capture-terrain` requires a real display/GPU, renders the simulation pass into a 960 × 720 offscreen target, reads it back as PPM and exits without audio. It excludes the HUD; it is not a desktop screenshot. Convert locally with `sips` on macOS if desired. Both captures start at a repeatable camera pose. Keep derivatives ignored.

Controls: arrows move horizontally, Shift accelerates translation 8×, Q/E or PageDown/PageUp lower/raise, A/D turn, W/S pitch. Camera movement uses elapsed time with a 50 ms cap, clamps to the theater and stays at least 100 feet above the rendered surface. It is an inspection camera, not aircraft physics. Focus loss clears held keys. Escape returns to the creator, then Choose Activity. The viewer schedules frames while active; menus remain idle when no redraw is needed.

`terrain.rs` (in `tore-world`) owns the renderer-independent surface, airport and weather data (`Terrain`), `camera.rs` the free camera, `scenery.rs` the art, mesh and camera weather built from it (`Scenery`); `sim_renderer.rs` and `terrain.wgsl` own the depth-tested GPU scene and sky. Follow [the recovery notes](formats/theater.md) before extending source semantics; record approximations and native evidence. The initial renderer builds the whole selected theater mesh at startup, including when entering through the main menu. Streaming, LOD, object rendering and complete weather remain open.

### All-theater selection and text

All 16 creator entries now select/load a theater, rebuild its GPU resources, update its briefing map and reset the free camera. `--theater CODE` also works with `--viewer`, `--quick-mission` and `--capture-terrain`. Ukraine retains the original inspection pose; others start near the grid center at 28,000 feet with a terrain-clearance floor. Example: `cargo run --locked -p tore-app -- --viewer --theater EGY`.

Older Ukraine-only caches automatically re-import from default local media. For external media, refresh with `--import`. The creator/all-theater pack is capped at 1 GiB / 32,768 resources (2 MiB each); a full import holds about 4,100 resources and 180 MB. It is still a development cache. Source textures use the first three theater-code characters (TVI for TVIET); Kurile's base MM has no numbered texture placements and currently renders palette-colored height geometry.

Quick Mission and Load Ordnance use a bundled, open-licensed Noto Sans Bold raster atlas for clean flat text, with beveled field boxes. See the [menu presentation spec](spec/quick-mission-menu.md) and [font provenance](../crates/tore-app/assets/README.md). Top menu bars retain MENUFONT and center labels using visible glyph bounds. Other menus retain their original imported fonts. No system font or new runtime dependency is required. Use `--quick-mission --snapshot .local/quick.ppm` to inspect the page.

## Hornet free flight

Create Quick Mission supports all imported aircraft with accepted weapons, ammunition
and fuel, and up to 29 straight-flying dummies from the six wing selectors. Select
no ground targets/defenses. Select a custom load to open Load Ordnance; standard
uses reviewed PT defaults. Direct `--free-flight` loads supported default weapons
with full fuel; restricted native research flight remains clean.
Selected creator altitude is retained exactly or rejected for terrain clearance.

```sh
cargo run --locked -p tore-app -- --free-flight --theater UKR
cargo run --locked -p tore-app -- --free-flight --theater TVIET --flight-view 1
cargo run --locked -p tore-app -- --headless-flight 1200 --maneuver pull
```

Arrows: pitch/bank (Down pulls up). End/PageDown or Z/X: rudder. 1..5: throttle idle/25/50/75/100%; 6: afterburner; 7/8: throttle -/+5%. Shift-B: afterburner switch (requires >95% throttle). E: engine. G/F/B/H: gear/flaps/airbrake/hook. O: F-22 bays. Insert/Delete: chaff/flare. J: jammer. R: radar power, or return to the radar channel when infrared is selected. M: cycle the available sensor channels; I: infrared; Y: scope contact history; comma/period: scope setting; N: NAV/ILS mode; W/Shift-W: waypoints. F1/F2/F3: front/back/up; F10: external. Shift-0..9: instrument windows (four large or six small). Backspace toggles cockpit art. Escape opens the paused in-flight menu; Ctrl-P pauses/resumes; Ctrl-Q ends flight. Focus loss pauses automatically. On Macs use Fn/Globe for function keys, Fn-Right/Down for End/PageDown and Fn-Delete for Delete. See [all current controls and source status](FLIGHT-CONTROLS.md).

Instrument window numbers: 0 radar cross section, 1 envelope, 2 front view, 3 other view, 4 target, 5 RWR, 6 navigation, 7 systems, 8 weapons, 9 radar. Page 0 draws the exposure contour and received emitters with `-`/`+` scale buttons; page 9 has `-`, `+`, `M` for the sensor channel and `Y` for history, and a click on a contact designates it. Unsupported readings and no-target states remain explicit. Older Hornet-less caches refresh from local media. `--viewer` remains a developer terrain diagnostic but has no creator button. Source data, authored integration and remaining parity work are distinguished in [aircraft notes](formats/aircraft.md); [baseline commands](baselines/f18-free-flight.md) cover screenshots and tests.


The full-height cockpit and live HUD can be captured with `--capture-flight .local/cockpit.ppm`. Add `--flight-menu` to capture the paused Escape menu. `--flight-view 0|1|2|3|4` selects front/chase/oblique/back/up for inspection. These flags require a display for GPU capture. See [cockpit/control validation](baselines/cockpit-controls.md).

Scene captures hold the prepared flight and weather state while the window and
GPU initialize. Startup time does not advance the captured simulation. Use
`--flight-probe-ticks` to capture an explicitly stepped state; a plain capture
uses the launch state. This also applies to `--capture-terrain`.


Flight UI now adapts to drawable aspect ratio independently of menu letterboxing. Use `--window-size 1280x720` (or resize normally) to inspect widescreen behavior. `--capture-flight` preserves the current aspect and writes at the flight overlay resolution, capped proportionally at 1920×1080. The original `--capture-terrain` remains 960×720. Small instruments resample directly from their native rasters; HUD readouts have transparent backgrounds. See [responsive-flight checks](baselines/responsive-flight-ui.md).

A mission replay frame is captured the same way with `--watch-replay FILE --capture-replay OUT.ppm --replay-tick N`, optionally with `--flight-view`, `--replay-aircraft`, `--replay-drone`, `--replay-ui`, `--replay-panels`, `--replay-look-at` (the object view), `--replay-clean` and `--replay-menu '?'|pref|time|help|graphics|sound|controls` (the Esc pause menu at that page, or that screen over it); `--replay-speed` with `TORE_PERF_FRAMES` times playback. See [replay captures](REPLAYS.md#captures-and-timing). `--debug-panels` and `--flight-panels thought,telemetry,guidance,comms,menu` show the [debug panels](REPLAYS.md#captures) in a `--capture-flight` capture.

## Headless development

The stores preview also has a CPU-only geometry mode. On a private imported
profile, `TORE_STORES_PREVIEW_CPU=1 TORE_STORES_PREVIEW_AIRCRAFT=f15,f16c
 target/debug/tore-app --stores-preview OUT_DIR` writes below, side-front,
front and top PNGs without a window or GPU (put the command on one line).
It uses flight's airframe and pylon triangles with solid face colours and
simple lighting, so textures and final GPU lighting still need offscreen
review. Omit the aircraft filter to cover the selectable roster.
`TORE_STORES_PREVIEW_FULL=1` fills each row with its current candidate up to
the normal Load Ordnance capacity, preserving installed tanks on shared rows.
`TORE_STORES_PREVIEW_CATALOG=1` prints compatible weapon and tank choices
and capacities by source row for reproducing alternative loads.
`TORE_STORES_PREVIEW_BAY=0..1` poses the doors. With GPU mode,
`TORE_STORES_PREVIEW_FIRE=5` instead presses the trigger once for hardpoint
row 5 and captures actual simulation at ticks 0, 60, release, release +3,
release +12 and 400. It checks bay gating and closing; use row 7 for side
bays or row 8 with `TORE_STORES_PREVIEW_FULL=1` for the wing missiles. Each aircraft's atlas is
reset before a fleet capture. `TORE_STORES_PREVIEW_GEOMETRY=1` prints the
source face vertices for local fitting, and `TORE_STORES_PREVIEW_MOUNT=7=3.9,-1.05,1.7`
sets one preview-only mount in feet right/up/forward. These fixture overrides
never change a flight or saved loadout.


Set `TORE_STORES_PREVIEW_AUDIT=1` with CPU mode to print station and model
chord measurements instead of images. Optional rows use their configured
candidate store and are labelled `loaded=0`. Set
`TORE_STORES_PREVIEW_RECORDS=4=AIM7.JT:4` to select a source row's store and
count through the normal ordnance compatibility checks.
`TORE_STORES_PREVIEW_CHEAT=1` opts into the existing ordnance-screen Cheat
for explicitly labelled geometry checks of unsupported combinations. A preview-only
comparison, `TORE_STORES_PREVIEW_RECORDED_STATIONS=f15,f16c`, bypasses the
per-aircraft station table for those aircraft and uses the generic source
placement. It does not change flight, the import or saved loadouts.
For a short GPU review, `TORE_STORES_PREVIEW_LOADED_ONLY=1` skips empty
loads, close-ups and release frames, while `TORE_STORES_PREVIEW_VIEWS=below,side-front,ahead`
limits the camera list.
See [the stores specification](spec/stores-on-pylons.md#the-f-16c-and-fleet-rail-audit)
for the audit's limits.

For many runs at once with automatic checks (menus, flight, AI fights up to 15 against 15, replays) use the battery in [testing](testing/README.md).

Run from the repository root with the pinned Rust toolchain and `--locked`.
`cargo run` uses the dev profile by default, retaining debug symbols and
assertions; no `--dev` flag is needed. Keep runtime data separate from normal
play by setting `TORE_DATA_DIR` to an ignored directory under `.local/`.

On Linux/macOS shells, prepare an isolated profile and output directory:

```sh
mkdir -p .local/headless
export TORE_DATA_DIR="$PWD/.local/dev-profile"
cargo run --locked -p tore-app -- --import gameassets/fighters-anthology --import-only --no-audio
```

The import command reads local user-owned media, creates the cache and exits
without opening a window or audio device. Run it once for a new profile; later
commands reuse that cache. See [cache locations and refresh](#macos-setup).

For a deterministic flight-model probe, run 1,200 fixed simulation ticks
(10 simulated seconds):

```sh
cargo run --locked -p tore-app -- --headless-flight 1200 --aircraft f18 --maneuver level --no-audio
```

This prints flight state and exits before window/audio initialization. It is
the isolated flight-model probe, not a complete rendered mission. Keep the
default researched flight adapter unless the task explicitly concerns another
adapter. Use `--aircraft rafale` for the exact Rafale C identity.

The powered-lift aircraft (`av8`, `yak141`, `v22`, `ah64`, `mi24`, `ch47`) start in
trimmed forward flight. `--maneuver hover` starts one at rest in the air in its own
hover trim (hands off it holds), and `--replay-input TAPE` flies a `tore-pilot 3`
tape from there. `--maneuver settle:SINK_FPS` starts a powered-lift aircraft 60 ft over flat open ground in its hover trim and holds that sink rate on the collective (the throttle on the jets) until it is down, then prints a `touchdown:` line, the probe of the [touchdown rule](spec/touchdown.md); the battery's `flight-powered-*` scenarios do exactly this
([flight lane](testing/lane-flight.md#powered-lift-scenarios-vtol-overhaul)).

For an individual aircraft's actual surface geometry, before broad integration
checks:

```sh
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --aircraft a7 --animation-probe .local/animation-probes/a7 --no-audio
```

This exits before window initialization. It writes actual transformed OBJ poses,
`report.json`, `index.txt` and CPU orthographic contact sheets. Missing required
motion, new planar crossings, reversed hook stow or a reviewed attachment/
direction failure returns an error after writing
evidence. Raw candidate seams still need source review. Keep these user-owned
geometry derivatives ignored. `--headless-flight` alone does not test rendered
control surfaces. Reports distinguish `motion-survey` from
`reviewed-controls-and-attachments`; a passing generic survey is not acceptance.
See the [aircraft animation audit](baselines/aircraft-animations.md).

For a CPU-rendered Ordnance screen capture:

```sh
cargo run --locked -p tore-app -- --quick-mission --snapshot-state ordnance --snapshot .local/headless/ordnance.ppm --no-audio
```

The snapshot command also exits without a display or audio device. Other menu
states, including `ordnance-empty` and `ordnance-drag`, use the same command.
`ordnance-tanks` previews air-to-surface with compatible tanks first, using the
same source station rows as air-to-air. The name is retained as a snapshot alias.
For a source-backed F-14 tank selection, mass and accepted-restart check without
a display, run `cargo run --locked -p tore-app -- --validate-tanks --aircraft f14
--no-audio` with an imported development profile.
For all 36 retail aircraft, `--validate-ordnance --no-audio` checks supported
weapon and tank compatibility, source station coverage, old loadouts and
accepted-load restoration. The battery scenario is `menus-ordnance-availability`.
Keep captures, logs and imported media under ignored directories. When using
`target/debug/tore-app` directly, first rebuild with
`cargo build --locked -p tore-app` so the binary matches the source.

`--no-audio` only disables sound. A plain app launch, `--free-flight`,
`--flight-probe-ticks` by itself, GPU `--capture-flight`/`--capture-terrain`, and
`--smoke-test` still need a display. Do not use those as generic headless
commands. The environment and other feature probes have their own exit paths;
follow their documented commands elsewhere in this guide.

### Windowed runs from scripts and agents

`tools/agent-run.sh COMMAND [ARGS...]` runs any command with its window on a
spare Hyprland workspace instead of the active one. The launch is silent (no
focus change, no workspace switch) and floating, so the window keeps its
requested size. Each concurrent run takes its own workspace from 91 to 99
(`TORE_AGENT_WS_FIRST` and `TORE_AGENT_WS_LAST` change the range) and frees it
on exit. Output, exit status and working directory behave as if the command ran
directly, and stopping the wrapper with INT or TERM stops the command and its
children. (SIGKILL cannot be caught, so a `kill -9` of the wrapper leaves the
game running; close it with `pkill -x tore-app`.) Outside Hyprland it just runs
the command in place. It needs no window
rule and does not retitle the window.

```sh
TORE_DATA_DIR=.local/dev-profile tools/agent-run.sh cargo run --locked -p tore-app -- --smoke-test --no-audio
```

Verified on Hyprland 0.56 (Lua `hyprctl dispatch`); older releases use the
fallback `[rules] command` form. Prefer the windowless modes above where they
answer the question.

To press keys and click in a windowed run without a person, give it `--input-script FILE`.
The script feeds key presses and mouse events to the game through the same handlers the
window's own events use, so letters, digits, modifiers and the mouse reach the real input path
whether or not the window has focus (a window on a spare workspace never does, and the game
rightly ignores most keys then). One step per line, `#` for comments:

```text
wait 1.5           seconds of wall clock
stall 8            block the game's whole main loop this long (at most 600 s), as a held window does
waittick 600 [90]  until the flight has run this many 120 Hz ticks, or 90 s (default)
key g              press and release: Shift+e, Ctrl+B, F10, Space, Escape, Enter, Up, ]
down Up            hold a key          up Up     release it
move 320 240       mouse to window pixels   movemenu 320 240   to the 640 by 480 menu layer
click [left|right] press and release   press / release   hold a button
wheel 3            wheel notches, negative for down
snapshot out.ppm   the menu layer as drawn now (menu screens); relative paths go in $TORE_SCRIPT_OUT
shot out.ppm       the flight as drawn now, cockpit, HUD and instruments included (same path rule)
exit               quit
```

`stall` is for the multiplayer keepalive: a joined game's loop stops for the time, its
keepalive thread keeps it connected (the host logs "game stalled" and "game back after"), and
its plane flies neutral. Stopping the process with SIGSTOP cannot test that, since it freezes
the keepalive thread too.

In a networked flight (`--connect`) `waittick` counts the client's ticks. To try the game
against a server on this machine, import a profile for both, start `tore-server` on a spare
port with its data folder (`tore-server --port 26911`; a `server.conf` with `empty-timeout
3600` keeps the mission up between runs), start the game through `tools/agent-run.sh` with
`--connect 127.0.0.1:26911 --slot 1 --windowed --no-audio --input-script FILE`, and a
`tore-bot --connect 127.0.0.1:26911 --slot 0 --seconds 200` for company. Build the server,
the bot and the game at one commit: a build must match to join.

`tore-bot --capture FILE` keeps the bot's capture (`FILE-1`, `FILE-2` with `--count`), and
`tore-app --convert-capture FILE` turns it into a replay
([network flights](REPLAYS.md#network-flights)); the battery scenario `net-convert-capture`
does both against a real server.

`Down` is nose up and `Up` nose down, as in the game. Set `TORE_RECORD_MISSIONS=1` to record the
flight. The battery's hand-flown scenarios (`replay-script-*`, scripts in
`tools/battery_scenarios/scripts/`) use it. The replay viewer takes its own events, but a script's
keys reach it too since the observer screen (slice F2-O2: the same names, `Escape`, `End`, `Tab`,
`Enter` and the letters, with no Alt or Command), and `shot` writes the viewer's picture there.
`tools/battery_scenarios/_replay_drive.py`, which sends keys to a window through Hyprland by
process id, remains for the replay lane and for quitting with Alt+F4; see the
[replay lane](testing/lane-replay.md#how-the-checks-work).

In PowerShell, create `.local/headless` with `New-Item -ItemType Directory -Force .local/headless`,
set `$env:TORE_DATA_DIR` to the absolute `.local/dev-profile` path, and run the
same Cargo commands. Remove the environment override after the session with
`Remove-Item Env:TORE_DATA_DIR`; on Linux/macOS use `unset TORE_DATA_DIR`.

## Graphics options

Pref → Graphics... sets anti-aliasing, render scale, the spotting aid, the frame rate, terrain filtering and the frame-rate counter; see [graphics options](spec/graphics-options.md). `TORE_SHOW_FPS=1` shows the counter for one run without saving it. `TORE_TRACE_PRESENT=1` logs (at info level, to the diagnostics log) each change of the presented screen kind, menu, loading, flight or replay, with the frames drawn of the one before, to catch a screen flashing between two others; the battery's `replay-script-ordnance-no-flash` uses it. `TORE_FRAME_LIMIT=off|vsync|60|100|200` sets the [frame rate](spec/graphics-options.md#frame-rate) for one run; `off` is uncapped rendering for benchmarks. Scripted runs, `TORE_PERF_FRAMES` measurements, captures and smoke tests are uncapped unless it is set. For one run, without saving, use `--anti-aliasing off|2x|4x|8x`, `--render-scale 75|100|125|150|200`, `--spotting-aid off|subtle|strong` and `--terrain-filtering on|off`; `--original-graphics` turns every addition off at 100%. Flags apply in order. Captures and smoke tests ignore the saved `graphics-v1.conf` and use the defaults plus any flags, so compare `--original-graphics` against no flag for matched before/after images.

## Flight performance

For the fixed-work before/after threading battery, see
[performance validation](testing/performance.md).

Normal `cargo run --locked -p tore-app -- --free-flight` now optimizes the app crate at level 2, retaining debug symbols/assertions. Dependencies keep their existing debug settings; Cargo may still label the overall dev profile “unoptimized.” No release build is required to benefit. Simulation remains fixed at 120 Hz; presentation interpolates its last two poses at the Graphics screen's [frame rate](spec/graphics-options.md#frame-rate) (V-sync by default, with one requested queued frame). Performance measurements (`TORE_PERF_FRAMES`) draw uncapped with Immediate presentation, as before, unless `TORE_FRAME_LIMIT` names a rate; set `TORE_FRAME_LIMIT=vsync` or `60` to measure a capped run.

Run a bounded desktop measurement (macOS/Linux shell):

```sh
TORE_PERF_FRAMES=330 TORE_PERF_ACTIVE=1 TORE_PERF_VIEWS=1 cargo run --locked -p tore-app -- --free-flight --no-audio --window-size 1280x720
TORE_PERF_FRAMES=180 TORE_PERF_ACTIVE=1 cargo run --locked -p tore-app -- --free-flight --no-audio --instrument-page 3
```

On PowerShell, set `$env:TORE_PERF_FRAMES="330"`, `$env:TORE_PERF_ACTIVE="1"`, and `$env:TORE_PERF_VIEWS="1"`, run the same Cargo command, then remove those environment variables. `TORE_PERF_FRAMES` accepts 60–100000 frames (0/off by default), prints mean/p50/p95/max milliseconds and exits after that many flight, terrain-viewer or replay-viewer frames. The first 30 are excluded. Presence of `TORE_PERF_VIEWS` cycles front/back/up/chase/oblique every 30 frames; omit it to measure the selected view or switch views manually. Use `--free-flight` for aircraft measurements or `--viewer` with a fixed `TORE_WEATHER_VIEW` for terrain measurements. Avoid concurrent GPU workloads when comparing runs. Presence of `TORE_PERF_ACTIVE` explicitly keeps the bounded flight benchmark unpaused even if automation steals focus (it does not dismiss menus). Omit it for normal pause behavior and interactive pause measurements. The report includes paused frames and completed live camera readbacks; check these before interpreting a run as active flight. A flight run that ran any simulation ticks adds one line after the paused-frames line: `simulated: N ticks in S s, R x real time (asked for Kx); ticks per frame mean M, max X`. It counts the 120 Hz ticks the recorded frames ran against the wall time those frames covered, since the time compression last changed, so a run that steps up to 8x reports the 8x alone. It shows the rate time compression really reached (`R`) beside the one chosen in the game (`K`), and how many ticks a frame ran. When `R` falls short of `K`, the frame loop has hit its 0.25 s limit on a frame's time (see [the frame loop under load](ARCHITECTURE.md#the-frame-loop-under-load)). To measure it, press `C` in the script (`--input-script`, `key c`) once for each step of time compression: 2x, 4x, 8x, and let the run go on for many frames after the last press. Presence of `TORE_PERF_VEIL` forces the G-effect blackout on for alternating 30-frame blocks (off first) and reports the two groups separately, to time the vignette in any aircraft; the veil is drawn by the canvas shader, so the two groups should match.

The report separates frame-start intervals, simulation/camera work, UI composition, and submission/presentation. These are CPU wall-clock measurements: presentation includes VSync/backpressure, and frame intervals are not verified display scanout times or GPU timestamps. A short run does not establish sustained thermal performance. See [measured baseline and remaining work](baselines/flight-performance.md).

F2/F3 are the recovered look-back/look-up bindings, **not exterior cameras**. Use F10 for the aircraft chase view, or `--flight-view 2` for the diagnostic oblique view. F1 returns to the cockpit. On macOS, use Fn/Globe if function keys invoke system actions.

### Look-around checks

Shift + arrows look around in the cockpit or orbit around the aircraft externally. Cockpit Down stops at the forward eye line; exterior orbit is unrestricted in both axes. Keypad 5 or Shift-/ recenters; F1 returns to the cockpit. For repeatable captures, `--flight-look YAW,PITCH` supplies degrees in -360..360 (finite values only); internal pitch clamps to 0..90. Example: `cargo run --locked -p tore-app -- --flight-view 1 --flight-look 120,-65 --capture-flight .local/orbit.ppm`. This flag sets orientation only; use `--free-flight` or a flight capture to enter flight. See [controls and limitations](FLIGHT-CONTROLS.md#look-around-and-exterior-orbit).

### Loop regression probe

`cargo run --locked -p tore-app -- --headless-flight 10800 --maneuver loop` starts the F/A-18D at 450 KTAS / 5,000 feet with full throttle and afterburner, then holds pull until it completes a loop or reaches the tick budget. It reports vertical/inverted/completed flags and the final state. This is an authored-adapter regression probe, not native flight-model acceptance. Use `--flight-look 0,90 --capture-flight .local/zenith.ppm` to inspect the sky directly overhead; the forward cockpit plane moves out of view naturally. [Baseline](baselines/flight-response-sky.md).

### Headless flight checks

Every `--headless-flight` run ends with a `final_position:` line and an `extremes:` line: sample
count, `non_finite` (any NaN or infinite pose, speed, load or fuel), top speed, highest and
lowest load, lowest and highest altitude, peak pitch and roll rates, fuel at the start and
end, `fuel_rise_lb` (fuel that went up, which it never should), `dead_stick_gain_ft` (energy
height gained above 60 feet with the engine off, which should stay at zero) and
`energy_rate_over_thrust` (the largest one-second energy gain as a share of what full thrust
could give, which should stay well under 1), `speed_over_envelope_top` (airspeed against the
aircraft's own envelope top speed at that altitude) and `max_blackout` / `max_redout` (the
pilot's G-effect veil, 0 to 1). `--flight-trace TICKS` also prints a `trace:` line
every that many ticks. These are development harness options, not game behaviour:

- `--flight-fault TICK:INDEX` (repeatable, index 0..44) applies a system fault to the player's
  aircraft at that tick through the normal fault path and adds a `systems:` line.
- `--flight-cheat NAME` (repeatable) turns on `extra-g`, `no-g-effects`, `no-spins`, `no-crashes`,
  `unlimited-fuel`, `unlimited-ammo`, `easy-physics`, `invulnerable` or `realistic-damage`. It also applies to
  `--live-fire` captures.
- `--flight-fuel POUNDS` sets the internal fuel, to run dry on purpose.
- `--loadout none|guns` now also applies to a headless ground start (payload and fuel systems follow
  it), and a headless run ends with a `fuel_end:` line of internal and external fuel.
- The `--live-fire ... --combat-probe-ticks` line now ends with `payload_start_lb`, `payload_lb`,
  `flight_payload_lb` (what the flight model carries: the stores less the tank fuel already burned)
  and `external_fuel_lb`, so a `--combat-command jettison` run shows the mass change.
- `--flight-start X,Z,HEADING,AGL` starts the probe over a real theater (`--theater`), at that
  world position in feet, heading in degrees and height above the ground. With terrain the
  `extremes:` line adds `min_agl_ft` and `under_ground_ticks`, and a `map_extent_ft:` line gives
  the map's size.
- `--countermeasure-preview` also prints the chaff and flare counts carried against capacity, and a
  headless ground start prints a `loadout:` line (empty weight, fuel, carried stores, gross and
  maximum takeoff weight).

Scripted pilots fly the same manoeuvre in every aircraft. Each is a `fitted` test harness (agent
decision, 2026-09-28):

| `--maneuver` | What it flies | Result line |
| --- | --- | --- |
| `spin-recover` | Starts at 15,000 feet, holds pro-spin controls until the aircraft spins, holds the spin for four seconds, then follows the manual's recovery (stick centred then slightly forward, full opposite rudder, full throttle). Aircraft whose PT disables spins report `entered_tick=never`. | `spin_recovery:` |
| `stall-recover` | Starts slow at 15,000 feet, pulls until the departure alert sounds, holds it three seconds, then afterburner, nose down and wings level until the alert clears. | `stall_recovery:` |
| `land` | With `--ground-start N`: starts four miles out on a three degree slope with gear, flaps and hook down at a speed taken from the aircraft's own stall speed and landing limits, flares, closes the throttle, brakes to a stop. | `landing_start:`, `landing:` and `ils_probe:` (what the player's ILS read along the approach) |
| `land-gear-up`, `land-hard`, `land-off-runway` | The same approach with the gear left up, no flare, or lined up 1,500 feet beside the runway's footprint, to check that unsafe touchdowns crash for the reason the aircraft's landing limits give. | `landing:` |
| `climb` | Full afterburner climb holding a climb speed, then whatever the aircraft does at the top; reports the highest altitude against the 1 G envelope's ceiling. | `climb:` |
| `dive` | A full afterburner dive from 40,000 feet at 60 degrees, nobody pulling out: ends on the ground, or in the time-based overspeed loss. | `extremes:` and `loss:` |
| `overspeed` | Starts at 20,000 feet and holds 1.1 times the current top speed to isolate the failure timer: one 25% roll at 6, 7, 8 and 9 seconds, guaranteed loss at 10. This is a timing fixture, not natural acceleration. | `overspeed_ticks=` and `loss: cause=overspeed` |
| `rudder` | Engine-off flight with full right rudder, checking flight-path turning and roll coupling. | `lateral:` |
| `nosewheel` | With `--ground-start N`: rolling at 10 mph, throttle closed, full right rudder, checking taxi steering and wheel articulation. | `lateral:` |
| `takeoff-gear-early`, `takeoff-gear-airborne` | With `--ground-start N`: the `takeoff` roll with the gear key pressed at 80 knots with the wheels still down (the ground sensor refuses it), or once 50 feet above the runway (a normal retraction). | `gear_pulled=`, `ground_sensor_refusals=`, `gear=` |
| `sprint` | Full afterburner in level flight at 5,000 feet, altitude held by the autopilot. | `extremes:` |
| `autopilot` | Starts in a 25 degree bank with heading and altitude hold engaged. | `extremes:` |
| `waypoint` | Waypoint autopilot toward a waypoint 60,000 feet out, 60 degrees right of north. | `final_position:` |
| `devices` | Gear, flaps, airbrake and hook down one after another, then up again. | `devices:` |
| `eject`, `eject-low` | Shift-E twice at 5,000 feet, or at 250 feet, and the pilot's descent. | `ejection=` |

`--retail-stall-speeds` turns the [weight-scaled stall speed](spec/takeoff-ground-contact.md#weight-scaled-stall-speed)
off for the whole run (developer switch), so the imported polygon speeds apply at every weight and old
comparisons and fingerprints can be re-run. The default is the weight-scaled rule, hybrid adapter only.
The `envelope:` line then shows `stall_scale=1.000`. `TORE_RETAIL_STALL_SPEEDS=1` does the same for test runs and the
battery (for example `TORE_RETAIL_STALL_SPEEDS=1 TORE_GOLDEN_VERBOSE=1 cargo test -p tore-sim golden` prints the fingerprints without the rule).

A ground-start `--maneuver takeoff` (and the `takeoff-gear-*` variants) prints an `envelope:` line
(the aircraft's imported 1 G, 2 G and 3 G speed range in knots at the airport's altitude, the flap
lift coefficient, the loading divisor, the imported landing limit, the weight scale now, the flapped
stall speed and the loaded minimum speed for 1 G with full flaps, both scaled) and a `liftoff:` line (the
tick, speed in knots, distance in feet from the start, and the rotation speed at which the nose
first passed 1.5 degrees). See [stall and liftoff speeds](testing/lane-flight.md#stall-and-liftoff-speeds-against-the-imported-data).

Every headless flight also prints `loss: cause=...` (`none`, `overspeed` or `out of bounds`), and
flying on out over a theater edge ends in the loss 105 nautical miles past the map
([world edge](spec/world-edge.md)).

The scripted landing floats about 1,500 feet past the aim point, so on runways under about 5,500
feet it can overrun, and on the small airstrips (about 1,000 feet) it lands off the end. Only long
runways are expected to end with `stopped=true` and `crashed=false`. `TORE_WIND=heading,speed`
applies to these runs. Every ground-start airport is a flat square of at least 5,000 feet a side
(`footprint_half_ft` in `landing_start:`), and a touchdown anywhere on it counts as landing.
See [the flight lane](testing/lane-flight.md).

## Directional cockpit checks

The original forward cockpit and HUD now share a flat GPU overlay that translates opposite head-look around the aircraft-forward datum, with a side/up fade. Inspect with `--free-flight --flight-look 8,4`, `--flight-look 40,5`, and `--flight-look 0,35`; add `--capture-flight .local/directional.ppm` for a repeatable GPU capture. Check both `--window-size 1280x720` and `--window-size 640x900`. F1 restores the centered frame; F2 should not repeat forward art behind the pilot. Instrument windows and the Escape menu remain screen-anchored. See [asset limits and measurements](baselines/directional-cockpit.md).

## Aircraft animation inspection

Use `--flight-devices G,F,B,H,AB` for initial fractions (0..1) and `--flight-controls pitch,roll,rudder` for initial visual deflections (-1..1). Combine with `--capture-flight` to pause at an exact pose. Without a capture, normal actuator and control response resumes after startup. See [examples and evidence](baselines/f18-animations.md).

## Banked-pull probes

`--headless-flight 360 --maneuver bank-left` (or `bank-right`) tests three seconds of pull from a 45-degree bank and reports AoA/sideslip. `--maneuver bank-right --flight-probe-ticks 120 --capture-flight .local/bank.ppm` captures the same maneuver after one second; rendered probes are limited to one minute. See [flight-model evidence](baselines/banked-pull-aoa.md).

## Native flight helper probes

`cargo run --locked -p tore-app -- --native-flight-report` prints deterministic
static-translated helper probes using the imported Hornet profile, without a
window. This does not select a different playable flight model. Recreate the local
executable/symbol inventory with the commands in [native flight research](formats/native-flight.md).

The native flight report now prints a typed PT profile, supplied-condition stall/spin probes and ordered velocity-step probes. Its forward limit uses the reviewed 1G-envelope update at a supplied 5,000-ft altitude. It is not a native flight trajectory. No display is required; the normal free-flight adapter remains unchanged.

Use `--native-flight-trig .local/native-flight/rotations-final/tables/sine-q15.bin` after static extraction to include imported-table rotation and force probes. The flag enables the headless native report. It validates exactly 642 bytes and does not alter free-flight behavior.

Fourth-pass static composition probe (no window, audio or retail-code execution):

```sh
python3 tools/extract_native_flight.py --source gameassets/fighters-anthology --out .local/native-flight/composition-final
cargo run --locked -p tore-formats --example native_composition -- .local/native-flight/composition-final/tables/sine-q15.bin .local/native-flight/composition-final/tables/atan-pa.bin
```

The example validates table lengths, prints supplied world/cockpit angle probes,
and checks that 120 authored fixed-clock steps account for 256 native time units.
Its seeded RNG draws and sample inputs are diagnostics, not recorded native
trajectories. It does not change the app's flight model.

## Aircraft selection and briefing selectors

Quick Mission uses all recovered active scalar tables and imported aircraft names.
Click scalar text to cycle; Shift-click opens a paged list. Aircraft and theater
fields open the list directly. Aircraft choices are restricted to imported flight
profiles for F/A-18D, Rafale C, F-14D, A-4E and X-31 EFM.
See the [menu presentation spec](spec/quick-mission-menu.md). Unsupported mission
systems remain editable setup placeholders with launch validation.

```sh
cargo run --locked -p tore-app -- --quick-mission --aircraft rafale
cargo run --locked -p tore-app -- --free-flight --aircraft rafale --theater FRA
cargo run --locked -p tore-app -- --aircraft rafale --headless-flight 10800 --maneuver loop
cargo run --locked -p tore-app -- --quick-mission --snapshot-state aircraft --snapshot .local/aircraft-selector.ppm
cargo run --locked -p tore-app -- --quick-mission --snapshot-state theaters --snapshot .local/theater-selector.ppm
```

`--aircraft f18` remains the default. Quick-mission snapshot states are `normal`,
`aircraft`, `theaters`, `ordnance`, `ordnance-empty`, `ordnance-drag`, `help`,
and `debrief-1` through `debrief-5` (the retail reference result on DEBSCV);
they use the original 640×480 menu canvas. The empty/drag ordnance fixtures
support CPU snapshots for inspecting card outlines and the carried thumbnail.
`ordnance-pod` and `ordnance-pod-yellow` hang the Pave Spike on the first station that takes it with the station's blink held on its red or yellow phase, and `ordnance-authored-tank` loads the first authored tank (the F-16C's 370 gallon one); `ordnance-gun-pod` loads the SUU-16 gun pod on the first station that takes it (the F-4B's centreline); `ordnance-message` previews the Cheat notice; `ordnance-message-long` previews
the single-line notice's ellipsis and fitted background width. `ordnance-laser` (and
`lobby-ordnance-laser`) load a laser missile or bomb to show the laser warning and the
sensor helper line; `first-load` shows the creator as a first load draws it, every
aircraft slot random from a fixed seed ([first load](spec/quick-mission-menu.md#first-load-and-the-saved-draft)).
Old caches re-import when local media is present. Aircraft switching refreshes
GPU atlas/cockpit resources, camera previews and instruments before launch.
See [validation and remaining parity](baselines/rafale-quick-mission.md).

Rafale animation inspection: use `--aircraft rafale --flight-view 2
--flight-devices 1,1,1,0,1 --flight-controls 1,0,1 --capture-flight
.local/rafale-deployed.ppm` (on one command line). The fourth fraction must be
zero: the imported model has no hook. [Animation and cockpit-switch evidence](baselines/rafale-animations.md).

## Shared flight-model development

`tore-sim` has no renderer/audio/platform dependency. It owns flight state and
attitude math shared by the app and headless tools. Use `--researched-flight` to
explicitly select the default hybrid model; use `--legacy-flight` for the old adapter. Extract
and validate either reviewed aircraft with `tools/extract_assets.py --aircraft
f18|rafale --validate-flight` (choose one literal identity).
See [FLIGHT-MODEL.md](FLIGHT-MODEL.md) for complete commands, surface inputs,
acceptance scenarios, and the explicit fitted/native boundary.

Use `--flight-zoom 0.5..4` for repeatable initial zoom, including `--capture-flight`. See [cockpit sliding/zoom evidence](baselines/cockpit-slide.md).

## Live cockpit mirrors

F/A-18D and Rafale C now render all three original mirror regions from a shared
768×384 rear GPU view every visible flight frame. There is no mirror refresh
rate cap or CPU readback. The aircraft is visible in the rear feed, while its
exterior stays hidden from the forward cockpit camera. Mirrors share cockpit
pan, zoom and fade. Source silhouettes are flood-filled at runtime from reviewed
opaque fills; optics and viewpoint are fitted. See [mirror evidence](baselines/mirrors.md).

Presentation follows the [frame rate](spec/graphics-options.md#frame-rate)
choice: FIFO for V-sync (with Wayland frame callbacks), Mailbox then Immediate
with a timer for 60, 100 and 200, and Immediate then Mailbox for
`TORE_FRAME_LIMIT=off`. No compositor settings are changed. The selected mode
is logged at startup and on every change. Physics remains fixed at 120 Hz.

For a matched diagnostic without mirrors, set `TORE_MIRRORS=0` at launch; original
flat fills remain. Omit it for normal live mirrors. Combine with
`TORE_PERF_FRAMES=6030 TORE_PERF_ACTIVE=1` and the same aircraft/window size.
The report includes rear-render counts. Instrument camera pages retain their
separate asynchronous readback cadence; that does not pace cockpit mirrors.

## Controller development

See [INPUT.md](INPUT.md) for the standard Linux gamepad mapping, custom profile
syntax, instrument focus, input tapes, disconnect semantics and feedback limits.

```sh
cargo run --locked -p tore-app -- --list-inputs
cargo run --locked -p tore-app -- --monitor-inputs 30
cargo run --locked -p tore-app -- --write-input-profile my-input.conf
cargo run --locked -p tore-app -- --input-profile my-input.conf --free-flight
```

Device diagnostics require neither retail media nor a display. Generated profiles
are create-new. Store a selected profile as `input-v1.conf` in the application data
directory for automatic loading. No desktop/udev permissions or drivers are
modified. `--no-controllers` disables native device access. Windows/macOS raw
controller mappings require an explicit profile; their backend cross-checks are
not hardware/runtime acceptance. macOS 11+ supported gamepads use GameController
and CoreHaptics; generic HID feedback and directional flight-stick forces remain
open. Apple gamepad IDs are session-only; see INPUT.md for shared profiles. Test
a single capable controller with `cargo run --locked -p tore-app -- --test-rumble only`.

`tore-input` owns safe, dependency-free binding policy and typed pilot input;
`tore-input-native` isolates the native platform boundary. Continue the everyday
workspace checks above. On a Linux development host with rustup targets installed:

```sh
cargo clippy -p tore-input-native --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings
cargo clippy -p tore-input-native --all-targets --locked --target aarch64-apple-darwin -- -D warnings
```

These check native backend Rust/FFI declarations but do not link or run the complete
app on those operating systems. [Current evidence](baselines/input.md).

### In-game controls and preferences

**Escape → Control** opens the binding/rumble editor; use **Save & apply** to persist
changes. Normal sessions automatically save instrument layouts/pages and scope
settings, cockpit/HUD/zoom and sound choices. Aircraft changes and restarts retain
these choices. [Editing, file behavior and platform limits](INPUT.md).

`--controls-menu` opens the paused editor directly for inspection. Reproducible
wide/tall captures use `--controls-menu --window-size 1280x720|720x1000
--capture-flight PATH` with one literal size. Diagnostic windows are non-resizable
to preserve requested dimensions on tiling compositors; normal windows remain
resizable. Smoke/capture/performance diagnostics ignore saved display preferences.

## Combat component research

`tools/extract_assets.py --aircraft f18 --aircraft rafale --weapons` selects both
reviewed aircraft and the armament catalog. Exclude unrelated demo archives as
shown in [extraction instructions](EXTRACTION.md#fa-18d-and-weapons).
`--native-weapons` performs the separate hash-gated static code pass.
`cargo run --locked -p tore-sim --example weapon_probe -- PATH.JT` exercises
recovered scalar components from an extracted definition. These research commands do not
prove native lifecycle parity. The separate `--live-fire --aircraft f18|rafale`
app mode runs the development range. `--combat-smoke --aircraft f18|rafale` runs
its imported end-to-end suite headlessly; use one literal identity per invocation.
`--combat-probe-ticks N` with `--capture-flight PATH` captures a scripted live pass.
[Live-fire controls and validation](baselines/live-fire.md). [Evidence and remaining gates](baselines/combat-components.md).

### Manual weapon acceptance

`--combat-smoke` exercises the selected aircraft's default JT slots against all
five source damage entries, with negative launch checks and deterministic
live-state comparison. All twelve registered identities pass, and scripted probes
now wait the same half second for a fire-control track that a player does. Set `TORE_COMBAT_EVIDENCE` to a
new ignored directory to additionally write and replay per-slot combat tapes,
comparing complete state before and after manual commands/reset:

```sh
TORE_COMBAT_EVIDENCE=.local/manual-check TORE_DATA_DIR=.local/combat-implementation/app-profile cargo run --locked -p tore-app -- --combat-smoke --aircraft f18
TORE_COMBAT_EVIDENCE=.local/manual-check TORE_DATA_DIR=.local/combat-implementation/app-profile cargo run --locked -p tore-app -- --combat-smoke --aircraft rafale
```

Files use create-new semantics; choose a fresh output directory on subsequent
runs. The application-data override is optional and must point to an imported
profile or permit import from local media. Runtime `--record-combat NEW_PATH`,
headless `--replay-combat PATH`, and `--combat-command NAME` capture setup are
specified in [manual weapons acceptance](baselines/manual-weapons.md).

The [systems continuation](baselines/weapons-systems.md) adds incoming/player-damage
and ECM checks to the combat smokes, combat tapes, `--jammer-on`,
and `--combat-command damage|incoming|target-jammer`. Tapes are now version 3:
each record also carries the player's sensor controls, and a designation is
stored as `designate-id:N` rather than a screen coordinate. Version-2 tapes still
replay with the default sensor controls. Probe logs include haptic
event/mixer counts without playing historical pulses on hardware. Use a new
`TORE_COMBAT_EVIDENCE` directory for ten serialized slot tapes. Current native
research has 28 reviewed regions; full subsystem/ECM parity remains open.

## Creator / ordnance acceptance

```sh
cargo run -q --locked -p tore-app -- --sensor-summary
cargo run --locked -p tore-app -- --validate-creator
cargo run --locked -p tore-app -- --validate-weather
TORE_WEATHER_TIME=19:06 cargo run --locked -p tore-app -- --capture-terrain .local/weather/dusk.ppm
TORE_VAPOR_PROBE=1 cargo run --locked -p tore-app -- --free-flight --maneuver pull --flight-probe-ticks 400 --smoke-test
cargo run --locked -p tore-app -- --weather-condition 1 --capture-terrain .local/weather/cloudy.ppm
cargo run --locked -p tore-app -- --quick-mission --snapshot-state ordnance --snapshot .local/ordnance.ppm
cargo run --locked -p tore-app -- --quick-mission --snapshot-state ordnance --smoke-test
```

`--sensor-summary` needs imported media but no display or audio. It prints one
line per registered aircraft: the installed radar record with its search and
track volumes, look-down coefficient and assigned preset; the infrared and visual
records; the ECM record with its assigned generation and strength; and the PT
radar and infrared signatures. Reviewing that output is the expected cost of
porting an aircraft's sensors. For repeatable headless captures,
`--sensor-channel radar|ir`, `--scope-range 5|10|25|50|100|150` and
`--scope-history` set the scope before the capture. See
[the component guide](radar.md) and [its validation](baselines/radar.md).

The Quick Mission ground-target/AAA/SAM sentence is live: the target list is the
theater's, the strengths read "not", "lightly", "moderately" and "heavily", and
the mission text carries them (`ground-target`, `defenses`). The enemy-distance
choices include 100 and 150 nautical miles between 50 and 200. `--separation`
accepts the same choices; a creator snapshot or launch moves one its theater,
target and start do not offer as the creator does ([the distance
table](#quick-mission-distance-table)). See the
[creator contract](spec/quick-mission-menu.md#ground-target-and-defenses).
Preview a theater's target line or the distance list without a display:

```sh
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --quick-mission --theater EGY --snapshot-state ground-target --snapshot .local/ground-target.ppm --no-audio
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --quick-mission --snapshot-state field-17 --snapshot .local/separation-choices.ppm --no-audio
```

`--validate-creator` needs imported media but no display/audio, and checks all
imported aircraft's supported placements, fuel, empty stations and accepted-ammo restart.
It first checks guns-only launch/restart across all six wings and the ordnance drag
paths for the full selectable roster, then every removed-store case (each station off
one at a time, all off, all externals off) against the flight inventory, weapons window
and selection. After the loadouts it sweeps the whole Quick Mission creator
(every theater layout, weather choice, player and wing aircraft, skill, wing size, group
order, separation and start runway through the same start steps a flown mission takes),
draws every dropdown value and popup page, and fuzzes keys and clicks on the creator and
the Load Ordnance page. `TORE_CREATOR_STAGE=loadouts|matrix|render` runs one part.
`--loadout none|guns` starts a flight, or a `--launch-quick-mission`, with every store off, or everything
but the gun off, as the Load Ordnance page leaves them; the launch line prints the ammunition
and the weapons window's list. See the [current results](baselines/ordnance-presentation.md)
and the [menus lane](testing/lane-menus.md).

Load Ordnance shows only imported weapons with connected flight support. Normal
loading also requires compatibility with at least one aircraft station. Weapons
→ Cheat unloads all stations and shows every supported imported weapon; toggling
it off unloads again and restores the normal catalog. Both category pages and
the selected catalog card reset on each toggle. See
[catalog availability](spec/ordnance-presentation.md#catalog-availability) for
placement limits and [future weapon passes](ROADMAP.md#weapon-catalog-update-passes).

In Load Ordnance click a catalog weapon then a compatible station, or drag between
them. The weapon thumbnail follows the pointer. Drag from station to station
to transfer ammunition, or from a station into the catalog to empty it.
Left-click a loaded station to add one (a pair on a paired station), or an empty station to load the selected
catalog weapon. Tab changes selected station; +/- changes ammunition;
right-click decrements.
Fuel rocker edits 500 lb at a time. Successful edits play the original weapon,
ammunition or fuel sound; canceled and unchanged edits are silent. Older caches
refresh these samples from local media through the existing import path.
Select Plane preserves the custom draft.
[Evidence, hands-on steps and material limits](baselines/creator-ordnance.md).

### Weather implementation audit

The [2026-09-15 review](baselines/weather-review.md) records corrections to the
initial weather implementation and remaining parity gaps. The creator exposes
six weather rows; overcast is omitted as a duplicate of cloudy. CLI source
indices remain `0..5` (clear, cloudy, foggy, dawn, sunset, night).
`tools/inspect_shape_effects.py EXTRACTED.SH` emits a static import/re-entry
candidate inventory; optional `--disassembly NEW_LOCAL_FILE` requires GNU
objdump. It never executes imported shape code and does not prove effect absence.

### Weather inspection cameras

`TORE_WEATHER_VIEW=x,y,z,yaw,pitch[,roll]` sets the terrain viewer pose in feet and
absolute degrees for reproducible sky/deck captures. It does not change the
flight camera. Optional roll permits reproducible celestial bank checks. For example:

```sh
TORE_WEATHER_VIEW=1070000,5000,590000,45,20 cargo run --locked -p tore-app -- --viewer --weather-condition 5 --capture-terrain .local/moon.ppm --no-audio
```

`TORE_CLOUD_ALTITUDE=0..400000` overrides the cloud-sheet altitude in feet for
crossing captures. Generated weather choices use the recovered scattered-cloud
chance; normal MM launches preserve their `clouds` field. Caches predating the
inert cloud layout automatically re-import when local reviewed media is present.

`TORE_SUN_GLARE=0|1` disables/enables the recovered glare and palette whitening
(default on in this host adapter). In flight, **Escape → Cheat → No sun
whiteout? → On** suppresses both whitening and lens flare immediately, including
while paused; the sun itself remains visible. The cheat lasts for the session
and across restarts, without changing saved preferences. `TORE_SUN_GLARE=0`
keeps glare disabled regardless of the cheat. Camera-specific weather sampling
and validation are described in [weather cameras](baselines/weather-cameras.md).

`TORE_CLOUD_DETAIL=0|1|2` selects recovered cloud candidate placement (default 2).
Levels 0/1 use the base period; level 2 uses the 4x4 repeat. Native coordinate
range and view-sector gates can make the visible results identical in forward
views. Use a downward view to inspect copies around the aircraft. This override
is cloud-specific; the lower-detail terrain/sky raster is not selected by it.


Weather captures use `--capture-terrain` for the viewer and `--capture-flight`
for flight with overlays. `--capture-terrain` selects the viewer even when
`--free-flight` occurs earlier; use the flight flag for aircraft/cockpit evidence.
The latest ignored `.local/weather-continuation/final/manifest.json` records
commands, explicit weather poses and result codes for the continuation captures.
Weather sky/ocean, moon and cloud textures now use source point-index samples;
float GPU projection remains qualified against native coverage/behavior.

Every airport is drawn and flown from its runway type's real-size redrawn
plan ([redrawn airports](formats/redrawn-airports.md)), the default since
protocol 24. `TORE_REDRAWN_AIRPORTS=0` is a developer escape hatch: single
player launches and the offline tools (`--surface-dump`, `--surface-parked`,
the previews, `--validate-maps`, `--validate-ils`, `--airport-allegiance`)
build the retail airfield shapes instead; unset or `1` keeps the redraw.
Online sessions, the dedicated server and the bot never read it (every game of
a session flies the redraw), and a replay builds the airfields its recording
names, whatever the switch says.
`--airfield-sheets OUTPUT_DIRECTORY [--all] [THEATER ...]` renders one redrawn
airport of each plan (`--all`: every one) overhead, oblique, from short final,
along the parking row, toward the runway's far end and over a taxiway junction;
with `TORE_REDRAWN_AIRPORTS=0` it renders the same views of
the retail airfields. Redrawn airports carry runway, approach, PAPI and taxiway
lights; `TORE_AIRFIELD_LIGHTS=0` hides them.
`--airfield-audit [--svg DIRECTORY] [--track FILE ...] [--field CODE:AIRPORT ...] [THEATER ...]`
(imported media, no display) walks every AI ground route of every redrawn
runway object of every layout (or the theaters or fields named) against the
drawn pavement, checks every runway end is reachable from parking and every
runway line (the ILS) lies on a drawn runway, and prints `PROBLEM` lines and a
closing count; `--svg` draws each airfield overhead with its routes and
`--track` adds AI probe tracks. `--airfield-audit --parked` checks the Quick
Mission templates' parked aircraft against the pavement instead
([audit](formats/redrawn-airports.md#audit)).

The [city fill](features.md) is on by default everywhere since protocol 25.
`TORE_CITY_FILL=0` is a developer escape hatch like the airports' one: single
player launches and the offline tools build no fill; online sessions, the
server and the bot never read it, and a replay builds the fill its
recording's `world.city_fill_scene` names. `TORE_CITY_FILL_CAP` changes the
number of buildings in single player and the offline tools (online games
ignore it) and
`TORE_CITY_FILL_SHADOW_NM` overrides just the city caster distance for
measurements. Use the Graphics Shadow distance row, `--shadow-distance
trees|1|2|4|6|8` or `TORE_SHADOW_DISTANCE` for the shared map and caster reach
([rules](spec/graphics-options.md#shadows)). The offscreen aid
`--city-fill-dump OUT [THEATER]` (no window, nothing simulated; options in
`crates/tore-app/src/city_fill_dump.rs`) writes mask and fill images and
takes `--render` (a still, fill off and on), `--target` (the target
display's view), `--sequence` (every frame along a path, for watching
transitions), `--sweep` (a still camera with one tier moving: building
detail, building distance or smallest size, printing the pixels that pop per
step) and `--bench` / `--target-bench` (frame times fill off then on, 4K by
default); `--burn NAME@X,Z,H,PITCH,YAW,RADIUS,FRAMES` knocks every building within RADIUS ft flat, draws the burning city and times FRAMES frames with the fires drawn and with them off. `TORE_CITY_DUMP_TIME=HH:MM` sets the dump's time of day (noon by default), for the burning city at night (nr-fx3). `--city-fires THEATER X,Z,RADIUS [--repeat N]` (no window, no GPU) prints how many burn and what a frame draws and costs on the CPU; `TORE_NO_CITY_FIRES=1` stops drawing the fires (a developer aid like `TORE_CITY_FILL=0`). The fade work was measured with `--sweep` and `--bench` over Kiev
(UKR, `--bench 671744,1330000,500,0,100,600`).

`TORE_WEATHER_SMOOTH=0|1` selects stepped or smooth weather presentation (default
1). Smooth mode blends source time/altitude colors and neighboring horizon/fog
shades. It uses fractional mission time, freezes on pause and does not change
simulation/callback scheduling. `--validate-weather` also reports native/smooth
palette change counts and maximum channel steps over a minute of dawn.
[Evidence and celestial sizing qualifications](baselines/weather-smoothing.md).
Smooth weather also adds an opinionated directional sun halo, dawn/dusk wash,
and per-pixel angular lighting on sky-deck textures and cloud sheets. Its visual
sun arc runs continuously through the day, with symmetric dawn/dusk twilight
gradients rather than a 19:00 drawing cutoff. The Sunset preset remains 19:01.
The sun and moon use half their previous apparent diameter (shared scale 2).
Decks, horizon gradients, haze tables, stars and moon fade through the dawn
and dusk record overlaps; the night sky turns, with a procedural starfield and
a moon that rises and sets ([spec](spec/sun-glow.md#night-sky)).
The trial orange-rim grade is removed; the earlier atmospheric glow remains. Smooth-mode glare
is gentler near the horizon and drops rapidly to zero by 0.5 degrees below it.
See [sun glow specification](spec/sun-glow.md).

Smooth weather also adds gentle distance and altitude-dependent horizon blending,
with extra haze along sightlines through moist weather bands. See the
[atmospheric distance specification](spec/atmospheric-distance.md).
Dense cloud layers now suppress residual surface colors, and cloudy weather
without an ocean deck receives original-art ripple reflections. See
[cloudy presentation checks](baselines/cloudy-presentation.md).

### Wind, turbulence and attachment probes

`TORE_WIND=heading,speed` supplies whole degrees (0..360) and feet/second
(0..200); `0,0` is explicit calm. Without an override or mission line, native-order
heading/speed draws use an isolated launch seed of 1. Both adapters start with
the wind added to ground velocity to preserve starting TAS.
`TORE_TURBULENCE=0|1` sets the initial session preference; the flight Cheat menu
can change it and restart preserves it. `TORE_FLIGHT_AGL=10..90000` sets
diagnostic starting height above terrain, not a validated runway.

```sh
TORE_ENVIRONMENT_PROBE=1 TORE_FLIGHT_AGL=100 TORE_WIND=90,20 target/debug/tore-app --free-flight --aircraft f18 --flight-probe-ticks 600 --no-audio
TORE_VAPOR_PROBE=1 TORE_WIND=0,0 target/debug/tore-app --free-flight --aircraft rafale --maneuver pull --flight-probe-ticks 400 --smoke-test --no-audio
```

The environment probe exits before window creation and prints resolved wind,
live AirData, turbulence state and final pose after the full fixed-tick service.
It differs from the isolated `--headless-flight` dynamics probe. AirData uses
a declared standard atmosphere; unavailable IAS/CAS/indicated altitude remain
unavailable. The vapor probe includes each raw CE point and nearest neutral
mesh vertex. [Acceptance](baselines/wind-turbulence-vapor.md).

### Flight-response diagnostics

The renderer-independent `response_probe` example covers both adapters for each
provided extracted PT, including applied G/rates, roll/rudder release, stall/spin
and recovery, low/high speed, devices, payload and full loops:

```sh
cargo run --locked -p tore-sim --example response_probe -- .local/aircraft/f18/FA_2.LIB/F18.PT .local/aircraft/rafale/FA_2.LIB/RAFALE.PT
```

Set `TORE_RESPONSE_TRACE=.local/response-traces` to record per-tick state. Traces
are source-derived local artifacts; do not commit them. See [conditions/results
and remaining gates](baselines/flight-response.md). This complements the hybrid
`--validate-flight` extraction suite; it is not a retail trajectory oracle.

### Native departure research

Use `cargo test --locked -p tore-formats tumble` for initial synthetic
native tumble/fall branch checks. The existing static extraction command now
includes their scheduling/movement slices. This diagnostic component is not
called by either live adapter. [Evidence and open gates](baselines/native-tumble.md).
Apply [behavior provenance](behavior-provenance.md) when interpreting results.
In research mode, identifying the source, testing the translation and writing the
spec are separate steps. "Runtime connection" is retired as a completion column,
and retail comparison is unavailable.

The `native_departure` example joins warning/stall/spin/tumble branches with
native movement composition. Pass extracted sine table, atan table and one or
more reviewed PT files; see [commands and scope](baselines/native-departure-stage.md).
It uses explicit scripted native-time inputs, not the full flight scheduler or
normal force update. It adds no live flight mode.

The departure example also evaluates separate native force/velocity snapshots
from each departure output, using explicit empty/fuel-off inputs. These are
component probes: they do not feed velocity back into a full flight trajectory.

`cargo test --locked -p tore-formats normal_control` and
`cargo test --locked -p tore-formats movement_stage` check the diagnostic primary
control and movement/contact contracts. `native_departure` now evaluates paired
force→movement snapshots with both PTs; it still does not run a full native flight.
[Scope and results](baselines/native-movement-control.md).


`native_flight` now joins loaded controls, departure, forces, movement and contact
into recurrent diagnostic updates for F18 and Rafale. Unlike `native_departure`,
it feeds position, velocity and control state back into subsequent services.
Pass the same sine/atan/PT arguments. It requires native environmental turbulence
disabled and explicit caller samples; it adds no live adapter or terrain producer.
[Reproduction, scenario coverage and remaining gates](baselines/native-flight-diagnostic.md).


### Airborne native research flight

`--native-flight-tables DIR` connects the joined service to live free flight;
DIR supplies bounded `sine-q15.bin` and `atan-pa.bin` files from the static native
extraction pass. Use it with either `--aircraft f18` or `--aircraft rafale`, and
with `--headless-flight`/`--flight-probe-ticks` for repeatable checks. It is mutually
exclusive with hybrid and combat modes. Two limits belong to this restricted
research path only: contact stops the run, and environmental turbulence is
unavailable here. Existing device/fuel/clock adaptation remains explicit.
[Commands, restart/failure semantics and acceptance](baselines/native-live-flight.md).
`cargo run --locked -p tore-sim --example native_live -- SINE ATAN PT [PT]` runs
both-aircraft live-API replay checks independently of the importer hybrid suite.

## Ocean motion inspection

Ocean decks use the original ocean and sky textures and weather palette, with
short procedural ripples and angle-dependent reflection. Close detail is
pixelated and blends to smooth sampling with distance and altitude. Smooth
reflection and fixed-size ripples blend back into the original water sample
from 2,700 feet to five statute miles of horizontal distance. Reflection strength
also follows that fade, reducing distant highlight contrast. Whitecaps
are removed. `TORE_OCEAN_MOTION=0` restores static ocean sampling for comparison;
the default is `1`. `TORE_OCEAN_PHASE=SECONDS` freezes motion at a finite phase in
`0..120` for captures. Ordinary playback uses shared simulation time, including
pause/restart semantics, in the viewer, flight, mirrors and camera instruments.
No additional texture import is required.
[Behavior and provenance](spec/ocean.md), [checks and commands](baselines/ocean.md).

Smooth water reflections separate an 85% sun peak from the sky/cloud peak.
`TORE_WATER_ENV_REFLECTION=0..1` overrides the selected sky/cloud default of 0.3; viewing angle, distance and weather reduce actual contributions. See [reflection validation](baselines/reflection-luminosity.md).

The sun reflection has independent long-range angular scatter and follows the
visible fraction of the sun disc. Its reach no longer uses the five-mile
sky/cloud reflection fade; dense weather still obscures it.

Missile reach probes use `--missile-acceptance --aircraft NAME` and print measured
hits, acquisition inhibits and expiry misses. These are controlled fixtures,
not guaranteed effective ranges. `--compatibility-weapons` with `--live-fire`
selects the prior weapon adapter independently of the flight model. The
`seeker-mode`, `target-heat:0..4`, `target-radar`, `target-distance:FEET` and
`empty-range` combat commands support deterministic fixture capture and replay.
Heat codes are unknown, off, idle, dry and afterburner respectively. Distance is
bounded to 1..1,000,000 feet. Version-4 tapes record these commands, full world
velocity and bay permission. Use fresh tape paths because recording never
overwrites an existing file. [Missile acceptance](baselines/missiles.md).

### Missile study

`--missile-study OUT_DIR` (first argument) is a headless Monte Carlo of
air-to-air shots for studying missile accuracy and countermeasures. No window
opens. Each shot builds a fresh combat state: the shooter (`--shooter`, default
F-16C) flies straight and level at 0.9 Mach, designates the target through its
own radar, waits for its mounted IR seeker where there is one and fires the
study missile through the player trigger; the real combat step flies the
missile. The target (`--target`, default MiG-29, with its imported signatures,
hit points and ECM record) is scripted, and each profile in `--profiles` names
what it does: `straight`, `break` (7 G onto the missile's beam 3 to 5 seconds
before impact), `beam` (5 G onto the radar source's beam after 1 to 3
seconds), `drag` (5 G away from the missile), with devices `cm-ai` (the AI's
two-device bursts every 2 seconds inside 6 seconds), `cm-hammer` (four a
second from one second after launch until empty) or `cm-atNs` (one device N
seconds before closest approach), joined with `+`. `dive` descends at 25 degrees
to a 100 ft deck. `@idle` and `@burner` set constant engine heat; `@off` cuts
heat after the seeded 1 to 3 s reaction delay, allowing an initial hot lock.
Scripted speed is controlled independently, so this isolates the heat effect.
Flares meet IR seekers and chaff radar seekers. Energy is the study default;
`--missile-model spec` selects the historical comparison. Energy releases use
the live physical-device scene and its actual one-roll-per-episode log.
Scripted studies skip advisory prediction work, which cannot affect their
player-triggered release or missile motion; AI studies retain it.
`--ranges short,medium,long`, `--aspects head,quarter,beam,tail` (quarter is
45 degrees from the tail heading), and `--altitudes deck,low,co,high,up,down` set the cells.
Deck/low/co/high are 500/5000/15000/35000 ft for both aircraft; up/down retain
the earlier unequal-altitude cases. Range bands retain the decoded historical
values to make old/new study comparisons reproducible. Additional `near` and
`close` bands use 0.25 and 0.5 nmi, each raised to 1.1 times the decoded launch
minimum before the same range jitter, for studying physically reachable short
shots. They are opt-in and do not change the historical bands. These are distinct
from the public-anchor calibration sweep. `model.txt` records the selected
rules and the model fingerprint (physical and motor data, derivation rules,
force/drag curves, seeker and decoy source). `--seeds N` the shots per cell (range, aspect, speed and reaction time are
jittered per seed, deterministically), `--threads N` the workers.
`--ai [--skill novice|average|experienced|ace]` flies the same shots against
the game's own AI on weapons hold in a fresh Quick Mission world instead, so
it perceives, maneuvers and dispenses as in a mission; `--ai-model
standard|hybrid` picks the single-player (default) or networked AI flight
model and `--trace` prints the AI target's flight and defence every quarter
second, plus every actual device release with its current closest-approach
estimate, maneuver and passage flag. The AI study currently repeats its fixed countermeasure random stream across
shots, so seed-to-seed hit counts are not independent survival samples. Use its
G, heading and release-time traces to assess defensive control until that stream
is replaced. `--decoy-model lure` is available only with `--missile-model spec` and is
a what-if, not game behaviour: a decoyed missile steers for the device that
decoyed it and bursts there instead of flying straight on. `--catalog` writes only
`catalog.csv` and exits (each missile's decoy susceptibility, seeker and turn rates, each
aircraft's dispensers). Output is `runs.csv`, one row per shot, and
`cells.csv`, one row per cell with Wilson 95 percent intervals. Runs also
record the first observed loss of control speed, ground impact and Energy
seeker notch state while enabled and controlled (first time and total duration)
separately;
absence of an earlier control-loss sample does not prove a cause for an impact.
AI trials explicitly reapply weapons hold after mission group objectives and
attribute hits through the launched projectile's ledger outcome.

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --missile-study .local/study \
  --missiles AIM9M,AIM120 --profiles straight,cm-ai,break+cm-ai --seeds 60
```

`--missile-sag --no-audio` measures coasting A2A and SAM motion without a window.
Use a private profile copy. [Probe and battery contract](testing/lane-flight.md),
[before/after measurements](baselines/missile-sag.md).

For a rendered formation check, use `--dummy-aircraft f18,5 --dummy-aircraft rafale,5`
with `--capture-flight .local/formation.ppm`. This repeatable diagnostic uses the
same dummy creation/rendering as the creator, one mile ahead. It accepts a count
without the creator's five-per-wing selector limit. Range, research and replay
modes have their own fixtures and cannot be combined with this diagnostic.
`--validate-creator` now also checks normal stores and 29-dummy reset/model geometry
for each supported identity. Custom mission recording remains unavailable.

Damage/smoke captures use `--damage-preview 0.6 --flight-view 2 --capture-flight
.local/damaged.ppm` on one command line. This explicit diagnostic sets player and
fixture health and advances two seconds before capture (override with
`--damage-preview-ticks 1..7200`); it cannot record/replay
or use native research flight. For motor smoke use `--live-fire --weapon-slot 2
--combat-command seeker-mode --combat-probe-ticks 100 --flight-view 2
--capture-flight .local/motor-smoke.ppm`. Runtime smoke comes from `SMOKE.PIC`;
`SMOKE.SH` remains unexecuted. [Fitted rules](spec/damage-smoke.md).

Chaff and flare captures release a device with `--combat-command chaff` or
`--combat-command flare` (repeat for both), then advance flight and combat
with `--countermeasure-preview TICKS` (1 to 7200) before `--capture-flight`.
The preview prints each flare's height above the ground. External views need
`--flight-view 1` and an orbit such as `--flight-look 165,-8`; a positive
orbit pitch looks down from above. Add `--weather-condition 5` for night,
`TORE_WEATHER_TIME=13:00` for midday and `TORE_FLIGHT_AGL=120` to see flare
light on the ground. Example: `--free-flight --combat-command flare
--countermeasure-preview 60 --flight-view 1 --flight-look 160,-12
--capture-flight .local/flare.ppm`. See
[countermeasure presentation](spec/countermeasures.md#presentation).

## AI regression probes

`cargo run --locked -p tore-app -- --ai-roster-probe-ticks 3600 --no-audio`
runs imported aircraft at all four experience levels, using all twelve exact
identities. It checks finite motion and exact replay of aircraft inputs, measures achieved
heading/bank rates, and
reports sensor fit, stores, launches and dropped launches. This is separate
from synthetic tests and is not a retail comparison or visual acceptance.
The shorter `--ai-probe-ticks N` retains the Quick Mission bridge probe.
It runs the live game's whole mission tick (`World::step`, see
[Mission core and seats](ARCHITECTURE.md#mission-core-and-seats)), driven by
the scripted pilot, attack, wing orders and threat fixtures instead of a
player. Compared with the reduced probe it replaced, it now steps the weather
clock, turbulence on the scripted player, building contact, the airport
service, the crew voice, the jolts and destruction of a hit without needing the
attack script, and the runway surface with wind for airborne flight. The
scripted player's world-edge loss is the tick's, checked after turbulence. Wing
orders and threat fixtures land at the start of their tick. It has no audio,
HUD or rumble. Its numbers changed once for this in the commit that switched
it; probe outputs from before that commit are not comparable with later ones.
For ground operations, combine it with `--ground-start AIRPORT`,
`--probe-wing-size 1..5` and `--maneuver takeoff`. Add `--probe-wing-only`
to omit the other wings. Those two wing options also apply to
`--launch-quick-mission` for matching creator captures. Schedule an order with
`--probe-wing-order TICK:land-selected`, `TICK:bug-out`, `TICK:attack-on-contact`,
`TICK:engage-my-target` or `TICK:sort` (the lead deals each wingman a bandit from the data link's picture), each with an optional `@MEMBER` (1 to 4, the first wingman is 1) to order one wingman; `--probe-player-lock TICK:ID` has the scripted leader designate aircraft `ID` at that tick, so the sensors lock it when they can; `--probe-data-link` and `--probe-player-lock` print a `t=T data link:` line for each plane's radar flag and each lock taken or dropped in the flight data link's picture; `--probe-blind-wing` takes the radar, infrared sensor and eyes from the player's wingmen, so only the data link can show them an enemy; `--probe-trace SECONDS`
prints each wingman's airfield phase and position. At a redrawn airport an AI aircraft on its wheels in an airfield phase off the drawn pavement is an `off the pavement` anomaly; `--probe-pavement` also prints `AI probe pavement: ground_samples=N off_pavement=M`, and `--probe-track FILE` writes every sixth such tick as `tick label phase leg x z on|off hdg= track= fps=` for `--airfield-audit --track`. `--probe-player-home FROM:UNTIL`
flies the scripted leader gear down toward the field during that tick range.
`--probe-lose-player TICK` crashes the scripted leader's aircraft at that tick, so that a probe can test the wing's new lead (test harness only).
`--separation 200` or `300` also exercises the expanded enemy-distance choices.
`--probe-fight FRIENDLY:ENEMY` sizes a whole battle, 1 to 15 aircraft a side filled five to a wing (the scripted leader counts as one friendly), and `--probe-friendly-aircraft ID` picks the friendly AI aircraft; `--probe-enemy-aircraft` and `--probe-enemy-skill` still choose the enemy. Fifteen against fifteen is the creator's own limit. With `--launch-quick-mission` the same option sets the creator's wings instead, so a window can fly the whole battle: it is a developer measurement option, used to time the frame loop with the biggest mission the creator allows (`--launch-quick-mission --probe-fight 15:15`, [windowed runs through `tools/agent-run.sh`](#windowed-runs-from-scripts-and-agents)).
The scripted leader is only a test harness. It holds the runway line on the roll, keeps its speed over the aircraft's own 1 G minimum in the climb, clears the terrain ahead and climbs again from its cruise if the ground rises to meet it ([the scripted pilot](testing/lane-ai.md)).
[Reproduction and limits](baselines/ground-start.md#whole-wing-ground-start-2026-09-23).

Quick Mission AI leaders can release their wings on a currently observed hostile
when the assigned mission permits engagement. Use `--ai-mission hold` to isolate
self-preservation without offensive fire. `--probe-attack TICK[:SECONDS]` makes the scripted leader
attack from that tick with the player's own controls: it clicks the nearest
hostile aircraft on its scope, steps `]` to the longest-reaching air-to-air
weapon whose envelope holds the range and fires when the readout says READY.
It changes to the gun when a missile gets no lock for two seconds, and fires
the gun only with the target under the pipper. With SECONDS it attacks again
that long after each shot. The enemy wing can release on permitted contact or a perceived attack;
the player's own wingmen follow a human leader and join only when ordered, for example with `--probe-wing-order TICK:attack-on-contact`. The probe
then prints each designation, shot, perceived attack, release, missile
defence, kill and ejection, and an `AI probe attack:` summary line. Without the
option the probe is unchanged.

### Visual contact and incoming-fire probes

`--probe-enemy-aircraft ID` independently selects the opponent, using the exact
roster identities. `--probe-enemy-skill novice|average|experienced|ace` selects
its resolved skill. `--probe-geometry head|side|rear` sets initial enemy heading;
`--probe-fault TICK:INDEX` delivers a controlled fault to the first enemy through
its combat fault counts and the ordinary AI damage bridge. It is bounded to
indices 0 through 44 and at most 64 combined fault/threat injections. Useful
cases: pilot 34, compressor 7, oil pump 12, flameout 4, weak structure 30,
engine fire 11, stuck throttle 29. This is a developer fixture, not a weapon hit
or proof of a fault's combat probability. The probe prints final fault counts,
recovery phase and physical throttle/power/temperature. [Validation](baselines/ai-damage-response.md).

`--probe-guns` keeps the player attack script on its gun.
`--probe-ai-guns-only` restricts AI stores to their carried gun without changing
the player script. It is useful for sustained [gun-employment checks](baselines/ai-gun-implementation.md). `--probe-trace SECONDS` also
prints enemy observation sources, permission, target and incoming-fire cues. `--probe-fuel-trace SECONDS` prints one `AI fuel:` line per AI aircraft at that interval (tick, aircraft, fuel, endurance, time home, fuel state, activity). `--probe-fuel POUNDS` starts every AI aircraft with that much internal fuel, so a return on low fuel can be probed in a minute. `--probe-rotor-trace SECONDS` prints one `AI rotor:` line per AI helicopter or V-22 at that interval: height over the ground, speed, climb, pitch, bank, heading, nacelles, collective, rotor speed, the rotor adapter's regime and commanded heading, path, speed, climb and nacelles, the requested path, the terrain floor and the envelope maximum (slice HF1).
Structural damage without a system fault also prints an `AI damage:` line,
with its exact `damage_fraction`, so a damaged crash is not labeled undamaged.
That value is the aircraft's damage before any crash: a ground impact sets
the flight's damage to 1, which would otherwise hide an undamaged aircraft
that flew into the ground.
Every probe prints first visual acquisition, first release, first gun/hit defense
and peak bank for each enemy.

The airborne probe retains its legacy adapter unless explicitly given
`--probe-flight-model researched`, which enables the researched adapter for the
scripted player and every AI actor. `legacy` selects the earlier path.
`--probe-ai-flight-model standard|all-hybrid` sets the mission's AI flight model
on its own: `standard` (the default) is single player as it is, and `all-hybrid`
puts every AI aircraft on the researched adapter at mission start, seeded the
same way. These probe flags do not change normal game defaults or the
native-table research path.

`--probe-threat TICK:hit|gun|aaa` adds a controlled event against the first enemy.
`hit` removes one hit point and supplies the same victim event as weapon damage.
`gun` starts an anonymous tracer 1,500 ft behind the aircraft; `aaa` uses that
horizontal point 10 ft above the terrain. Both use the player's imported gun
with a fitted 3,000 ft/s initial speed and 50 ft lead offset. These are repeatable
victim-response fixtures, not autonomous surface gunners. Up to 64 may be
scheduled; every fixture is named in the replay header and event stream.

```sh
TORE_DATA_DIR="$PWD/.local/dev-profile" cargo run --locked -p tore-app -- \
  --ai-probe-ticks 1200 --aircraft f22 --probe-enemy-aircraft su27 \
  --probe-geometry rear --separation 1 --ai-mission hold \
  --probe-threat 120:aaa --probe-flight-model researched \
  --record-mission .local/su27-ground-fire.tore-replay --verify-render --no-audio
```

`--probe-matrix NEW_DIR` runs and records one encounter per case with one imported
world: F-22A/F-22N/F/A-XX by every opponent the AI can fly (31 of the 37 selectable
aircraft, see [AI wingmen](spec/variety-flight.md#ai-wingmen)), four skills, three initial headings
and both flight adapters. Use `--ai-probe-ticks 360 --separation 1 --verify-render`
for close-contact acquisition, or a larger tick budget and the attack options
for firing. It refuses an existing output directory. The usual actor, trace and
verification lines are grouped under `AI probe matrix: CASE`. This is an
explicit development batch, not a retail comparison or automated balance score.

### AI test settings in the probe

`--ai-observe-hz 120|60|40|30|20` and `--ai-lean-sensors [on|off]` run the probe
with the AI test settings described under
[AI test settings](ARCHITECTURE.md#ai-test-settings-observation-rate-and-lean-sensors):
AI aircraft observe at that rate (staggered across ticks; their controllers and
flight stay at 120 Hz), and with lean sensors (the default; a bare flag means
on, `off` gives the full sensors) skip the visual channel and build the
received-radar list only when a warning needs it. The default rate is 60 Hz
since optimization round 2 (a matrix of 240 fights behaved at 60 Hz as at
120 Hz within the fights' own chaos; see
[the baseline](baselines/performance-threads.md#ai-observation-rate-matrix)), so
a run without either flag differs from a build before that change;
`--ai-observe-hz 120` gives the earlier run, its stdout and its checksum (lean
sensors decide as full ones do). With a setting off its default the probe adds
an `AI probe test settings:` line. Compare
runs by the probe's own outcome lines: `AI probe attack:` (`lost_friendly`,
`lost_enemy`, `destroyed`, `decoys_used`), `AI probe encounter:`
(`first_visual`, `first_engage`, `first_gun_defense` per aircraft), the
`AI probe debrief:` line and the `destroyed:` and `ejected` lines. The same
two settings are in the flight menu's Debug tab (Escape, then Debug).

The probe runs its ticks flat out, so the load-aware shared workers (see
[shared scoped workers](ARCHITECTURE.md#shared-scoped-workers)) stay serial
unless `TORE_WORKERS=N` forces a count; the checksum is the same either way.

```sh
TORE_DATA_DIR="$PWD/.local/dev-profile" target/release/tore-app --ai-probe-ticks 2400 \
  --probe-fight 15:15 --separation 5 --probe-attack 600:10 --no-audio \
  --ai-observe-hz 30 --ai-lean-sensors off
```

## Mission recordings for debugging

Every flight in the game records itself into `replays/` in the data folder
([mission replays](REPLAYS.md)). A headless AI probe records only when asked,
which is the fastest way to reproduce a problem and read it back:

```sh
TORE_DATA_DIR="$PWD/.local/dev-profile" cargo run --locked -p tore-app -- \
  --ai-probe-ticks 7200 --separation 5 --probe-attack 600:10 \
  --record-mission .local/headless/attack.tore-replay --verify-render --no-audio
```

Recording does not change the probe's output; `--verify-render` adds one
line saying whether every recorded tick rebuilds the picture the probe drew,
within the format's precision, and the released chaff and flares exactly. Then read the recording back without media:

```sh
cargo run --locked -p tore-app -- --recording-info .local/headless/attack.tore-replay
cargo run --locked -p tore-app -- --recording-log .local/headless/attack.tore-replay --rate 10
cargo run --locked -p tore-app -- --recording-acmi .local/headless/attack.tore-replay
cargo run --locked -p tore-app -- --recording-diff before.tore-replay after.tore-replay
```

`summary.txt` is the place to start: each aircraft's statistics, every shot
with its launch geometry and outcome, the radio transcript, a timeline and
the anomaly flags. `log.jsonl` has every event and a state sample per
aircraft at `--rate` per second for tools. `--recording-diff` of two probe
runs names the first tick where they part, which makes it a quick check
that a change left behaviour alone. `TORE_RECORD_MISSIONS=0` stops the
game recording a flight, and `=1` makes it record even during frame timing
runs, so `TORE_PERF_FRAMES` can compare the two.


## Formation flight traces

Normal flight has no formation diagnostics on screen. To record a live rejoin,
set `TORE_FORMATION_TRACE` to a local CSV path before starting the application:

```sh
TORE_FORMATION_TRACE=.local/formation-flight.csv cargo run --locked -p tore-app
```

The parent directory must exist. The app appends a header at each mission
start and samples every 12 simulation ticks (10 Hz), flushing every second.
Rows contain the decision phase, its duration, slot error, closure, altitude error, predicted
minimum separation, yielding actor, steering target, achieved position/speed/
bank/G, and commanded axes plus actual throttle/afterburner. Decision quantities
precede that tick's physics; achieved quantities follow it. A write failure
reports to stderr and disables logging without stopping flight. With the
variable unset, no file is opened. The simulation inspection hook is
`Controller::formation_trace`; it never changes aircraft movement.

For airport visibility and distance precision, run the production-pipeline
checks on a GPU-capable host:

```sh
cargo test --locked -p tore-app gpu_airport_ -- --ignored --nocapture --test-threads=1
```

These check aircraft above pavement and moving distant views with 1x/4x samples.
[Measured failures and results](baselines/airfield-radio.md#ground-visibility).

`cargo test --locked -p tore-sim formation_diving_reversal -- --nocapture`
runs the synthetic four-wingman diving-reversal regression. This is a safety
and physical-input replay check, not a target rejoin-time requirement.

## Surface lighting and shadow validation

The [surface lighting spec](spec/surface-lighting.md) describes the smooth renderer.
`TORE_WEATHER_SMOOTH=0` retains stepped palette lighting without geometric shadows.
On a GPU-capable host, run the synthetic cross-surface shadow and warmth test:

```sh
cargo test --locked -p tore-app gpu_geometry_shadows -- --ignored --nocapture
cargo test --locked -p tore-app gpu_smooth_glare -- --ignored --nocapture
```

The test renders terrain/object receivers and occluders through the production
pipelines, reads pixels back, checks both sun directions and stepped mode,
and checks a warm surface's continuous response, partial-disc shadows and
view-dependent panel highlights, blocked water glints and sharper near-contact
shadows versus distant casters, and a five-second low-sun stability sequence
with small camera movements and sunglare disabled. The glare test checks
every channel of a synthetic gradient under two flare circles against continuous
optical composition. Both tests use no retail media.

## Powered-lift HUD fixtures

`--hud-snapshot PATH --aircraft ah64 --hud-snapshot-state hover` (or `forward`, or `converting`: a V-22 hovering at 140 knots, so its conversion protection is driving the nacelles forward, or `low`: the hover 45 feet over the ground at stability Off, which shows the radar height and the stability label)
draws the HUD of the selected aircraft headless, as a PPM over a flat sky and
ground split at the horizon. A powered-lift aircraft is started in trimmed
forward flight or in a hover, 3,000 feet over a plain, and flown a second.
Layout is in the [HUD layout spec](spec/hud-layout.md#powered-lift-cluster).

## Systems instrument fixtures

`--panel-snapshot PATH --instrument-page 7` renders one 162×160 window, framed and coloured with the aircraft's stored daytime cockpit palette, from the aircraft's actual default
loadout, including external tanks. `--systems-preview 12,13,14` applies explicit
panel-only oil pump, oil leak and hydraulic leak faults. Advance with
`--flight-probe-ticks N` (default 1,200 in this fixture), and use
`--flight-throttle 0..1` for thermal comparisons. The preview requires a panel
snapshot and cannot inject damage into an interactive sortie. Indices 1..35
are listed in the [damage event map](formats/systems-damage.md). Keep captures ignored.

## AI mission and objective checks

`--ai-mission free|cap|intercept|escort|self-defense|hold` sets the inherited
Quick Mission policy. Normal default remains free engagement. Group stamps in
the creator override the preset and remain on mission restart. For example:

```sh
cargo run --locked -p tore-app -- --ai-probe-ticks 1200 --ai-mission escort --no-audio
cargo run --locked -p tore-app -- --launch-quick-mission --ai-mission intercept
cargo run --locked -p tore-app -- --quick-mission --snapshot-state objective-1 --snapshot .local/objective-popup.ppm
```

Objective popup snapshots accept `objective-1` through `objective-6`, friendly
then enemy groups. `--snapshot-state objectives` shows a primary-group/free-fire
example with a required-survival friendly group. These diagnostics do not replace campaign mission loading.

## Ejection inspection

After reimporting into an isolated profile, use `--ejection-preview seat`,
`--ejection-preview freefall` or `--ejection-preview chute` with
`--capture-flight .local/ejection/chute.ppm --no-audio`. These pause a presentation
fixture with the pilot separated from the aircraft so its original art is visible.
Pilot tapes accept `eject` as a one-shot command. Two entries within the documented
confirmation interval exercise the complete headless escape path; replay prints
phase, pilot survival and position. See [validation](baselines/ejection.md).

## Explosion inspection

`TORE_EFFECT_PREVIEW=1` puts every [explosion type](spec/explosions.md) 15 to
38 halfway through its animation in a row 2,500 feet ahead of a free flight,
surface types on the ground, and a burning crash site with its smoke column and
three weapon craters (sizes 3, 9 and 18) on the ground 4,000 feet ahead. It
only changes what is drawn. From the default 5,000-foot start, look down to
see the ground row:

```sh
TORE_DATA_DIR=.local/effects-data TORE_EFFECT_PREVIEW=1 target/debug/tore-app --free-flight --capture-flight .local/effects/row.ppm --flight-view 1 --no-audio
TORE_DATA_DIR=.local/effects-data TORE_EFFECT_PREVIEW=1 target/debug/tore-app --free-flight --capture-flight .local/effects/ground.ppm --flight-view 1 --flight-look 0,40 --flight-zoom 2.5 --no-audio
```

`TORE_EFFECT_PREVIEW=hill` instead starts 2,600 feet short of the nearest even
12 to 20 degree hillside, facing uphill, with six craters (sizes 3, 9 and 18) on
it and nothing else, to check ground marks against sloping terrain
(`--flight-view 1`; try `--flight-look 0,0` and `0,-35`).

A cache made before the explosion sounds and sheets were added re-imports on
first start, as any stale cache does. See [the evidence](baselines/explosions.md).

`--blast-preview OUT_DIR` (first argument) renders the shockwave ring of a
large ground explosion offscreen, without a window: a type 35 blast with a
Mk 84 crater at 0.1, 0.35, 0.7, 1.2 and 1.7 seconds from a strike aircraft's
height, from low beside it and from overhead, by day (and the low view at
dusk), plus a type 21 blast and a type 34 sea blast. `TORE_PREVIEW_THEATER`
picks the theater (default UKR):

```sh
TORE_DATA_DIR=.local/surface-data target/debug/tore-app --blast-preview .local/tmp/blast
```

`--impact-preview OUT_DIR` (first argument) renders weapon impacts side by
side offscreen, each with its own record's land explosion, its
[size by weapon](spec/explosions.md#size-by-weapon) and its
[light and sparks](spec/explosions.md#impact-light-and-sparks): a row of a
20 mm, a 105 mm, a 250 lb, a 500 lb and a 2,000 lb impact 900 ft apart at
0.1, 0.3, 0.6 and 1 second, close views of the 20 mm and 105 mm, and an
eight-round 20 mm burst, by day and at night. `TORE_CITY_FILL=0` keeps the
city fill out of the row:

```sh
TORE_CITY_FILL=0 TORE_DATA_DIR=.local/tmp-me/profile target/debug/tore-app --impact-preview .local/tmp/impacts
```

Add `night-range` after the folder for the [night
glow](spec/explosions.md#night-glow) set (nr-fx3): a 250 lb, a 500 lb and a
2,000 lb bomb 1,200 ft apart with a 24 ft, a 90 ft and a 160 ft fire behind
them, at night, seen from 0.3, 2, 5, 10, 20 and 30 nm (3,000 ft up, 800 ft a
mile farther out) 0.15 s and 1 s after the bombs (`night-RR.Rnm-tS.SSs.png`);
each picture also prints the night haze ramp. The set also writes
`fire-compare-024ft.png`, `fire-compare-090ft.png` and `fire-compare-160ft.png`:
a close fire on the left and a single burning flare on the right at a fixed
camera distance. The battery's `render-night-effect-lights` scenario captures
these in Egypt, plus the range set, for visual inspection. `TORE_NIGHT_GLOW=0`
draws them without the glow points and sustained fire/fireball light, for
comparison.

Add `shockwave` after the folder for the [climate](spec/explosions.md#climate)
set: a 500 lb bomb, a 2,000 lb bomb and an AGM-65G 1,300 ft apart on open
land near the theater's start, seen from low beside the row at 0.1, 0.5, 2,
10, 30 and 60 seconds, by day and at night (`shock-tSSS.Ss-day.png`), with
their shockwave ring, condensation cloud or dust and craters.
`TORE_PREVIEW_THEATER` picks the theater and so its humidity (`TVIET` humid,
`EGY` arid):

```sh
TORE_CITY_FILL=0 TORE_PREVIEW_THEATER=EGY TORE_DATA_DIR=.local/tmp-me/profile target/debug/tore-app --impact-preview .local/tmp/shock-egy shockwave
```

The AI probe's `--probe-render DIR --probe-render-at TICK,...` draws the
world offscreen at 1080p (no window) at those ticks, framing AI plane
`--probe-render-plane ID` from `--probe-render-view chase` (260 ft behind,
140 ft right), `wide` (420 ft behind, 330 ft right) or `ground` (from the
ground beside it, looking up), as `DIR/tTICK-planeID-VIEW.png` (slice H2: the
vertical takeoff and landing renders).

On a player's `--a2g-run`, `--render-scene DIR --scene-at S,...` draws the
world offscreen S seconds after the first rocket, missile or bomb leaves,
chasing the newest one from just behind and beside it (`DIR/scene-tS.png`),
and `--condition night` flies the run at night: the way to look at motor
smoke, the motor flame ending at burnout and impacts in a real run:

```sh
TORE_CITY_FILL=0 TORE_DATA_DIR=.local/tmp-me/profile target/debug/tore-app --a2g-run UKR QUCOL --attack release --from 1 --aircraft a4e --load LAU61 --altitude 1500 --dive 10 --speed 400 --hold 1.5 --render-scene .local/tmp/rockets --scene-at 0.6,1.5,3
```

`--cockpit-sheet OUT_DIR [ID,ID,...]` (first argument) renders every flyable
aircraft's cockpit with live mirrors, its cockpit art with the mirror masks
tinted (centre red, left green, right blue) and two contact sheets, offscreen;
`summary.txt` lists each picture's mirror regions. `--dome-sheet OUT_DIR`
renders the E-3, IL-76 and E-2 from above and behind at six ticks (50 degrees
of dome turn apart). Both read `TORE_DATA_DIR`, like `--blast-preview`
([mirror review](baselines/mirrors.md#2026-10-12-review-of-every-cockpit-picture),
[radar domes](formats/objects-and-shapes.md#airborne-radar-domes-fitted-2026-10-12)).

## Surface unit inspection

### Quick Mission objective placement audit

Creator snapshots honor `--separation`. With no `--ground-target`, an explicit
`--defenses AAA SAM` sets the area defenses; each strength is a number from 0 to 3.
`--surface-seed` fixes the preview's layout. Normal defaults stay unchanged.

Run the complete CPU surface matrix against an isolated imported profile:

```sh
TORE_DATA_DIR=.local/dev-profile python3 tools/objectives_audit.py --output .local/objective-audit --seeds 3 --supplemental
```

Build `tore-app` first with `cargo build --locked -p tore-app`. The runner does
not build or open a window. The application can write its usual logs and city
cache inside the selected profile. The runner retains every command's log,
exit status and a complete `findings.tsv` table. Each table row names the theater,
separation, template, both defense strengths, seed, object, position in feet and
rule. Use a new output directory to preserve a previous run. Add `--theater UKR`
(repeatable) to restrict theaters. `--fast --seeds 1` selects the first target, the first airport/strip target when different,
and no-target area defenses in each theater, with the four matched strengths.
The battery scenario `surface-objectives-audit` uses that subset in Ukraine and
the Falklands. It compares independent ceilings for slope, runway clearance,
object clearance, terrain class and bounds, so an improvement does not fail the
scenario. The 264-case regression gate now requires zero generated-support footprint
slope failures and zero for every other contract category. Zero-error and
reported-error exits are both inspected. Passing this
scenario does not certify authored shoreline or assembly overlaps.

The `menus-snap-quick-target-ground-distances` scenario verifies all eleven distance
choices in a ground-target runway start and launches its 300 nm setup. CPU Quick
Mission snapshots honor `--ground-start`, including when it selects the creator
without an explicit `--quick-mission`. Their `quick-snapshot` line records the
actual start mode and selectable distances. The same regression verifies that
an explicit airport 9 remains Odesa after choosing a target; the target's added
Automatic row must not shift it to the preceding airport (L'viv, airport 8).

The `surface-objectives-cache-kuril` scenario exercises all 4,224 Kuril cases
and independently rebuilds two silo setups where equal raw fronts previously
hid a different final front at another separation. It compares full scene digests,
covering both a newly available front and a moved retained front.

The underlying command is `--surface-dump --objectives [--seeds N] [--fast]
[THEATER ...]`. The full matrix uses every offered non-empty template plus area
defenses without a target, all sixteen AAA/SAM strength pairs, seeds 1 through
N, and all eleven values from the game's separation list. Every case goes through
the real surface resolver and placer, including the front line. Checks use the
completed airport scene and its foundations. Identical object layouts reuse
geometry checks only after comparing their complete placement inputs and
repeating the final front pass against its exact integer obstacles at the current
separation. Starts are recomputed from the repaired target; canonical field ranks
are retained from the built scene because they do not depend on separation.
`--uncached` rebuilds and checks every scene directly. The battery compares both modes' cases, findings and digests.
The first distance repeats the raw placement for every strength pair, and the
zero-defense scene for every template and seed, to check determinism. The table
includes authored positions that older sweeps exempt.

A `placement-refused` row also records every setup the normal launch guard would
refuse for placement, including authored vehicles and retained front groups.
Distance refusals remain separate.

Checks cover map bounds, terrain class at the center and conservative footprint
corners, per-vehicle footprint grades, separate runway and object clearance,
legacy 256 ft cell-corner slope findings, rotated footprint overlap,
route segments sampled every 250 ft, IDs and resolved template counts, missing
layout additions, truck reach, the start/objective distances, final target
centroids, shape-bottom clearance through shared grounding, ship waterlines and
city-fill contact at unit centers. Horizontal
footprint overlap is a review finding, not proof of solid-mesh collision: a road,
bridge, airport assembly or hollow structure can intentionally share that area.
The class query is the placement grid with texture coverage, not a pixel-by-pixel
coastline. Route findings do not certify painted-road alignment or simulate
turning, lanes and movement. Surface Y coordinates are authored offsets above
support, not absolute world heights.

Separation means Blue to Red, within 0.02 nm for integer rounding. Red can be
5 nm from the target, so the objective-distance check allows 5.02 nm. A diagnostic shortened
candidate marks a distance that the creator must not offer; mission launch refuses
that distance instead of using the candidate.
Ground starts override separation. The command reports strict findings separately
from placement-contract errors; it fails on the latter. Thus a zero exit status
is not a claim that every row is clear.

`--supplemental` also runs `--surface-dump --objective-starts` through the full
mission builder with five exact F/A-18D aircraft in each of six wings, every
listed target, distance and seed at 20,000 ft. It checks final aircraft bounds
and terrain clearance, actual separation, CAP regions and intercept/escort
references, and exercises Strike role assignment. Every available runway also
runs the five-aircraft ground-layout checks, with both sides' automatic choices
reported; refusals remain explicit in the log. It then runs the existing grounding-sites, airport,
parked-aircraft and merged-airport audits. These supplemental geometry audits
have their own sampling scopes and logs; they are not the full surface matrix.
The measured results are in the [objective placement baseline](baselines/objectives-audit.md).

### Quick Mission distance table

The creator's Distance list and the dedicated server read a fixed table of the
enemy distances each theater, ground target and start offers
(`crates/tore-world/src/mission_layout/distance_table_data.rs`, read through
`distance_table.rs`). It is generated, never edited by hand, with the launch
solver (`mission_layout::availability`) against an imported profile:

```sh
cargo build --locked -p tore-app
TORE_DATA_DIR=<imported profile> TORE_WORKERS=0 nice -n 19 target/debug/tore-app \
  --qmc-distance-table --jobs 4 --write crates/tore-world/src/mission_layout/distance_table_data.rs
```

The run is headless and takes about six minutes with six jobs on the
development machine (each target's 64 layouts dominate). It prints one line per
theater (the airborne row, each situation's, the ground row and every offered
airfield's) and per target (its row and the largest distance each seed
reached), marks a row with a hole in it `GAP`, and lists the layout
limitations with the seeds that had them. A limitation every seed has becomes
the target's note, which the creator shows while the target is chosen.
`--write` needs all sixteen theaters. Options: `--jobs N` (1 to 8 worker
threads, default 4), `--seeds N` (default 64, the table's), theater codes to
measure only those, `--check` to fail when the checked-in table differs from
the measurement, and `--measure` to time one creator setting change before the
table (the placement audit) and after (a lookup). The ignored test
`qmc_distance_table::tests::the_checked_in_table_matches_a_fresh_measurement`
checks the Kurile and Ukraine rows:
`TORE_DATA_DIR=<profile> cargo test -p tore-app qmc_distance -- --ignored`.

Regenerate the table after any change to the start geometry, the map margin,
the separation list, the creator's airfields or the surface layout of a target,
and commit the data file with the change. The measured table and its rules are
in the [distance table baseline](baselines/qmc-distance-table.md).
No aircraft or surface AI is stepped by the new audit. Retail gameplay comparison
is unavailable, and these checks do not establish retail parity.


### Surface dumps and captures

`--surface-dump` prints a theater's surface units as a mission resolves and
places them ([surface defenses](spec/surface-defenses.md)): each unit's id,
type, side, position, destroyed look and whether the scene can draw it (added
trucks and radars marked `added`), the removed defense slots, the template's
anchor and group move, every SAM battery (its radar, adopted or added, its
launchers and truck), every supply truck with the unit it serves and its gap,
the starts (target point, Red's and Blue's starts, Blue's heading, the ranked airfields), what
the layout could not add, and the surface digest. Name a ground target
template to add its units; defenses default to heavy and the enemy to the
theater's own and the enemy distance to 20 nm (`--separation N`). `--no-jitter` and `--no-relocate` show the retail spot.
`--all` resolves every offered template at every defense level, which the
`surface-resolve-all` battery scenario checks against the retail survey.
`--sweep [--seeds N] [THEATER ...]` places every offered template with seeds
1 to N (20) and reports each base layout's batteries and every placement's
anchor, move, broken site rules and digest (`surface-relocate-sweep`).
`--classes [--seeds N] [THEATER ...]` checks the terrain class rule over
every base layout and every template placement and lists each moved unit on
the wrong class (`surface-class-sweep`).
`--starts [THEATER ...]` builds one mission per theater with a ground target
and reports where Blue and Red start (`surface-start-placement`).
`--grounding [TYPE ...]` is the grounding audit: every OT, NT and PT (or the
types named) stood on flat ground and drawn through the scenery's own code,
with its shape's lowest and highest point about its origin, the share below
it, its F2 ground offset, the lift that stands it on the ground, and the
lowest drawn point of its faces, its hit box and its man sprites or cards near and
far, in world feet at its drawn size; anything sunk or floating is listed as
a problem ([grounding](formats/objects-and-shapes.md#grounding-every-object-stands-on-the-bottom-of-its-shape),
`surface-grounding`).
`--ground-pads [THEATER ...]` builds the actual theater layouts and objectives,
authored and relocated, and checks flat shapes and broad fixed footprints against the finished
terrain. It prints old burial and current support error; `surface-ground-pads`
is the matching battery scenario.
`--damage [TYPE ...]` lists every OT (or the types named) with where retail
keeps its destroyed look: its `~` variant, the damage branch inside its shape
(intact and damaged face counts and heights) or neither
([destroyed looks of buildings](formats/objects-and-shapes.md#destroyed-looks-of-buildings-and-bridges-2026-10-12)).
`--faces TYPE [...]` lists every face of each named type's main shape, intact
and damaged: its texture (and whether the pack has it), colour slots, record
kind and extent, which is how a missing texture or a stray palette colour
shows (the bridge's magenta patch was a colour 6 shadow quad, [ground shadow
decals](formats/objects-and-shapes.md#ground-shadow-decals-2026-10-13)).
`--surface-sheets` renders each template from above and closer, heavy
defenses, at its retail spot (seed 1, no jitter or relocation); `--variants`
adds the placed layout with seeds 1 and 2. A marker per unit: red Redfor, blue
Blue, a white ring for a target, yellow parked aircraft, green supply trucks,
cyan battery radars, orange battery launchers, magenta a unit whose shape does
not read yet. No window opens.

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-dump TVIET QTSAM --defenses 2 3 --surface-seed 7
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-dump UKR QUNUKE --no-relocate
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-dump --all
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-dump --sweep --seeds 5 CUB TVIET
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-dump --starts
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-sheets .local/surface-sheets --variants UKR:QUCITY TVIET
```

The import does not keep the templates or the unit types only they name yet,
so both commands read what the pack lacks from the retail media at runtime:
`TORE_GAME_DIR`, else the source the data directory remembers, else the
checkout's `gameassets/fighters-anthology` link.

`--surface-battle --survey [THEATER ...] [--seeds N]` builds every base layout
and offered ground target at heavy defenses (seed 1, or seeds 1 to N) and lists
every pair of opposing units within a weapon's reach of each other, with the
nearest pair out of reach; `--surface-battle THEATER [STEM] [--blue
ORDINAL,...] [--no-front] [--seconds S] [--twice]` runs the mission with the
player parked and prints what surface units do to each other: engagements of
ground units and ships, shots, hits with the unit credited, kills, columns
halting and driving on, a summary and an end-state digest. `--blue` hands
template objects to Blue first (a test mission: retail never stands opposing
units in reach), and `--twice` reruns it and compares the digests
([surface against surface](spec/surface-defenses.md#surface-against-surface)).
A mission with a ground target carries its [front
line](spec/surface-defenses.md#the-front-line) as in the game; `--no-front`
leaves it out (the survey always does, since it measures retail's layout).
`--surface-battle --front-survey [THEATER ...] [--seeds N]` builds each offered
target with the front line (seeds 1 to N, 2 by default) and lists its units,
the pairs in sight and reach, and its distance from the targets. `--surface-objective` and
`--surface-dump` take `--no-front` too. `--surface-trace` and the weapon, radar
and drive probes (`--a2g-run`, `--a2g-radar`, `--surface-parked`,
`--surface-drive` and the previews) build the target without the front line,
since they measure one unit or one weapon (`--surface-trace --front` adds it).

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-battle --survey --seeds 20
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-battle UKR QUCOL --blue 0,1,2,3 --seconds 120 --twice
```

`--surface-trace THEATER [STEM]` flies the player on a scripted straight line
past one armed unit (`--over TYPE [--index N]` or `--unit ID`) at
`--altitude` feet above it, `--speed` knots, `--pass` feet to its side,
starting `--from` nm short, for `--seconds`, and prints what the surface
controllers do: phase changes, shots and gun bursts with their rate, magazine
swaps, radar on and off, HARM shutdown rolls and battery changes, the RWR's
ground squares, locks and tone, decoy rolls, hits, the unit's stock and a
summary line. `--chaff S` and `--flares S` dispense while a missile is in
flight at the jet, `--harm-at NM` puts an AGM-88 in flight at the unit's
radar, `--kill-at S` destroys that radar, `--player-side red` flies for
Redfor, `--condition night` darkens the sky, `--skill N` and `--rng N` fix the
experience and the random draws, and `--near FT` follows the unit's
neighbours too. For resupply, `--drain` empties the followed units' rails and
magazines at the start, `--drain-reserve` their spare magazines too, and
`--kill-truck-at S` destroys the supply trucks within 0.1 mile of the unit; the
trace prints `rearm` and `refill` lines and the final stock of every followed
unit. `--no-relocate` traces the template at its retail spot. A launcher in a SAM battery is traced with its battery's radar
and launchers. It reads the same retail records, weapons and sensors too.
The `surface-*` engagement battery scenarios run it.

`--surface-dump --grounding-sites` extends the flat type audit to actual
theater and mission placements and every offered Quick Mission template
plus the three unused Ukraine templates, both authored and seed 1 relocated, at heavy defenses. It reports per-theater
coverage, missing geometry, footing exceptions and before/after clearance.
`surface-grounding-sites` runs it in the battery. The separate
`surface-grounding-radar-render` scenario captures a radar objective offscreen.

`--bridge-audit OUT_DIR [LAYOUT ...] [--strict] [--capture]` reads only the
imported pack and writes `assets.tsv`, `placements.tsv` and `banks.tsv`.
The bank table measures excavation and constructed approach grades; strict
audits reject cuts over 1 ft, grades over 8.1 percent (rounding tolerance)
and END road toes more than 30 ft above terrain. By default it
checks all 75 layouts plus every offered template that names bridge pieces
(heavy defenses, seed 1, ordinary placement). Named layouts limit the run.
The tables record every bridge's deck/ramp samples, endpoint clearances,
physical feet of the connected assembly versus ground, sea-plane clearance, source orientation and
nearby object overlap candidates. `--strict` fails for buried decks, unknown
decks, support gaps over 30 ft or missing assets. Sampling is at most 64 ft;
road/river artwork alignment and candidate overlaps still need visual review.

`--capture` renders every bridge intact and destroyed with the existing
scene renderer, plus half-health appearance for the first of each type.
Optional `--view=side`, `--view=along` or `--view=near` selects a bridge
inspection angle; `--id=40000082,40000092` limits captures to those IDs
without narrowing the numeric audit.
It checks that bridge geometry remains present and that live HP and replay
HP produce identical geometry, positive HP retains intact art, and zero HP
changes the drawn geometry. `bridge-end-ruins` covers all four END types in
the battery. This uses the GPU and must run through
`tools/agent-run.sh`. Keep the output outside tracked directories. Example:

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --bridge-audit .local/bridges --strict
TORE_DATA_DIR=.local/dev-profile tools/agent-run.sh target/debug/tore-app --bridge-audit .local/bridge-images UKR --capture --strict
```

The battery's `bridge-audit` checks Ukraine and the Baltics, including the
sloping crossings and bridge objectives. `a2g-bridge-destruction` fires a
Maverick broadside at the Ukraine span and verifies its recorded damage and
effect snapshots. [Behavior and limitations](spec/bridges.md).

`--surface-scene OUT_DIR THEATER STEM` renders a ground target as the game
draws it, offscreen at 1080p (no window): it steps the whole world
`--seconds S` (`--surface-only` steps just the moving units, for long
marches), optionally destroys units (`--kill all`, or a comma list of
template ordinals, unit types such as `SA3.NT`, `within:FEET` of the
framed units and `runways`, every runway object within the framed distance,
the layout's too) and lets them burn for `--burn S` seconds, sets every
launcher's rails to at most `--rails N` rounds, then writes one PNG per view
(`--views oblique,close,low,top,deck`, or `--look YAW,PITCH` in degrees)
framing `--focus` (`routed`, `parked`, ordinals or types) within
`--distance FT`. It draws the static scene with its wrecks and rails, the
moving units, men and deck crew, explosions, fires and smoke. Units aim as
in the game, stepped every tick; `--no-aim` draws them unturned.
`--intruder EAST,UP,NORTH` holds the player's aircraft (invulnerable)
circling that far from the focus so the defenses lock on and fire, `--blue
ORDINALS` hands template objects to Blue so units fight each other, and each
focused unit prints its aim (`aim ... body ... turrets ... holds ...`).
The scene's other static objects are named `object:TYPE` (`object:HANGR.OT`)
in `--focus` and `--kill` (and `within:FEET` takes them), `near:ITEM` frames
only the one ITEM nearest the template, `--flatten FEET` knocks the city fill's buildings within FEET of the focus flat so they burn and `--frames N` times N more frames of each view (nr-fx2), `--bomb-runway N` drops N Mk 82s
along the runway nearest the focus, `--until-flash` steps on after `--seconds` (up to five
minutes) until a gun within 300 ft of the focus has just fired so the picture has its muzzle
flash (nr-flak: `--focus 0 --distance 45 --until-flash --intruder 3000,6000,2000 --look 90,-8`
shows where a KS-12's flash starts), and `--time HH:MM` sets the start time
(night views); with no focus left the run lists every place by type (B16).
`--surface-preview
OUT_DIR destroyed` draws one unit of each class beside its destroyed look,
read from the records, and the four carriers with their damaged islands.

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes UKR QUCOL --seconds 1300 --surface-only --focus routed --distance 110 --look 300,-35
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes PGU QPGSAM --focus 8 --distance 14 --look 30,-25 --rails 1
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes FRA QFFLT --focus CLEM.NT --distance 420 --kill CLEM.NT --burn 30
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes UKR QUCOL --seconds 30 --focus 10 --views close --distance 40 --intruder 5000,3000,2000
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes UKR QUSTRIP --kill within:600 --focus near:object:BRDMID.OT --distance 700 --time 23:00
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-scene .local/scenes UKR QUSTRIP --bomb-runway 8 --focus 0 --distance 1200
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-preview .local/destroyed destroyed
```

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-trace IRA --over SA6 --altitude 15000 --from 20 --seconds 150
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-trace TVIET QTAAA --over KS19 --aircraft a10 --altitude 15000 --speed 300 --from 10
```

`--hud-a2g-preview OUT_DIR` draws the HUD's bomb and rocket sights
([air-to-ground HUD](spec/air-to-ground.md#the-hud-in-air-to-ground-modes))
without a window: for each frame it builds the TVIET truck park's Quick
Mission, places the aircraft, steps the world so combat solves the sight, and
draws the flight HUD and the weapon HUD over a plain sky and ground with the
target marked. It writes each frame twice its size (`NAME.png`: CCIP beyond,
near and inside the range, off the HUD, LOW, FRAG and IMP on an F/A-18D, the
dot trail on an F-16C, the level "T" far out, at 10 s, 2 s and the release,
off track and OVERFLY on an F-4B, the rocket pipper on an A-4E) and contact
sheets `sheet-N.png`. `--a2g-run` flies the same sights through the trigger
(`--attack ccip`, `level`, `rocket`; [flight lane](testing/lane-flight.md#air-to-ground-weapons)).

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --hud-a2g-preview .local/hud-a2g
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --a2g-run UKR QUCOL --aircraft f4b --load MK82 --attack level --altitude 5000 --speed 450 --from 2
```

`--hud-guided-preview OUT_DIR` draws the guided weapons' release range scale
([guided bomb release range indicator](spec/air-to-ground.md#guided-bomb-release-range-indicator))
the same way: an F-16C's GBU-30 far out, at the far edge of its reach, in the
middle, inside the near edge and with nothing designated, its GBU-10 lasing
and with the laser off, and an Su-25's AS-14 (`NAME.png` and
`guided-sheet-N.png`). `--a2g-run` flies the guided weapons with `--attack
laser`, `jdam` and `at2`, which designate through the A-G page (T, else Easy
targeting's Ctrl+X), release on the HUD's IN RNG cue and follow each weapon
to its end; `--laser-off S` turns the laser off S seconds after the release,
`--buddy ID` puts a second aircraft on a second seat to lase for the
attacker, `--turn-away S` turns the attacker away after the release and
`--condition clear|cloudy|foggy|night` sets the weather. Guided attacks accept
`--release cue` (the default, a quarter of the way into the reach),
`--release bar` (hold for the level T) and `--release max` (98% of the range
interval, for a near-limit check). The `flight-measured` line reports release
TTI, elapsed flight time, their difference in ticks, and arrival speed in knots.
The `a2g-bomb-flight-*` battery cases record the four shared-model acceptance
passes and verify replay transforms on the CPU.

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --hud-guided-preview .local/hud-guided
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --a2g-run UKR QUCOL --aircraft f16c --load GBU10 --station 2 --count 2 --attack laser --altitude 15000 --speed 480 --from 6 --seconds 120
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --a2g-run UKR QUCOL --aircraft f18 --load GBU10 --station 2 --count 2 --attack laser --altitude 12000 --from 6 --seconds 120 --buddy su25
```

`--surface-fx-preview OUT_DIR [SCENE ...]` renders what the surface defenses
look like, offscreen, by day, at dusk and at night: `flak` (the KS-12 and
KS-19 bursts over North Vietnam, wide and close to the jet), `zsu23` and
`zsu57` (muzzle flash, firing light and tracers beside a gun), `troops` (an
airfield squad's soldiers and tracers) and `squadkill` (the same squad killed
as it opens fire), `sam` (an SA-6
leaving its rail, at the pad and up its path), `wreck` (a destroyed ZSU-23
smoking) and `map` (the flight map over the same target). Each scene is the
real game: it builds the mission, flies the player past the unit, steps the
whole world and draws the frame the first flak burst, round, launch or kill
makes through the same picture, tracker and renderer the game uses. The names
of the frames say the scene, the light, the view and the ticks after the
trigger. `--ground-target STEM [--defenses AAA SAM] [--surface-seed N]` sets
the creator's ground target for a launched flight or capture
(`--launch-quick-mission`, `--smoke-test`).

```sh
TORE_DATA_DIR=.local/dev-profile tools/agent-run.sh target/debug/tore-app --surface-fx-preview .local/tmp/fx flak sam
```

`--surface-objective THEATER STEM` checks a ground target's objectives and the
debrief in the whole world. The player flies a scripted pass at the target's
defenses (`--from` nm out, `--altitude` feet above the ground, 400 knots,
invulnerable, so its SAMs and guns fire), then a Mk 82 is placed on every
target every two seconds until it is down (`--kill-friendly` also bombs a
friendly unit that is not a target). It prints the target list, the
objectives, the outcome and the debrief's tallies at the start and at the end:
SAM and AAA hits and launches, the kill rows, friendly fire and who shot the
player down. `--redfor` builds the multiplayer mission and seats the human in
the first enemy plane, so the targets are a Protect objective and a Blue plane
drops the bombs; `--defenses AAA SAM` and `--surface-seed N` as above. The
`surface-objective-*` battery scenarios run it. An `end runway` line gives each
runway within 20,000 ft of a target with its hit points and whether it is
redrawn (`redrawn PLAN`) or `retail`; `--runway-art` also builds the scenery on
the CPU and adds the vertex floats the runway draws now (`drawn N`), and
`--retail-airfields` builds the retail airfield shapes instead of the redraw.
The `surface-runway-stays` scenario uses them (B15: bombs on `~QUSTRIP`'s
buildings bring its 750 hit point runway down, and it stays drawn).

`--record PATH` records the run as a mission recording (a format 3 file, see
[Mission replays](REPLAYS.md#versions-and-damage)); `--verify-render` then reads
it back and compares every tick with the picture the run drew, printing
`surface-objective: verify-render: PASS ticks=... missing=0 differing=0` or the
first difference. `--run-on` keeps the run going to `--seconds` after every
target is down, so supply trucks rearm the emptied launchers, and
`--vulnerable` takes the player's invulnerability away, so the defenses can
shoot it down. The `replay-surface` scenario uses all four.

`--city-strike N` (nr-fx3) also drops N Mk 84s, four a pass from the first
pass, on the standing city fill buildings nearest the target (at least 600 ft
apart), printing each aim point and, at the end, `surface-objective: city
flattened N buildings`. With `--record` the recording keeps them in its city
section. The `replay-city-strike` scenario records one and draws it back:

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-objective UKR QUCOL --surface-seed 1 --from 2 --run-on --seconds 40 --city-strike 12 --record .local/tmp/city.tore-replay --verify-render
```

`--replay-stills REPLAY FRAME_PLAN OUT_DIR` (first argument) draws a
recording's frames offscreen through the replay viewer, as PNG files
(`still-NN-tTICK.png`), with no window, no audio written and no ffmpeg. The
plan is the reel director's (`--reel-render`, one frame a line: tick,
anchor, eye, target, field of view, visual time in minutes of the day (a fraction steps the clock on to the second) or -1, view,
rate, fixed-eye tick or -1); anchor 4294967294 puts the eye and target in
world feet. A plan's rows may jump backwards, which is how the stills check a
seek. When the city fill has burning buildings a frame also prints a `Replay
city smoke: tick T burning N fires F puffs P tallest H ft digest D` line: what
the burning city drew in numbers (the digest covers every puff), so a seek and
a play-through can be compared as text (nr-fx4, scenario
`replay-city-smoke-seek`). `TORE_REPLAY_CITY_CLOCK=0` ages the columns on the
wall clock from the moment each is first seen, as before nr-fx4 (a developer
aid for before pictures).

The per-theater acceptance run ([baseline](baselines/surface-defenses.md)) adds
`--shuttle` (fly the line back and forth across the site for the whole run, so
the defenses keep firing and the trucks rearm them), `--follow-terrain` (hold
`--altitude` above the ground under the aircraft, so a line over hills does not
fly into them) and `--no-relocate` (`--no-jitter`) for the retail spot. The run
then ends with a `surface-objective: layout` line (the template's units, SAM,
AAA and ship counts, batteries, supply trucks, parked aircraft, anchor and how
far it moved) and a `surface-objective: summary` line (missiles, gun rounds and
flak shells fired by the template's units and by the base layout's, refused
rounds, magazine swaps, rearms, refills, kills, the first shot, the first
missile, the last target's fall, a crash, and rounds by weapon record).

```sh
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-objective UKR QUCOL
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-objective UKR QUCOL --redfor
TORE_DATA_DIR=.local/dev-profile target/debug/tore-app --surface-objective TVIET QTAAA --shuttle --run-on --follow-terrain --seconds 900 --no-relocate
```

## Flight view inspection

`--flight-view 0..11` preserves 0 front, 1 external, 2 oblique, 3 back and 4 up.
The additions are 5 tracking, 6 inbound threat, 7 wingman, 8 player-to-target,
9 target-to-player, 10 fly-by and 11 missile-to-target. These are capture indices,
not the function-key numbers. `--flight-reference player|target|missile` selects
the reference for capture. Missing subjects return to Forward with feedback.
Use an isolated profile with `--capture-flight .local/retail-views/view.ppm`.
The [view validation](baselines/flight-views.md) records fixtures and limitations.

## Retail map detail validation

The Quick Mission location picker offers the sixteen base maps only; the 59
imported `~` source variants are incomplete (mostly one or two airports) and are
not player choices since 2026-09-29. They remain a developer option for probes
and the battery: `--theater '~UKR1'` selects an exact variant for the viewer,
free flight, Quick Mission (where that one layout is added to the picker for the
run) or headless flight. Shell quotes preserve the tilde. Existing flight
adapters remain independent of map selection.

```sh
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --validate-maps --no-audio
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --theater KURILE --viewer --smoke-test
TORE_DATA_DIR=.local/dev-profile cargo run --locked -p tore-app -- --theater '~UKR1' --viewer --capture-terrain .local/ukr1.ppm
```

`--airport-allegiance` (imported media, no display) lists, for every base theater and for Blue and for Redfor, the ground-start airports the creator offers and checks them, the automatic ground start and the AI's home runway against the built world: none may be the other side's ([allegiance](spec/airports.md#allegiance)). `--validate-ils` (imported media, no display) measures the ILS at every airport of every base theater, and of the `--theater ~CODE` variant if one is named: the datum against the runway plane, the glide path crossing the threshold, the bars reading zero with the right signs down the ideal path, and any terrain above the final 5 nm of it. See [ILS alignment](testing/ils.md). `--validate-maps` needs imported media but no display. It constructs every
imported layout, reports source identity, placement/body counts, geometry and
indexed artwork size (without the flight vegetation, whose pages
`--vegetation-preview DIR limits-check` checks through the mission loader), and
exits with an error on construction failure. The
other two commands need a display. Older caches require re-import for the
`TORE_TERRAIN_V2` dependency set. Use the existing isolated import workflow
above; never write extracted retail resources into tracked assets.

Variants are static source scenery lists, with the
[documented fitted grid/composition rule](spec/terrain-detail.md#host-presentation-where-the-source-rule-is-incomplete).
They do not start a campaign or add autonomous ground behavior.


## Ground colour regression

`--terrain-colors` (first argument) checks all sixteen theaters and six weather
choices at 0, 6,000 and 30,000 feet without a window or GPU. It prints the mean
land-cell RGB and compares every terrain palette entry with the weather family
independently read from the retail base MM. A mismatch fails the command.
The battery scenario is `render-terrain-colors`.

`--terrain-colors OUT_DIR THEATER` also writes four offscreen 1080p PNGs:
`THEATER-day-near.png`, `THEATER-day-far.png`, `THEATER-night-near.png` and
`THEATER-night-far.png`. The cameras sit 1,800 and 18,000 feet above the central
land cell and look down 0.65 radians. City fill and scattered clouds are off
for this repeatable terrain fixture. Use a private profile copy and never
write over an earlier capture directory. This is a developer fixture, not a
retail screenshot comparison.

Add `--cpu` after THEATER for an unlit orthographic software render of the
actual terrain mesh and its indexed textures. The near/far views span 32,768
and 262,144 feet vertically, sampled through the live palette at the same
camera altitudes. This mode needs no GPU. It omits objects, lighting, fog and
GPU distance filtering, so it verifies artwork and palette colours only.

### CAP and strike order regression counters

AI probes with CAP assignments print `AI probe CAP` per aircraft: its first
target tick, launch-request count and greatest distance from station in NM.
The AI strike driver accepts `--quick-wing --clear-air-at SECONDS` to remove
the last bandit during an explicitly ordered ground strike. See the
[CAP and strike checks](testing/lane-ai.md#cap-orders-and-unfinished-strikes).

## CPU paired-airfield comparison

`TORE_DATA_DIR=<isolated profile> tore-app --airfield-sheets <output directory> --cpu-merged`
produces eight windowless diagnostic PNGs: Southampton and Spassk Dalniy,
before and after merging, overhead and from an orthographic approach angle.
It reads the real terrain and static scenery buffers and indexed texture
pages. It omits GPU lighting, city-fill instances, weather and moving objects.
Use it when the graphics backend cannot create an offscreen renderer; it
checks layout and artwork, not full GPU presentation.

The headless AI probe accepts up to 288,000 ticks (40 simulation minutes).
This allows four-ship landing and parking checks on the 12,000 ft merged
fields to finish their long taxi returns; normal flight duration is unchanged.

### QMC wing loadout probe

`TORE_DATA_DIR=<private-profile> target/debug/tore-app --validate-wing-loadout <output-directory> --no-audio`
builds a QMC four-ship F-16C flight with Mk 82s, AIM-9s, no tanks and 60 percent
internal fuel. It writes mission text and tick-zero recordings under the output
directory, first with the previous default-wing policy and then with sharing.
It asserts the accepted load survives airborne and ground starts and restart.
The battery scenario is `qmc-wing-loadout`. This probe requires imported media
but no display; do not point it at a shared profile.

### Missile motor calibration and prediction cost

The offline `missile_energy_ranges LOCAL_JT_DIRECTORY --fit-pair` example
prints candidate fuel fractions, drag and both anchor residuals; it never
writes runtime data. An optional comma-separated source list limits the fit.
The default sweep prints physical and seeker-valid ranges including the
500-foot deck case. Conditions and data provenance live in the
[missile energy data](formats/missile-energy-data.md) and
[baseline](baselines/missile-energy.md).

The release `missile_energy_cost PACK TICKS THEATER moving WEAPON STATIONS`
example accepts `--natural` for real AI launch clients, `--big` for 23+ AI,
and `--spec` for the historical missile comparison. Without `--natural` it
uses Hold, eight AI and sixteen replenished synthetic missiles; jettisoned
tanks do not count toward those sixteen. The alive guard applies to that
injected fixture, while natural fights retain real damage and casualties.
