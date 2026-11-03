# Self-hosted CI runners

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

GitHub's hosted runners took one to two and a half hours per platform for the
Rust baseline and release packaging (2026-10-23), mostly cold compiles on four
cores. Since then the project's own machines can run those jobs instead, with
their build folders kept between runs so later builds are incremental
(John, 2026-10-23).

| Workflow platform | Runner label | Where | Runners |
| --- | --- | --- | --- |
| `ubuntu-22.04` | `tore-ubuntu-22.04` | Docker containers from Ubuntu 22.04 on the Linux desktop | `tore-linux-1` to `-3` |
| `windows-2022` | `tore-windows-2022` | A Windows 11 VM under KVM on the Linux desktop | `tore-windows-1`, `-2` |
| `macos-14` | `tore-macos-14` | John's Apple Silicon MacBook, natively | `tore-mac-arm64` |
| `macos-15-intel` | `tore-macos-15-intel` | The same MacBook, under Rosetta | `tore-mac-x86_64` |

## The switch

`TORE_SELF_HOSTED`, a repository variable, lists the platforms that use these
runners, by their workflow name, separated by commas:

```sh
gh variable set TORE_SELF_HOSTED --body "ubuntu-22.04,windows-2022,macos-14,macos-15-intel"
gh variable set TORE_SELF_HOSTED --body "ubuntu-22.04"   # Linux only
gh variable delete TORE_SELF_HOSTED                      # everything on GitHub again
```

Each job's `runs-on` in `ci.yml`, `network.yml` and `release.yml` reads it:
a listed platform runs on `[self-hosted, tore-<platform>]`, any other on the
hosted image of that name. Take a platform out when its machine is off or busy,
for example the MacBook while it travels; a job for an absent self-hosted
runner waits in the queue rather than failing. The `release` job that publishes
a tag's packages always runs on GitHub.

On a self-hosted runner the checkout keeps cargo's `target/` folder
(`clean: false`, then `git clean -ffdx -e /target/`), so every other untracked
file, `dist/` included, is gone before a job starts, as on a hosted runner.

## Safety

A self-hosted runner runs whatever a workflow asks of it, so these run only
for the project's own triggers: `workflow_dispatch`, `v*` tags and pushes to
`release-test/...`. No workflow runs on pull requests, so an outside
contributor cannot start a job on these machines. Keep it that way: before
adding a `pull_request` trigger, take the platforms out of `TORE_SELF_HOSTED`.
The Linux runners are containers without the Docker socket or the host's home
folder; the Windows runner is its own VM, apart from Omarchy's Windows VM.

## Linux runners

`tools/ci-runners/linux/` holds the image (Ubuntu 22.04, the release's glibc
2.35 floor, with the windowing libraries, Python and rustup) and the compose
file for three runners. They run from `~/tore-ci/linux`:

```sh
cp tools/ci-runners/linux/{Dockerfile,entrypoint.sh,compose.yml} ~/tore-ci/linux/
cd ~/tore-ci/linux
RUNNER_TOKEN=$(gh api -X POST repos/john-overton/T.O.R.E-Fighters/actions/runners/registration-token --jq .token) \
  docker compose up -d --build
```

The token is only needed the first time: each runner keeps its registration,
work folder, cargo home and rustup in `runners/<n>/`. The pinned toolchain
installs itself on the first job. The runners run at `nice 10` with a low CPU
share and a 20 GB memory cap each, so a game on the desktop keeps priority.
`docker compose stop` pauses them; queued jobs wait.

## Windows runner

`tools/ci-runners/windows/` holds the VM's compose file (dockurr/windows,
Windows 11, 12 cores, 24 GB, a 256 GB sparse disk), its first-logon step
(`oem/`: OpenSSH with the CI key) and `setup.ps1`, which installs Git for
Windows, PowerShell 7, Visual Studio 2022 Build Tools with the C++ workload,
WiX 3.14 and rustup with the pinned toolchain for both Windows targets, then
registers two runner services. From `~/tore-ci/windows`, which also holds
`.env` (the VM account's password), `ssh/` (the key) and `storage/` (the disk;
copy-on-write is off for it on btrfs):

```sh
docker compose up -d        # first start installs Windows unattended (about 15 minutes)
scp -i ssh/id_ed25519 -P 2222 setup.ps1 runner@127.0.0.1:C:/OEM/setup.ps1
ssh -i ssh/id_ed25519 -p 2222 runner@127.0.0.1 \
  "powershell -ExecutionPolicy Bypass -File C:\OEM\setup.ps1 -Token <token> -Password <password>"
```

The console is at http://127.0.0.1:8016/ while it installs, RDP at
127.0.0.1:3399. Cargo's folders in `C:\ci` are excluded from Defender.
`docker compose stop` shuts the VM down cleanly.

## macOS runners

`tools/ci-runners/macos/setup.sh` installs one runner per call on an Apple
Silicon Mac, as a LaunchAgent that starts at login:

```sh
tools/ci-runners/macos/setup.sh arm64  <token>   # macos-14 jobs
tools/ci-runners/macos/setup.sh x86_64 <token>   # macos-15-intel jobs, under Rosetta
```

The x86_64 runner is the runner's x64 build, so every step it starts runs as
x86_64: `uname -m` says x86_64, the DMG is named for it and its rustup is
the x86_64 one, with no change to the workflows. Each runner keeps its own
rustup, cargo home and work folder in `~/tore-ci/<arch>`, apart from the
user's own `~/.cargo`, and its own standalone Python 3.14 for its
architecture (python-build-standalone) in `~/tore-ci/<arch>/bin`, first on
its PATH: the workflows skip `actions/setup-python` on a self-hosted Mac,
because its macOS builds install a system package with `sudo` on every run.
The Mac must be awake for its jobs (`wake-macbook`
from the Linux desktop wakes it over the network).

To leave the Mac alone while someone is working on it, start a run with
`-f skip_macos=true`, for example `gh workflow run ci.yml --ref BRANCH -f skip_macos=true`.
`ci.yml`, `network.yml` and `release.yml` all take it. A tag push always includes
the Macs. A dispatch on a tag can skip them, but a run that skips them never publishes
a release.

## Upkeep

- The runners update themselves. Rebuilding the Linux image or rerunning a
  setup script keeps the registrations.
- A full disk: `cargo clean` in a runner's work checkout, or delete its
  `target/` folder; the next job rebuilds it.
- Removing a runner: GitHub, Settings, Actions, Runners, or
  `./config.sh remove --token <removal token>` in its folder, then stop the
  container, service or VM.
