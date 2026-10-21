# Phase 4: library management, backup and multipart workflows

Date: 2026-10-10
Status: locally implemented, verified, packaged and installed; Phase 5 may proceed.
Every capability below is Free in Community and the unified app.
Pinned upstream: f551c4601c03072362b426c652b5dd37e4c55601 (GUIDE.md).

## Scope and behavior
- Find content-hash duplicate groups within explicit current-filter model IDs. Show previews
  and selection counts; never silently widen the cleanup scope.
- Reveal an admitted model's actual source location. ZIP members reveal the outer archive.
- Remove explicit selected records recoverably. Offer an explicit separate original-file
  Trash action for loose sources. Shared managed blobs and unselected sources survive.
  ZIP-member removal does not rewrite the archive or remove its other members.
- Preserve removed IDs, metadata and print history; scans must not resurrect removed
  records. A restore workflow handles conflicts without overwriting a destination.
- Manual full backup/restore includes a consistent SQLite snapshot, managed assets and
  owned recovery records. Indexed sources remain references; restoration never writes
  files to external indexed roots implicitly. Reject traversal/symlinks/encryption,
  malformed catalogs, oversized/inconsistent/hash-mismatched entries and existing outputs.
- Restore is staged and verified before atomic replacement. Preserve the previous library
  as a rollback backup, block scan/render jobs during replacement, close SQLite handles,
  invalidate old edits/viewer sources, and recover the old directory on swap failure.
- Purge catalog data only through explicit confirmation and a preserved restorable backup;
  external originals are untouched. Owned orphan/cache maintenance validates namespaces and
  references, preserves custom previews, current viewer sources and handoff assets.
- Expand/collapse multi-file source/manual bundles; show details and preview all supported
  parts laid out in a grid. Match baseline preview capacity of 32 with an explicit notice
  for larger bundles. Single-file folders appear as ordinary rows.
- Bundle handoff passes every admitted selected part as literal arguments to a freshly
  detected supported slicer. Never silently truncate. Preserve verified handoff copies
  across asynchronous slicer loading; stale membership/content/preview tokens fail.
  macOS sends open a new slicer instance.
- Print Roulette chooses uniformly from the documented current-filter scope, excluding
  unavailable/removed models. Show the chosen result and open its normal viewer workflow.
- Support custom PNG/JPEG previews and embedded 3MF preview images where valid, with
  bounded decoding and preferred custom images surviving regeneration.

## API and ownership
- index_storage owns crates/library-core, new crates/library-backup, native
  library_management.rs and LibraryState maintenance/replacement helpers; root and native
  Cargo dependency wiring for those modules. Duplicate/removal/restore/backups/cache APIs
  validate optimistic versions and use owned paths. No user live-data tests.
- index_native owns library_bundle.rs, handoff.rs and any bundle-preview-only module.
  Independent preview state avoids replacing the main viewer; native literal launch
  guards and verified cache lifetimes remain in force.
- catalog_ui owns library.ts/ipc.ts/main library wiring/styles and new management/bundle
  dialogs, filtered-scope duplicate review, restore/backup/purge flows and roulette.
- Coordinator owns lib.rs registration, custom/embedded image support, docs, cross-review,
  browser QA, Community/unified final build/install and local checkpoints.
- Workers coordinate exact typed DTO/command names before overlapping integration. They
  are not alone and must preserve other workers' edits and unrelated Pro plans.

## Gates
- Duplicate scope, stale edits, shared blobs, scan tombstones, recoverable removal/restore
  and conflicting destinations; no unselected/outside-original mutation.
- Backup/restore IDs/history/tags/bundles/settings/assets and SHA checks; corrupt/traversal/
  bomb/symlink cases; failed swap rollback; job admission and edit invalidation.
- Real multipart STL/3MF preview geometry, bounded layout and explicit capacity notice.
  Literal Unicode/metacharacter multi-file argv, stale tokens, unknown slicers, original
  byte preservation and handoff-copy lifetime.
- Filtered roulette, grouping/expand/details, keyboard/focus, deletion counts and errors,
  browser narrow/desktop flows and bounded image decoding/custom preservation.
- Independent data/native/UI/security review; fix evidence-backed integrity findings.
- Full Community/unified checks and app packages; preserve and install/launch the new
  local app before phase 5. Native capture limitation and cross-platform release gates
  remain explicit. No push or external publication.

## Review and validation record
- Native/storage cross-review moved handoff assets to a stable sibling namespace, added
  shared I/O admission, capped aggregate preview geometry while loading and rejected
  computed non-finite coordinates. Main and bundle previews keep separate generations.
- Single handoff now verifies the committed source digest and keeps viewer locks through
  stable copy and fresh literal launch. Public OriginalSource API remains compatible.
- Restore rollback retries isolating failed replacements and fails closed with maintenance
  armed if recovery cannot complete. Previous data is preserved; viewer is invalidated
  before replacement. Restored versions exceed the prior maximum to reject stale edits.
- Backups reject executable/unsupported SQLite schema before mutable access, bound both
  file passes and traversal counts, validate manifest/entries/hashes and stage before swap.
- Undo derives destinations from catalog roots, bounds and hashes the actual restored
  stream, preserves collisions, and restores source availability after missing scans.
- Inherited imports now bound reads, validate owned namespaces/DB, and publish immutable
  blobs without overwriting mismatched crash leftovers or following symlinked namespaces.
- Hash/path changes advance optimistic model versions; unchanged scans retain them.
- Cache maintenance excludes current viewer extracts and recent/active handoff assets.
  Namespace and known-file checks protect originals and unrecognized files.
- UI source directory, parent/license filters and keyboard resizing complete the pinned
  guide baseline. Duplicate scope, roulette and selection share the composed filters.
- Browser fixture rendered actual native-generated mixed STL/3MF geometry (13 triangles),
  verified bundle send failure/retry and close, exact 20-model available directory scope,
  one checked versioned removal, roulette and no purge call without a backup destination.
- At 480px, bundle/duplicate/management dialogs are 448px wide without horizontal overflow.
  Desktop keyboard resizing changed the source rail from 270px to 286px and kept focus.
  Browser console reported zero errors. Native geometry and screenshots are in output/.
- Image tests verify real JPEG and 3MF embedded-image conversion, source byte/mtime retention,
  size/dimension/pixel bounds, safe internal relationships, DTD rejection, stale versions,
  symlink refusal and custom preview priority during explicit regeneration.
- The initial purge UI approval rejection was resolved by selecting a backup destination
  before typed PURGE admission. No live purge/removal/provider action is performed here.
- Independent review targets new data/I/O boundaries. A full unrelated repository secrets,
  CI and skill-supply-chain audit is deferred; network/auth auditing applies in phases 5–6.
- Full Community gate: 80 frontend tests/226 assertions; native 138 passing/1 ignored;
  storage 35; backup 4; recovery 16; other workspace suites pass with one existing
  ignored test. Default Clippy passes with existing unrelated warnings. New core
  warning fixed; changed-crate --no-deps Clippy with -D warnings passes.
- Community and unified macOS packaging pass; unified frontend 44 tests and native
  workspace/default Clippy pass. Local public b28ff37 / unified a50835c.
- Installed executable SHA256:
  f98718830607da8941a837a9db47a56a4c65b96af798e09eed751fe6dc97706d.
  Signature verified and process 66749 launched. Previous bundle preserved at
  output/app-backups/GoggleLab-before-phase4-20261010.app.
- Native window capture remains unavailable (cgWindowNotFound). Browser/fixture
  evidence does not establish native visual interactions, actual slicer launch,
  Windows/Linux runtime behavior or external release readiness.
