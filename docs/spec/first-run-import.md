# First-run media import

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode. **Opinionated** host behaviour: the original game had an
installer and no importer, so nothing here is original behaviour. John set the
scope on 2026-09-21: mounted disc or folder, no raw ISO reading; proper
installers; unsigned first builds. Sequencing lives in the
[roadmap](../ROADMAP.md#1g-installer-and-first-run-import); the container
format is in [SETUP.ESA notes](../formats/esa-installer.md).

## What the player sees

1. The installed T.O.R.E application starts. If a valid import already exists
   in application data, the main menu appears as today.
2. Otherwise the app shows a **Locate Fighters Anthology** screen. It
   explains, in one paragraph, that the player's own copy is needed and that
   nothing is copied out of the repository. It offers:
   - a text field for a path, prefilled with the first automatically detected
     source if any;
   - drag-and-drop of a folder or a file onto the window;
   - a list of detected sources, each labelled by kind (see below);
   - **Import** and **Quit**. Alt+F4 (Command-Q on macOS) also quits, as on every
     other screen.
   - on Windows, the [graphics API choice](graphics-options.md#graphics-api) on the
     title line, Vulkan or DirectX 12. Tab reaches it, and Left, Right, Enter or Space
     change it. It applies when the game starts, with no restart.

   The screen is drawn on a flat panel with the larger of the two bundled
   menu fonts, because on a first run there is no retail art on disk to draw
   with. The panel fills all but a twelve-pixel margin of the 640 by 480
   canvas, one text line is seventeen pixels tall and lines are eighteen
   pixels apart, so the screen is still readable when the window is scaled to
   a 1080p or fullscreen display. Headings, the typed path and the button
   labels are white; the explanation, the field label and the source kinds are
   a light grey. When a pack does
   exist, which is the re-import case and the stale-cache case, the same
   screen is drawn over the Choose Activity frame the player was looking at.
   The path field takes typed characters, Backspace, Delete, Home, End, the
   arrow keys and Enter. There is no paste: drag-and-drop and automatic
   detection cover the cases a clipboard would, and no clipboard dependency is
   added. Tab moves between the field, the list and the buttons; Escape quits.

   When the app already knows a source, the remembered one or a developer
   checkout, it starts that import immediately instead of waiting for a click,
   so a stale cache and a fresh clone both reach the menu on their own. Only a
   re-import asked for from Pref always waits, because the player opened that
   screen to change something.
3. Clicking **Import** changes the screen at once to **Starting import...**
   with the current setup step underneath ("Opening the game files",
   "Identifying the game build", "Finding aircraft and theaters") and a block
   sweeping along the progress bar, about one crossing every one and a half
   seconds. Setup reads nothing countable yet and can take tens of seconds
   from a disc, so the moving bar is what tells the player the import is
   running (requested by John on 2026-09-26; step wording and sweep speed are
   agent choices).
   Import then shows progress by archive and resource count, then the summary
   already written to the import report: build read (`1.0 (disc)` or `1.02F`),
   archives read, optional parts that were missing (recorded music, radio).
   The import runs on a worker thread, so the screen keeps drawing and the
   window keeps responding. **Continue** opens the main menu.
4. A failed import stays on the locate screen with the reason in plain words:
   the folder is not a Fighters Anthology source, an archive is truncated, the
   executable is an unreviewed build, or the disk is full. Nothing partial is
   kept.

The same screen is reachable later from **Pref** as **Re-import media**, with
the remembered source prefilled and the menu behind it. Continuing from there
rebuilds the game from the new pack without restarting the application.

`--smoke-test` drives the screen without a player: it imports the prefilled
source, continues as soon as the import finishes, presents one menu frame and
exits, so a first run can be checked end to end in one command.
`--snapshot PATH --snapshot-state locate` writes the screen's layout headlessly
with a fixed candidate list, without needing any media. The `locate-starting`,
`locate-importing` and `locate-done` states draw the same screen as a disc 1
import starts, part way through and at the end, using fixed figures; the
README's getting-started pictures come from `locate`, `locate-importing` and
`locate-done`.

## Accepted sources

A source is a folder. The app decides its kind by content, never by name:

| Kind | Recognised by | Provides |
| --- | --- | --- |
| Installed directory | `FA_1.LIB`, `FA_2.LIB` and `FA.EXE` present, case-insensitive | everything, including optional `FA_4B.LIB`/`FA_4D.LIB` music |
| Disc or mounted image | `SETUP.ESA` present with the `ELECTRONIC_ARTS_ARCHIVE_FILE` magic | the same five files read from inside the container |
| Extracted disc | a folder into which a disc image was copied; same rule as above | same |

Only disc 1 is required. A raw `.iso` file is not opened; dropping one shows
"Mount the image and choose the mounted folder" with the platform hint
(double-click on Windows and macOS; `udisksctl loop-setup` or the file
manager on Linux).

Both known executable builds are accepted: the disc's 1.0 build and the
1.02F patched build. Any other build is refused by name of the hash, with
the message that it has not been reviewed; the archives are not imported
either, so the cache never holds a half-known data set.

## Automatic detection

At the locate screen the app scans, in order, and lists every hit:

1. The folder next to the executable and its `gameassets/fighters-anthology`
   child (developer checkouts).
2. Removable and optical volumes: `/run/media/*/*`, `/media/*/*` and
   `/mnt/*` on Linux; `/Volumes/*` on macOS; every drive letter with a volume
   on Windows.
3. Conventional install folders on Windows: `Program Files` and
   `Program Files (x86)` under `Jane's Combat Simulations`, and `C:\JANES`.

Each candidate is checked by content as above, one directory level deep,
without following symlinks. Scanning stops after 2 seconds; anything slower is
left to the manual field.

## Remembering the source

After a successful import the app stores the source path and kind in
application data beside the pack (`media-source.txt`, one line per field).
When a later build of the importer needs more resources than the cache holds,
the app re-imports from that path silently if it is still valid, and otherwise
shows the locate screen with the old path in the field and the reason.

## Installers

Each platform gets a proper installer built in CI from the same commit:

| Platform | Package | Installs to | Also |
| --- | --- | --- | --- |
| Windows x86_64 | MSI | `%ProgramFiles%\T.O.R.E-Fighters` | Start menu shortcut, uninstaller |
| macOS arm64 and x86_64 | DMG with `.app` bundle | `/Applications` by drag | unsigned: first launch by right-click, Open |
| Linux x86_64 | AppImage, plus `.tar.gz` | anywhere | desktop entry when run from the AppImage |

First builds are unsigned on every platform; a signing step is added later
without changing the package layout. The installer never contains or asks for
retail media. `tools/check_assets.py` runs against every package before it is
published; a package that fails is not released.

## Validation

- Synthetic tests for the ESA reader: stored and compressed entries, terminator,
  overlapping or out-of-range offsets, unsupported DCL header.
- Unit tests for the startup decision: a readable pack plays, a known source
  imports without asking, an unknown source asks when there is a window, and a
  headless run with no source fails with the terminal message.
- Synthetic tests for source detection: each kind, mixed case, nested one level.
- The two executable fingerprints each decode the same creator, cloud, flare
  and radio content in a fixture-driven test that uses no retail bytes.
- Manual: a fresh machine per platform, install, point at a mounted disc,
  fly the F/A-18D free-flight check from the README.
