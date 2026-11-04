# Asset packs and content integrity review draft

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Research mode: codebase review and proposed host behavior, not recovered FA
behavior. **Draft for review, not implemented or approved for implementation.**
Prepared from the local `multiplayer` branch at `7d25975` on 2026-10-03 and not
rechecked against later code. The proposed sequence is under
[implementation stages](#implementation-stages); it is not on the roadmap.

The proposed system gives stock FA content and TORE mods one asset root. Each
logical asset has a typed pack, such as one aircraft or one theater. Existing
gameplay remains unchanged while storage and loading move to that structure.
Stock integrity checks and bounded readers are part of the first slice.

## Requirements and proposals

John requested on 2026-10-03:

- One asset root supporting imported FA content and TORE content and mods.
- Individual packs grouped by asset type, such as `Cache/Aircraft/FA-18D.pack`
  and `Cache/Theater/Ukraine.pack`.
- Different manifest definitions for different object types.
- Tools that create packs from a required authoring structure.
- Master hashes for default content so modifications can be flagged.
- Protection against malicious packs.

All design details below are **opinionated agent proposals for review**. In
particular, blocking modified stock, the proposed directory defaults, mod
activation rules, cryptographic choices, and stage boundaries are not decisions
already made by John. No AI or autonomous behavior changes are included.

## Current implementation

| Area | Evidence and consequence |
| --- | --- |
| Shared FA import | [Importer](../../crates/tore-import/src/import.rs) reads selected resources from `FA_1.LIB` and `FA_2.LIB`, optional music libraries, and reviewed `FA.EXE` data. Libraries alone are currently insufficient. Original modules are never executed. |
| Runtime cache | [Pack reader](../../crates/tore-import/src/pack.rs) reads a flat resource map from a generated `menu-*.pack`. It has size/count limits and rejects duplicate names and trailing bytes. The name predates its broader gameplay contents. [Existing contract](import-cache.md). |
| TORE content | [Engine artwork loader](../../crates/tore-app/src/engine_material.rs) has separate filesystem searches and `TORE_ASSET_DIR`. Some project fonts and icons are embedded. |
| Catalogs | [Aircraft identities](../../crates/tore-formats/src/aircraft.rs) and [theater discovery](../../crates/tore-formats/src/theater.rs) use fixed lists. Packaging alone cannot make arbitrary new aircraft or maps selectable. |
| Multiplayer | [Resource tracking](../../crates/tore-world/src/resources.rs) records mission reads and FNV-1a hashes. These support comparison, not cryptographic authentication. |

## Asset root and ownership

Proposed installed default on macOS:
`~/Library/Application Support/T.O.R.E-Fighters/assets/`.
Other platforms use their existing application-data location plus `assets/`.
Development can use an ignored staging root such as `.local/assets/`.
The exact configuration interface remains a review decision. Preserve existing
`TORE_DATA_DIR` and `TORE_ASSET_DIR` behavior through explicit compatibility
handling; neither override changes meaning silently.

```text
assets/
  Sources/
    FA/                       User-owned media, or a remembered external source
    TORE/                     Shipped project content used to build the cache
  Mods/
    example-mod/              Retained editable source or original mod package
  Cache/
    Aircraft/
      FA-18D.pack
      F-22A.pack
      Rafale-C.pack
    Theater/
      Ukraine.pack
      Kurile.pack
    Weapons/
      AIM-9M.pack
    Objects/
      Control-Tower.pack
    Shared/
      Common-Effects.pack
      Common-Fonts.pack
    UI/
      Menus.pack
    Audio/
      Radio.pack
```

Names other than John's Aircraft/Theater examples illustrate the proposed
grouping. Weather and other resource types need definitions as their packages
are introduced. A pack is a logical content unit, not necessarily one texture
or sound. A weapon catalog may remain grouped initially.

`Cache/` is generated and rebuildable. It never holds the only copy of authored
mods, downloaded packages, or source media. Settings, saves, and recordings keep
their existing application-data location. Cache cleanup cannot remove them or
retained sources. Retail media and derivatives remain local and untracked.

Physical filenames are conveniences. References use stable package IDs and
resource IDs. Stock and mod identities occupy separate namespaces. Duplicate
IDs fail validation. A replacement mod keeps its own identity and declares its
stock target; it does not overwrite the stock package. Filename collisions get
distinct managed storage names or a clear error, never silent replacement.
Directory placement alone does not activate a mod.

## Package definitions

Every pack contains a common manifest envelope and a typed definition.

| Common envelope | Aircraft definition | Theater definition |
| --- | --- | --- |
| Package ID, version, asset type | Exact aircraft identity | Theater identity and display name |
| Container and schema versions | Flight configuration or supported adapter reference | Terrain grid and elevation references |
| Claimed source and provenance | Exterior geometry and textures | Textures and palettes |
| Dependencies and compatible versions | Cockpit, HUD, instruments | Object placements and airport data |
| Resource list, lengths, SHA-256 hashes | Animation configuration | Environment references |
| Explicit replacement targets | Hardpoints and equipment | Required object/shared packages |

Origin claims are metadata, not proof of trust. Verified stock status is computed
against the trusted catalog. The manifest's resource hashes exclude the manifest
itself; the external catalog hashes the complete pack, including its manifest.

The initial aircraft definition references existing FA resources and implemented
adapters. It does not imply that animations or flight behavior are already
editable through files. Preserve `F18.PT` as F/A-18D and `RAFALE.PT` as Rafale C.
Do not introduce variant aliases or change flight adapter defaults.

Aircraft-specific content belongs in its aircraft pack. Common fonts, effects,
equipment, and weapons can live in shared or separate packs. Dependencies must
be explicit and bounded. Resolve them in a deterministic order, reject cycles,
and identify missing requirements by package and resource. A package can be
independently replaceable without being self-contained. A future export command
can collect redistributable dependencies into a bundle without including retail
content.

For migration, combine selected packs into the existing resource map before
calling current readers. Retain package ownership alongside that map for error
reporting. Stock duplicate resource names require explicit shared ownership or
identical, documented compatibility handling. Mod overrides require declared
targets and enabled order; conflicting overrides must be reported.

## Authoring and tools

Editable mod folders contain a manifest and the directories their type requires,
such as `aircraft/`, `textures/`, and `audio/`. Type-specific validators identify
missing fields and unsupported capabilities before producing a pack.

The FA importer and mod tools target the same package contract. Planned tool
operations are validate, build, and inspect. Inspect reports identity, version,
type, resources, dependencies, replacements, and integrity status. Command names,
manifest syntax, and the binary container layout remain undecided.

Identical inputs, schema, and builder version must produce identical pack bytes.
Use stable ordering and encoding; keep timestamps, local paths, and machine
details in external import reports. A build writes temporary output, validates
it by reading it back, and activates a complete package set only after success.
Failure leaves the prior usable set intact. Rebuilding a set cannot expose a
mixture of old and new dependency versions.

## Stock catalog and modification reporting

Ship a versioned trusted stock catalog with the application, outside the writable
cache. Each entry binds package ID/type, content version, supported source set,
builder/schema version, complete pack SHA-256, and resource hashes. Resource
hashes allow an error to identify what changed. Hashes and metadata can be
committed; retail payloads cannot.

Establish reviewed baselines for supported FA source sets and TORE releases.
Identifying `FA.EXE` alone does not establish that every library is unchanged.
A hash generated from arbitrary imported media means only "unchanged since
import," not "verified stock." Unknown source sets remain unverified until
evidence establishes a baseline. Optional music and accepted source variations
need explicit catalog entries rather than false modification warnings.

The catalog must not be silently regenerated from installed packs. If catalogs
are later updated separately, authenticate updates with signatures checked
against an application-trusted key and define version/rollback rules. Do not
create custom cryptography. Signing library choice and key management need
review before that feature is implemented.

| Reported state | Proposed behavior |
| --- | --- |
| Verified stock | Matches a supported trusted baseline and passes validation. |
| Modified stock | Claims a known stock identity but differs. Show changed resources; block silent stock activation and offer re-import or an explicit mod workflow. |
| Unverified source | No reviewed baseline exists. Never label it verified stock; activation policy remains open for review. |
| Mod | Explicit custom content. Show enabled identity/version and overrides. Stock multiplayer policy may refuse it. |
| Invalid | Reject structural, semantic, dependency, or budget violations regardless of claimed origin or signature. |

## Safe loading

Threat model: a downloaded pack or modified local cache may be malicious.
Protect filesystem boundaries, process stability, resource budgets, and content
identity. This does not promise protection after an attacker replaces the game
binary or its trusted installation. A matching hash or valid signature does not
replace parser validation or prove that content is harmless.

- Packs supply data only. No scripts, native plugins, shell commands, install
  hooks, or pack-supplied shaders. Original FA modules remain inputs to reviewed
  bounded data readers and are never executed.
- Validate the container, supported schema, and each typed payload. Check
  lengths and arithmetic before allocation, finite numeric values and ranges,
  image dimensions, geometry indices, audio sizes, and references before use.
- Apply both per-entry and aggregate budgets, including dependency count/depth,
  decoded memory, processing work, and renderer upload limits. Start without
  pack compression unless justified. Any later compression needs enforced
  output/work limits, not just a declared uncompressed size.
- Use logical internal resource names, not arbitrary host paths. Reject absolute
  paths, traversal, symlink escapes, duplicate normalized names, and cross-platform
  case collisions. Packages cannot request network access or arbitrary file reads.
  Prefer reading entries directly over extracting them to caller-supplied paths.
- Validate staged bytes before activation and consume the verified snapshot.
  Avoid checking one file and later reopening potentially different bytes.
  Runtime checks apply even when content came from our own builder.
- Reject malformed data without panics or partial activation. Show a bounded,
  plain-language error naming the responsible pack. Fuzz container and typed
  readers and add synthetic corruption/dependency/budget tests.

Exact limits are not yet specified. Before implementing each type, measure its
supported stock content, document bounded headroom and workload limits, and test
the boundaries. Existing cache bounds are not automatically suitable for every
new type. Rust and data-only policies reduce risks but do not replace this work.

Security design references: [OWASP file handling](https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html)
for content validation and decoded-size limits;
[The Update Framework](https://theupdateframework.github.io/specification/latest/)
for authenticated metadata and update versioning. These inform proposals, not a
claim that TORE currently implements or has been audited against either system.

## Multiplayer and recordings

Keep the existing mission resource comparison through migration. Security
integrity uses cryptographic hashes; existing FNV-1a comparisons are not a trust
boundary. Any network hash/schema change needs an explicit protocol compatibility
decision and tests, not a silent replacement.

Include gameplay-affecting package definitions and resolved dependencies in
mission compatibility checks. Report mismatches by package/version and resource.
Define cosmetic exceptions only after checking that those resources cannot
affect simulation. Record enough content identity with recordings to explain
missing or changed dependencies. Preserve existing recording readers.

Downloaded packages, automatic distribution, and in-session hot reload are not
part of the first implementation. Freeze the active content set during a mission.

## Decisions for review

1. Confirm the asset-root default and override/migration interface.
2. Choose the first mod example: replacement artwork, a supported FA library mod,
   or a later new aircraft definition. Arbitrary FA mod libraries are not
   currently imported as overlays.
3. Confirm blocking modified stock and decide how unverified source sets behave.
4. Choose manifest syntax, container evolution, dependency version rules, and
   the physical naming policy for multiple versions and colliding display names.
5. Agree multiplayer stock/mod policies and the scope of cosmetic exceptions.
6. Set measured budgets, reviewed stock baselines, and cryptographic dependency
   choices before implementation. Keep `tore-formats` dependency-free.

## Implementation stages

Proposed implementation sequence. These stage boundaries are agent proposals,
not completed work.

1. **Contract and baselines.** Resolve the review decisions, specify aircraft
   and theater schemas plus the common container, define measured budgets, and
   establish trusted stock baselines. Document format details under
   `docs/formats/` when selected. Exit: the first slice has an implementable
   contract and synthetic fixtures, with unknown source sets labeled honestly.
2. **One root and one aircraft pack.** Produce `Cache/Aircraft/FA-18D.pack` and
   temporary `Cache/Shared/FA-Base.pack`. Preserve existing import/cache access,
   explicit override compatibility, app/server/headless consumers, and flight
   adapters. Include stock checks, bounded validation, and safe activation from
   the start. Exit: resolved resource bytes and existing mission manifests match
   the current import; migration and failed imports preserve the usable set.
3. **Typed packages and authoring tools.** Split remaining aircraft and theaters,
   then other logical assets as their contracts are ready. Add validate/build/
   inspect operations, deterministic output, shared ownership, dependencies, and
   explicit mod activation. Exit: a small replacement mod can be enabled and
   disabled without rewriting stock; errors identify invalid/conflicting packs.
4. **New definitions and multiplayer policies.** Move supported catalog and
   configuration choices out of fixed lists incrementally. Include those
   definitions in cryptographic content checks and recording metadata with
   explicit compatibility handling. Exit: each newly supported content type has
   its own acceptance evidence; gameplay mismatches are refused and declared
   cosmetic exceptions are tested. Packaging does not imply new gameplay support.

Every implementation slice runs the repository checks and relevant battery
scenarios using synthetic committed fixtures. Add malformed-input, duplicate ID,
path escape, missing/cyclic dependency, resource-budget, modified-stock, and
interrupted-activation cases. Check pack bytes are reproducible across supported
platforms. Use local owned media for before/after resource and mission comparisons;
record evidence without committing payloads. Rendering changes also need the
wrapped display smoke test. Broader adversarial testing includes fuzzing and a
security review; neither is satisfied by ordinary unit tests alone.

## Validation status

This draft records a read-only code review and the discussion, not implemented
pack behavior, runtime validation, retail comparison, or a security audit.
Future implementation checks are specified in the stages above.
No retail bytes, extracted resources, or generated packs belong in this change.
