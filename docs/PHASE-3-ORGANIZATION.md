# Phase 3: organization, metadata and print history

Date: 2026-10-10
Status: local gate complete; phase 4 may proceed.
All capabilities are Community/Free in standalone and unified builds.

## Outcome
Edit designer, source URL, notes, model license, tags/categories and print status for one
model or explicit bulk selections. Record/review/edit/delete print attempts with dates,
quantities, filament references and notes. Filter by history/status and link parent/child
models. Manage virtual collections and group in-place source folders/ZIP bundles.

## Contracts and invariants
- Schema 3 is additive and backed up before migration. Existing model/tag IDs, source
  hashes, colorway identities and source locations survive. Legacy description seeds notes.
- Metadata updates use expected metadata versions. A bulk patch applies only selected
  fields, validates every target, and commits atomically; stale targets are reported.
- Tag IDs remain stable on rename; category/name CRUD validates bounded input and affected
  models' versions advance. Tag membership changes do not mutate source files.
- Status domain: unprinted, want, queued, printing, printed, failed. Print attempts store
  date, positive bounded quantity, notes, optional filament references and outcome.
  History/count/date filters preserve previous successful prints regardless of current status.
- Parent/child links reject missing/deleted targets and cycles. Collections remain virtual.
- Source folder and ZIP bundles are automatic grouping identities, separate from manually
  assigned collections. Existing source models retain IDs on unambiguous rename.
- ZIP scanning is opt-in per root. Outer archive path and member name form a compound
  source identity. ZIPs are streamed; directory count, decompression and per-model sizes
  are bounded. Absolute/traversal/symlink/encrypted/unsupported entries are rejected.
- Selected members extract only into owned cache for preview/handoff. Original archives
  remain untouched. Thumbnail-only extractions are temporary and do not accumulate full
  copies of every source member. Viewer copies remain valid for active-source handoff.
- Read-only recovery accepts v1/v2/v3 and retains organization/history/bundle/archive
  references while managed original exports keep their byte verification.
- UI dialogs are keyboard accessible, restore focus, handle conflicts explicitly and
  never render user metadata via innerHTML. Bulk selection scope is clearly stated.
- Full multipart bundle preview/handoff, duplicate cleanup and backup/restore are phase 4.

## Workstreams
- index_storage: schema3, metadata/tag/history/versioned batch/relations/bundle/archive
  source contracts and fixture tests.
- index_native: native wrappers, safe ZIP scan/extraction/source resolution and tests.
- catalog_ui: editors, selection/tag manager/history/relations/collections/bundle filters,
  archive toggle, IPC/main wiring and tests.
- Coordinator: reconcile public API, recovery adapter, docs, independent review, browser
  QA, full Community/unified checks, build/package/install and local checkpoints.

## Gate before phase 4
- Migration fixture retains prior data and a restorable prior-schema snapshot.
- Metadata validation, stale edits, exact bulk scope, rollback, tags/categories and versions.
- History persistence/quantity/date/outcome/filter behavior and parent-cycle protection.
- Folder/ZIP grouping and source/member uniqueness; root/member rename and duplicate safety.
- ZIP traversal/bomb/symlink/encryption/unsupported cases; originals unchanged and temporary
  extraction cleanup; actual STL/3MF member opening/thumbnail generation.
- Browser metadata/batch/history/tag/collection/bundle workflows, error/focus and narrow UI.
- Full native/frontend suites and standalone/unified builds; installed app signature/hash.
- Existing native-capture limitation remains recorded, and Windows/Linux runtime validation
  remains necessary before release claims. No publication is authorized.

## Independent review and fixes
- Native/storage cross-review closed archive partial-batch persistence, full-payload
  fingerprinting, member-loop cancellation and paused-ZIP reconciliation.
- UI review found oversized indexed sources could be treated as a completed scan.
  Scans now remain incomplete and opening/thumbnail admission enforces the root limit.
- Core API bounds now enforce up to five bounded filament references and bounded tag names.
- Recovery accepts valid multiline notes and the same tag/filament lengths as mutable data.
- Browser review fixed explicit accessible field names, post-save focus restoration and
  local-calendar default print dates. Metadata is text-only, including adversarial strings.
- Browser fixtures verified 42-target batch scope with only the selected designer field,
  single edit, conflict display/retry, print create/edit/delete, tags and history filtering.
  The editor is 448px wide at a 480px viewport, with no horizontal overflow; screenshot
  output/phase3-editor-narrow.png. Browser console reported zero errors.
- Browser callbacks are synthetic. Native archive/DB operations use isolated real fixtures;
  no tests operate on the user's live library or launch a real slicer.

- Final source grouping regressions cover root/singleton files, stable directory renames,
  folder splits, full archive renames, partial/removed members and archive-to-loose moves.
  Recovery is exercised with external sources removed. Automatic bundles are scanner-owned;
  explicit assignments target manual bundles. Whole-group rename retention requires an
  unambiguous target for every old member; absence alone is insufficient.
- Final Community native results: 111 native tests plus 31 storage and 16 recovery passed;
  other workspace suites passed with two existing ignored tests. Frontend: 72 tests /
  205 assertions. Standalone app packaging passed. Unified packaging/install remains the
  final local gate before phase 4.


## Final local gate
- Public implementation checkpoint 33e1e8e; unified checkpoint dacadd9.
- Final Community frontend/native/default Clippy and standalone build passed. No new
  library warnings remain; existing stl-core/settings warnings and two ignored tests remain.
- Unified frontend 44 tests, native workspace/default Clippy and app packaging passed.
  Locks add only existing time dependency edges; no dependency upgrade.
- Installed /Applications/GoggleLab.app signature and exact executable hash verified:
  c1ff4ace5775176e23191dff0d4b16359c57658aac447d1ffab3450a2ab2f9d8.
- App launched (PID 43676 at verification). Previous app preserved at
  output/app-backups/GoggleLab-before-phase3-20261010.app.
- Native CUA capture still returns cgWindowNotFound. Native window appearance is not
  asserted. Browser fixtures plus isolated real native/data fixtures remain the evidence.
- Windows/Linux runtime checks remain release gates. No push/release/provider action.
