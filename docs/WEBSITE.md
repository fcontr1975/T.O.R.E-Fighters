# Project homepage

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

[index.html](index.html) is the standalone project homepage;
[releases.html](releases.html) beside it shows the [release notes](#release-notes-page). It uses inline CSS,
relative image paths, system fonts, and a small inline script for release
downloads and GitHub stars. No framework, external font, analytics, or build step is required.
The page and GitHub release links also work without JavaScript. Open the file
directly or serve the documentation:

```sh
python3 -m http.server 8080 --directory docs --bind 127.0.0.1
```

Then visit `http://127.0.0.1:8080/`. Keep `images/homepage/` beside the HTML when
copying the page. Standalone means a static document with adjacent images, not
an HTML file containing embedded image data.

## GitHub Pages

After the website change is reviewed and merged, GitHub Pages can publish it
from the repository's `main` branch and `/docs` folder using **Deploy from a
branch**. The included `.nojekyll` serves the files directly. No workflow or
repository setting is changed by this implementation.

All navigation within the page uses fragment links. Image paths work under the
repository's `/T.O.R.E-Fighters/` URL prefix. Documentation links point to the
readable Markdown on GitHub, so they do not depend on Jekyll converting the other
documents. Update the repository links and social-preview image URL if the
repository is renamed or hosted elsewhere.

## Latest release downloads

John requested live release downloads on 2026-09-23. The page makes one public,
unauthenticated request to GitHub's
[`releases?per_page=100` endpoint](https://api.github.com/repos/john-overton/T.O.R.E-Fighters/releases?per_page=100)
per page load. No token or backend is needed. The request has an eight-second
timeout. Release data is not kept in local storage, so a new visit can pick up
newly published packages.

Agent choice: select the most recently published non-draft release from the
returned list, including pre-releases. This matches the existing packaging
workflow, which publishes pre-releases. GitHub's
[`releases/latest` endpoint](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)
excludes pre-releases, so it would hide the project's current release channel.
The page explicitly labels pre-releases and shows the tag and UTC publication
date. Release notes link to that exact release.

Download URLs come from the release's actual uploaded assets, matched to the
names emitted by `tools/package/`:

| Platform | Required filename suffix | Primary download |
| --- | --- | --- |
| Windows Intel/AMD 64-bit | `-windows-x86_64.msi` | MSI |
| Windows portable | `-windows-x86_64-portable.zip` | Additional link on the Windows card, shown only when present |
| Windows 32-bit | `-windows-x86.msi` | Additional link on the Windows card, shown only when present. Releases after 0.2.1 have none (32-bit builds dropped 2026-10-25) |
| macOS Apple Silicon | `-macos-arm64.dmg` | DMG |
| macOS Intel | `-macos-x86_64.dmg` | DMG |
| Linux Intel/AMD 64-bit | `-x86_64.AppImage` (also matches the older `-linux-x86_64.AppImage`) | AppImage |
| Linux archive | `-linux-x86_64.tar.gz` | Additional link, or primary when AppImage is absent |

Each filename must start with `T.O.R.E-Fighters-`. Empty or unfinished assets
are ignored. The dedicated server's `tore-server-...` archives and the master
server's `tore-master-...` archive, with their `.sha256` files, share the
release but not that prefix, so the panel never offers them in place of the
game; operators take them from the release page
([dedicated server](DEDICATED-SERVER.md#downloading-it)).
The page accepts links only under this repository's HTTPS GitHub
release paths, and inserts release text as text, never as HTML. It never
constructs a guessed download filename or mixes assets from different releases.
Keep the suffix matching in the inline script aligned with the package scripts
if package naming changes.

An absent platform package is labeled **Not in this release** and links to the
release details. An empty feed displays **Coming soon**. Network errors, API
rate limits, and timeouts retain the working GitHub release links. With
JavaScript disabled, all four platform cards link to GitHub releases. The source
build guide remains available in every state.

The public feed was empty when checked during this pass. No version number or
download availability is hardcoded; packages appear automatically after a public
release is published with matching assets.

## Release notes page

[releases.html](releases.html) shows the player-facing notes in
[release/](release/README.md), one Markdown file per version, requested by John
on 2026-09-28. The homepage links to it from the top navigation, the roadmap and
the downloads panel; once the release feed loads, the downloads link opens the
latest tag's notes.

A static site cannot list a folder, so the page reads
`release/releases.json`, a newest-first list of file names, and fetches each
file beside it. The sidebar takes each release's name from its title line and
its date from the **Released:** field. `#v0.1.1` opens one release and
`#v0.1.1/known-limitations` one of its sections; an unknown version shows the
latest notes with a pointer to that tag on GitHub.

Agent choice: Markdown is rendered with
[marked](https://github.com/markedjs/marked) 18.0.14 and cleaned with
[DOMPurify](https://github.com/cure53/DOMPurify) 3.4.16, both pinned on
jsDelivr with subresource integrity hashes. They are the page's only external
scripts; the homepage still has none. If they fail to load, the notes are shown
as plain Markdown text. Without JavaScript the page links to the release folder
on GitHub. Links between release notes stay on the page, and relative links to
other repository documents open on GitHub. The page fetches its files, so it
must be served, as above, rather than opened from disk.

`python3 tools/check_docs.py` checks that the list and the files agree, so a
note cannot be left off the page.

## GitHub stars

The top GitHub link shows a gold star and the live repository star count,
requested by John on 2026-09-23. A separate public repository API request runs
alongside the release request, with the same eight-second timeout. Large counts
use compact notation, such as `1.2K`; the accessible link label and tooltip retain
the exact number. Zero is a valid count. Without JavaScript, or if the request
fails, the link keeps its static **GitHub ★ Star** invitation. Clicking it opens
the repository and does not automatically star anything.

## Content and design

Implementation mode. John requested a boxed-set homepage on 2026-09-23. The
olive slipcase, cream back-panel treatment, photographic cover, typography,
and numbered sections are agent-selected design decisions.
John requested a 50% increase to the hero's squadron patch on the same date:
its desktop size is now 198 pixels, up from 132, and its tablet size is 157.5
pixels, up from 105. The surrounding cover layout retains its spacing.

The page is an introduction, not a second feature ledger. Detailed status lives
in [the parity plan](parity-plan.md), [feature matrix](features.md), and
[roadmap](ROADMAP.md). Update the short overview and its date when the current
milestone or roster changes. Future milestones are explicitly marked as planned;
the homepage does not claim completed retail parity.

## Images

The six aviation photographs were supplied by John in `.local/webpage-pics/`.
Web copies are resized, metadata-stripped WebP images under `images/homepage/`:

| Original | Web copy | Placement |
| --- | --- | --- |
| `harrier_sunset_full.jpg` | `harrier_sunset_full.webp` | Box cover |
| `cockpit.jpg` | `cockpit.webp` | Aircraft feature |
| `f-4s.jpg` | `f-4s.webp` | Theater feature |
| `f-22.jpg` | `f-22.webp` | Quick Mission feature |
| `rafale-refuel.jpg` | `rafale-refuel.webp` | Project philosophy |
| `f-16-thunderbirds.jpg` | `f-16-thunderbirds.webp` | Getting started |

`squadron-patch.webp` is a reduced copy of the existing
`images/tore-fighters-logo.png`. `favicon.png` comes from
`images/tore-icon-fullsize.png`. Original files are unchanged. The photography
is labeled separately from the game's roster and screenshots. Supplied images
retain their existing ownership; the engine's GPL license is not an image license.

John subsequently requested game captures for inclusion in the homepage,
specifically dawn, missile firing, and takeoff. This is a scoped exception to
the normal local-only rule for generated retail derivatives, covering only
`game-dawn.webp`, `game-missile.webp`, and `game-takeoff.webp` in this folder.
It does not authorize publishing source media, extracted assets, or other
derivatives.

The current screenshots were supplied by John from his Pictures folder on
2026-09-23, with the cockpit image requested as the main gallery screenshot:

| Original | Web copy | Placement |
| --- | --- | --- |
| `screenshot-2026-09-23_20-03-54.png` | `game-dawn.webp` | Main cockpit screenshot |
| `screenshot-2026-09-23_19-54-19.png` | `game-missile.webp` | Missile launch |
| `screenshot-2026-09-23_19-58-03.png` | `game-takeoff.webp` | Takeoff |

Agent choice: retain the existing image URLs and convert the PNGs to
metadata-stripped WebP at quality 90, preserving their full 3840 × 2160
resolution and framing. The historical `game-dawn.webp` filename now holds
the cockpit image. Captions and alternative text describe the supplied images
without assuming aircraft identity, theater, or capture settings. The original
PNGs remain outside the repository; no game build identity was supplied with
them, and they do not establish retail parity.

The initial automated captures and website validation are recorded in the
[homepage baseline](baselines/homepage.md); those captures have been replaced
by the supplied screenshots above.
