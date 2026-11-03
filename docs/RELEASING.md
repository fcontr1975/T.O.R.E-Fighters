# Releasing a new version

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Use this checklist for every release so all version numbers stay in step. The
examples go from `0.1.0` to `0.1.1`; substitute your own numbers.

## 1. Change the version

| File | What to change |
| --- | --- |
| `crates/*/Cargo.toml` (every crate, nine since `tore-world` joined) | `version = "0.1.1"` on line 3. `tore-app` is the one the release checks; the rest are kept equal. |
| `Cargo.lock` | Refreshed by the build in step 2; never edit it by hand. |
| `README.md` | The milestone badge's alt text near the top, and "Version 0.1.0 completes..." under **What works today**, with that section's paragraphs (such as **Replays**) if the release changes what they say. |
| `docs/DEVELOPMENT.md` | The `TORE_BUILD_VERSION=0.1.0` example under **Packaging**. |

One command covers every crate. On macOS:

```sh
sed -i '' 's/^version = "0.1.0"/version = "0.1.1"/' crates/*/Cargo.toml
```

On Linux, drop the `''` after `-i`.

## 2. Write the release notes

Add `docs/release/v0.1.1.md` and list it at the top of
`docs/release/releases.json`, following the
[release notes guide](release/README.md). The notes appear on the website's
[releases page](https://john-overton.github.io/T.O.R.E-Fighters/releases.html),
and the release workflow publishes the same file as the GitHub release title
and description.

## 3. Refresh the lock file and check

```sh
cargo build --workspace
git grep -n "0\.1\.0" -- ':!Cargo.lock' ':!docs/research' ':!docs/baselines'
```

The build, run once without `--locked`, updates `Cargo.lock`. The search
should list nothing that still needs changing. These stay as they are:

- the examples in the comments in `crates/tore-app/src/version.rs` and
  `crates/tore-replay/src/model.rs`;
- the synthetic recordings in the `tore-replay` tests (`src/format.rs` and
  `tests/`) and their golden summary, log and Tacview outputs in
  `crates/tore-replay/tests/golden/`, which use a fixed version so the
  goldens do not change with each release;
- old baselines and research archives, which record past versions.

[Mission recordings](REPLAYS.md) need nothing: each stores the version of the
build that made it, and recordings from earlier versions stay watchable,
because a reader checks the recording's own format number, not the game
version. That number (`FORMAT_VERSION` in `crates/tore-replay/src/lib.rs`)
rises only for a format change older builds could not read past, never for a
release. Then run the
[everyday checks](DEVELOPMENT.md#everyday-checks).

## 4. Commit, tag and push

```sh
git commit -am "Release 0.1.1"
git push
git tag v0.1.1
git push origin v0.1.1
```

Pushing the tag starts the release workflow, which builds and publishes the
installers. It refuses a tag that does not match `crates/tore-app/Cargo.toml`,
so the version must be changed before tagging. The app's menu label and
`--version` take their version from the tag, as described in
[packaging](DEVELOPMENT.md#packaging).

The release carries, for each platform: the game's installer
(`T.O.R.E-Fighters-<version>-windows-x86_64.msi`,
`-macos-arm64.dmg`, `-macos-x86_64.dmg`, `-x86_64.AppImage`) and the Linux
`T.O.R.E-Fighters-<version>-linux-x86_64.tar.gz`; the dedicated server,
`tore-server-<version>-windows-x86_64.zip`, `-macos-arm64.tar.gz`,
`-macos-x86_64.tar.gz` and `-linux-x86_64.tar.gz`, each with a `.sha256`
([downloading it](DEDICATED-SERVER.md#downloading-it)); and the master server,
`tore-master-<version>-linux-x86_64.tar.gz` with its `.sha256`
([operations](MASTER-SERVER.md#downloading-a-release)). Check the release page
lists all ten packages and the five checksums.

The workflow only writes the notes when it creates the release. To change them
after the tag is pushed, edit the file, commit it, and run:

```sh
python3 tools/release_notes.py docs/release/v0.1.1.md --body --ref v0.1.1 \
  | gh release edit v0.1.1 --notes-file -
```

## Compatibility identity

Release tags, `TORE_BUILD_VERSION` and commit stamps are informational and do
not affect multiplayer admission. Follow the protocol and simulation bump
rules in [Compatibility code](ARCHITECTURE.md#compatibility-code). The code
also depends on the player's imported content, so there is no single code
assigned to a release artifact. Protocol 71 requires master protocol 2;
upgrade the master before clients need its listings. The updated master also
serves released master-protocol-1/game-protocol-67 clients and servers, using
their original layout and a separate population. Existing 0.2.0 servers do
not need to be upgraded at the same time. See [master rollout](DEDICATED-SERVER.md#running-a-master-for-old-and-new-games).
