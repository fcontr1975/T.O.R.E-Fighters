# Release notes

One Markdown file per release, written for players. The same file is shown on
the [releases page](https://john-overton.github.io/T.O.R.E-Fighters/releases.html),
read on GitHub, and used as the GitHub release description when the tag is
pushed ([releasing](../RELEASING.md)).

| Release | Title |
| --- | --- |
| [v0.2.2](v0.2.2.md) | Notch, Chaff and Energy |
| [v0.2.1](v0.2.1.md) | Multiplayer, Patch Up, Buttercup |
| [v0.2.0](v0.2.0.md) | Wake up, dear, multiplayer is here! |
| [v0.1.3](v0.1.3.md) | Bug Bash, Overspeed, World Edge and Loaded Takeoffs |
| [v0.1.2](v0.1.2.md) | Explosions, Sound Prefs, Smarter AI and Flight Views |
| [v0.1.1](v0.1.1.md) | Replays, Flares, Chaff and Quality of Life |
| [v0.1.0](v0.1.0.md) | Quick Mission Creator |

## Adding a release

1. Copy the [template](#template) into `vX.Y.Z.md`, named after the tag.
2. Add the file name to the top of the `releases` list in
   [releases.json](releases.json). The list is newest first; the releases page
   reads it to find the notes, because a static site cannot list a folder.
3. Add a row to the table above.
4. Run `python3 tools/check_docs.py`. It fails when a file and the list
   disagree, when the list is out of order, when a title does not match its file
   name, or when a note contains an em or en dash.

Write the notes before pushing the tag. The release workflow publishes the file
as the GitHub release, using the title line as the release name and making
relative links absolute. [Releasing](../RELEASING.md) has the command for
changing the notes after the tag is pushed.

## Template

```markdown
# vX.Y.Z: Short Name of the Release

**Released:** YYYY-MM-DD · **Channel:** Pre-release · **Tag:** [vX.Y.Z](https://github.com/john-overton/T.O.R.E-Fighters/releases/tag/vX.Y.Z)

Two to four sentences: what this release means for a player, and the one thing
they should try first.

## Getting started

1. Only when installing or first-run steps change.

## Highlights

### The biggest change

- What the player can now do, and how.

## Improvements

### Area of the game

- One change per bullet.

## Fixes

- What was wrong, as the player saw it, and that it is fixed.

## Known limitations

- **What does not work yet, in bold.** Then one sentence on why or what to do.

## What's next

The next milestone, with a link to the roadmap.

## Feedback

Where to report bugs.
```

Leave out any section that has nothing in it. Keep the order above.

## Format

- **Title.** `# vX.Y.Z: Name`, where `vX.Y.Z` matches the file name and the tag.
  Use the same name as the GitHub release, in title case.
- **Metadata line.** Directly under the title, in the order shown in the
  template. The date is the GitHub publication date in UTC, written
  `YYYY-MM-DD`. The channel is `Pre-release` or `Release`. Separate the fields
  with ` · `.
- **Headings.** `##` for sections, `###` for groups inside a section. Never skip a
  level, never go below `###`, and never put a link in a heading.
- **Bullets.** One change per bullet, each a full sentence or a clean fragment
  that ends with a full stop. Start with what the player sees or does, not with
  the code that changed. Bold only the name of the feature or the one fact a
  skimmer must not miss, at most once per bullet.
- **Keys and menus.** Keys in bold with `+` between them: **Ctrl+B**,
  **Alt+Shift+1**. Menu paths in bold with ` > `: **Pref > Re-import media**.
  Quote on-screen messages in plain double quotes: "Out of chaff".
- **Code formatting.** Only for file names, extensions and commands: `.msi`,
  `vX.Y.Z.md`.
- **Numbers.** Give the numbers a player would notice, with units: 30 seconds,
  1,500 ft, 1/8x to 16x. Use a comma in thousands.
- **Tables.** Only for true side-by-side data, such as a package for each
  platform. Keep each cell to a sentence.
- **Notes and warnings.** A `>` quote with its first sentence in bold. At most
  one per release, near the top.
- **Wrap** prose at about 80 columns. The renderer joins the lines.

## Links

- **Other release notes:** relative, such as `[v0.1.0](v0.1.0.md)` or
  `v0.1.0.md#known-limitations`. The releases page turns these into links to
  that release on the page.
- **Repository documents:** relative, such as `[mission replays](../REPLAYS.md)`.
  They work on GitHub, and the releases page points them at GitHub too.
- **The website:** absolute `https://john-overton.github.io/T.O.R.E-Fighters/`
  links.
- **Downloads:** link the tag's release page, never an individual file. The file
  names change with each version.
- Link text says where it goes: "the [feature matrix](../features.md)", never
  "click [here](../features.md)".

## Style

These follow the [agent instructions](../../AGENTS.md).

- **No em dashes or en dashes.** Use a comma, a colon, or a new sentence. For a
  range, write "1 to 5" or "Alt+2 to Alt+5". `tools/check_docs.py` enforces this
  for this folder.
- **No emoji** and no exclamation marks (a release title John chose is the exception).
- **Plain English** for a smart player: short sentences, no unexplained jargon.
  Lead with what a change means, not how it works.
- **Be honest.** Say what has not been tested and what is still approximate.
  Never claim parity with the original that has not been shown.
- Write "the original" or "the original game" for Fighters Anthology, and
  "T.O.R.E addition" for something the original never had.
- Straight quotes and apostrophes (`"` and `'`), not curly ones.
