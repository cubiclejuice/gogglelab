# Phase 2: in-place indexing and thumbnail catalog

Date: 2026-10-10
Status: implementation and Community validation complete; unified build/install gate in progress. All functionality belongs to Community/Free.

## Outcome
Register multiple local or mounted-network model roots and browse a persistent thumbnail catalog without copying, moving or writing originals. Managed imports continue to work. UI exposes scan status/cancellation, periodic scan settings, configurable file limits, name/size/date sorting, global search, source/folder/missing filters and thumbnail regenerate/purge.

## Design and contract
- Schema v2 adds indexed source references and root settings. Original v1 tables, managed IDs, blobs/revisions, tags and colorway identities remain. Create a consistent SQLite backup before transactional migration; reject newer schemas and do not reset corrupt catalogs.
- Root paths are canonical existing directories. Indexed paths are relative under their root and checked for symlinks/containment whenever opened. Nested/duplicate roots are rejected to avoid ambiguous ownership.
- A source record stores size, modified time, format and content hash. Reconcile completed scans: add new paths, update changed hashes, preserve metadata/ID for an unambiguous rename, keep byte-identical different files as separate records and mark genuinely absent records missing.
- Cancellation, offline root or incomplete/failed enumeration never marks the root's records missing. Progress/errors are bounded; parsing runs outside the catalog mutex and async commands keep the UI responsive.
- Native renderer creates deterministic isometric 256px PNG thumbnails from parsed model geometry, with bounded work. Cache identity includes source hash and renderer version. Metadata changes do not invalidate geometry; source changes do. Generated files live under managed thumbnails storage.
- Free IPC: library_list_roots, library_add_root, library_update_root, library_remove_root, library_start_scan, library_scan_status, library_cancel_scan, library_get_thumbnail and library_purge_thumbnails, alongside existing managed commands.
- Default scan interval 60 minutes and source limit 50 MiB; supported configuration remains bounded by parser limits. Startup scans enabled roots, periodic scans use their saved settings. Removing a source changes catalog references only and requires explicit UI confirmation; originals stay untouched.
- Browser thumbnails load with bounded concurrency and stale-response checks. Gallery uses pagination/lazy image loading. Missing rows explain status and cannot open.
- Read-only recovery accepts v1 and v2 without migrating and includes indexed references/root settings, while retaining existing managed export validation.

## Implementation and ownership
1. index_storage: public library-core migration, roots/reconciliation/source resolution, thumbnail registry and temp-fixture tests.
2. index_native: native asynchronous scanner/lifecycle, current source resolution, thumbnail renderer/cache and commands/tests.
3. catalog_ui: public IPC, gallery/control UI, filters/sorting/stale state and frontend tests.
4. Coordinator: API reconciliation, native dependencies/registration, read-only recovery compatibility, docs, independent reviews, browser QA, build/package/install and core pin integration.

Model-router: Codex frontier workers for data migration and filesystem trust boundaries, and shared UI contract integration. No ensemble. Existing skill repos contain unrelated changes; using installed instructions. Tokensave absent; Semble failed with exec format error, so exact source inspection is the fallback. Existing approved product direction supplies ideation; plan review focuses on data integrity, scan completion semantics and bounded work.

## Gates
- New tests first or reproduction tests for changed behavior. Isolated fixture roots only.
- Persisted v1 migration retains model IDs/metadata/blob bytes; backup can reopen as v1; migration errors preserve original.
- In-place scan leaves source bytes/mtime unchanged and creates no managed original copy.
- Add/change/rename/missing/duplicate, offline/repermission/cancel and root removal tests.
- Source symlink/escape, malicious cached paths and oversized/truncated file tests.
- PNG decode verifies actual dimensions and geometry pixels; restart hits cache; content change regenerates; purge affects generated files only.
- UI search/filter/sort, missing state, root configuration, cancellation and stale response tests. Real browser rendered gallery, interactive scan/import and keyboard/narrow layout checks.
- Full Community + unified frontends, native tests/clippy and standalone/unified desktop packaging.
- Install and launch updated local macOS app, retain previous bundle, preserve live data/Pro functionality.
- Windows/Linux runtime checks remain required before cross-platform release claims. Native visual tooling limitation from phase1 must be rechecked.

## Finding ledger
- High: migration must back up a SQLite snapshot including WAL; VACUUM INTO/backup API before schema transaction.
- High: incomplete scan must never infer missing records; completion is explicit and gated.
- High: indexed records must never mutate originals; validated root + relative path at use.
- Medium: exact duplicates cannot collapse metadata/identity; rename requires unique absent candidate.
- Medium: thumbnail work/cache updates must be bounded and stale-proof.
- Medium: v1-only recovery would reject migrated catalogs; extend read-only reader in the same phase.

No phase 3 work starts until these phase 2 gates pass. Full parity phases 3–6 remain part of the active objective.

Upstream phase-2 baseline pinned: Printventory f551c4601c03072362b426c652b5dd37e4c55601 (main, 2026-10-10).

## Validation record
- Library storage: 13 tests covering migration snapshot, in-place reconciliation, duplicate/rename identity, cancellation and offline configuration.
- Native scanner/thumbnail focused suite: 17 tests, strict no-deps Clippy passed; full native workspace tests and default Clippy passed with unrelated existing warnings.
- Read-only recovery: 12 tests; schema-v1 exports remain intact and schema-v2 references/provenance are validated.
- Frontend: 59 tests; format/lint/typecheck and production web build passed.
- Browser QA: search/filter/sort, pagination, disabled missing rows, scan/cancel state, source config/date and focus restoration passed. Desktop cards ~218px with 128px contained images; 480px uses two columns with all header actions within bounds.
- Visually inspected native 256px geometry PNG and desktop/narrow catalog screenshots under output/.
- Review fixes: current-scan-only rename matching, offline cache use, source/hash races, bounded file/PNG reads, source-change UI cache invalidation, ns timestamp normalization and intrinsic grid sizing.
- Recovery exports indexed metadata/references only; original source-folder backup belongs to phase 4's explicit backup options.
- Standalone Community packaging passed; final UI repackaging and unified core pin/native checks/package/install remain underway.
