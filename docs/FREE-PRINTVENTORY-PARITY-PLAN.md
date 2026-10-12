# GoggleLab Free: full Printventory parity

Date: 2026-10-10
Status: phases 1–5 implemented and locally built/verified; phase 6 in progress. Native window capture and cross-platform release verification remain outstanding.

## Decision

GoggleLab Free will include the complete Printventory capability baseline. Basic library management and slicer handoff become Community functionality. They must work in both independent Community builds and the unified official app without a Pro license. Pro retains editable/saved colorways and slicer palette access. Other paid differentiators require a separate product decision and cannot remove a parity capability from Free.

This supersedes the earlier product boundary that reserved all managed Library and slicer launch for Pro. Current README/architecture implementation descriptions remain accurate until phase 1 is integrated. One official customer app still starts Free and unlocks remaining Pro features through licensing.

## Evidence and baseline

Primary sources reviewed on 2026-10-10:
- https://printventory.com/
- https://printventory.com/public.version (returned 2.2.2)
- https://github.com/TechJeeper/Printventory/blob/main/README.md (documents 2.2.9)
- https://github.com/TechJeeper/Printventory/blob/main/GUIDE.md
- https://printventory.com/faq.html

The homepage, dynamic public version file, and repository documentation differ. Repository-documented functionality is part of the target, but is not represented here as runtime-verified stable behavior. Before each later milestone, pin the upstream commit/build and verify the relevant behavior. Parity is functional, not copying Printventory code or its UI. Browser-extension distribution/capabilities remain an investigation item; they must be resolved before declaring full parity.

Current GoggleLab baseline was checked against Community README, architecture, extension API and Pro library/frontend/native handlers. Existing Pro library is an imported managed catalog with folder filtering and text search, not a completed thumbnail catalog or metadata editor.

## Full Free scope and acceptance matrix

| Capability | Phase | Acceptance evidence |
| --- | --- | --- |
| Existing managed catalog, import, browse/search, folders, open model | 1 | Same schema-v1 data opens and mutates without any license; fresh Free profile imports and reopens STL/3MF |
| Basic native slicer handoff | 1 | Loaded original STL/3MF reaches a freshly detected supported slicer; missing/invalid/revoked license does not block it |
| Index existing directories in place, multiple library roots | 2 | Scan leaves originals in place; renamed, missing, changed and new files reconcile predictably |
| Startup/background/periodic scanning, configurable size limits | 2 | Incremental rescan and cancellation; defaults documented and large-file limits configurable |
| Thumbnail catalog, regeneration/purge, sorting and global search | 2 | Persistent thumbnails; sort by name/size/date; search and filters work across roots |
| Tags/categories, designer, notes, source URLs, model license | 3 | Editable persistent fields; CRUD/filtering and tag management; accessible keyboard workflow |
| Batch selection and metadata editing | 3 | Select visible/all-filtered scope is explicit; operation affects exactly selected models |
| Star ratings (0–5) and favorites | 3 | Rating/favorite persist across restarts and backup/restore; global filtering and individual/batch editing work |
| Print status and reprint history | 3 | Unprinted/Want/Queued/Printing/Printed/Failed; dated print records/notes; last/ever printed filters |
| Parent/child relationships; folder and ZIP bundle grouping | 3 | Related parts remain grouped; relationships survive restart and source reconciliation |
| Content-hash duplicate review and safe cleanup | 4 | Exact duplicates shown with previews; cleanup confirmed/recoverable; originals outside selected scope untouched |
| Database/catalog backup and restore | 4 | Restored metadata/identities and files verified; backup taken before destructive schema updates |
| Multipart/bundle 3D preview and slicer handoff | 4 | All supported parts visible and passed to slicer with literal arguments; no temporary-path lifetime failures |
| Random model selection (“Print Roulette”) | 4 | Random choice respects documented selection/filter scope |
| Filament catalog and pull-only Spoolman integration | 5 | Local assignment/filtering; repeatable pull without writing to Spoolman |
| AI tag suggestions and batch review | 5 | Provider configurable, including compatible endpoints; accept/reject and merge/replace behavior explicit; no GoggleLab subscription gate |
| Headless server, LAN browser access, Docker/NAS deployment | 6 | Same library functionality via browser; mounted storage and restart persistence tested; access controls and TLS deployment documented |
| MCP library integration | 6 | Versioned tool contract; search/details/metadata/tags/scanning/duplicates/filaments/print history/thumbnails covered against pinned baseline |
| Browser extension, if confirmed in baseline | 6 discovery gate | Verify distribution and supported actions; record concrete acceptance tests or primary-source evidence of absence |
| Windows/macOS/Linux delivery and usable responsive UI | Every phase | Native OS checks for implemented capabilities; no blanket parity claim from macOS-only tests |

No phase is a paid add-on. Provider API fees are separate from app entitlement; AI credentials remain user controlled. New server/MCP surfaces must be opt-in, with explicit local/remote trust boundaries; current desktop exposure must not increase implicitly.

## Phase 1: move existing basic library and handoff into Free

### Scope

Move existing implementations and wire them into the public core; preserve behavior and data, then make the unified app reuse them. Do not add schema fields, in-place indexing, thumbnail generation or new metadata features in this slice. Do not copy the Pro licensing system, palette implementation or editable colorways into Community.

### Source and license gate

The user explicitly approved moving the existing private library and slicer-handoff source into public Community under MPL-2.0 on 2026-10-10, after disclosure of the proprietary-source boundary. Licensing and colorways remain private. This resolves the automatic approval review source-transfer gate. No push/publication is authorized by this approval alone.

### Ordered implementation

1. Confirm clean/dirty state in both repositories; preserve unrelated Pro payment/runner documents. Use short-lived codex branches and targeted commits.
2. Move library-core into public crates/library-core, retain schema version 1 and existing IDs/blob layout; adjust stl-core dependency to the public workspace and authorized license metadata.
3. Add public library_commands with list/create/rename/move/delete-folder/import/open operations. Remove LicenseState and entitlement admission only; retain managed-relative-path validation.
4. Register public native commands and LibraryState in the core builder. Initialize lazily at app_data_dir/library, retaining library.sqlite3 and compatibility with existing official-app data.
5. Integrate public slicer command. Keep current-generation original-source admission and held locks, fresh supported-slicer discovery, bundle/application identity checks, literal argument vectors, source-format restrictions and error behavior.
6. Move LibraryPanel, IPC calls and required styles into Community. Free services must not depend on license fetch success or entitlement refresh. Both Community and official-app Free modes show working Library and handoff.
7. Update Pro composition to reuse core functionality. Remove duplicate private handlers/state and obsolete plugin command permissions. Keep colorways, palettes and license controls paid. Legacy signed license feature claims remain parse-compatible; they no longer decide access to Free functions.
8. Update public/private Cargo manifests, locks, frontend contracts/exports and pinned core submodule. Verify Community builds independently with no private dependency.
9. Update README, architecture, licensing/help and release notes to describe implemented behavior accurately. Do not claim full parity after this slice.
10. Run independent review and validation. Build the unified macOS app, preserve the previous installed app as a recoverable backup, install at its existing local location and launch for visible QA. Do not publish or deploy.

### Data and application identity

- Official app identifier remains com.sonicparke.stl-handoff; do not silently change user data/config roots.
- Separately branded Community releases may use a different identifier/root. Do not automatically migrate/copy user data between editions or merge libraries; document recovery/import separately.
- Preserve existing Library IDs, schema version, blob paths, source identity library:<model_id>, recovery exports and saved Pro colorways.
- Lazy opening must report corruption/newer-schema errors without resetting or replacing the database.
- Phase 1 must not change actual model files except through already explicit import/management actions.
- No tests against the user's live library; fixtures use isolated temporary roots.

### Validation gates

- Red/green tests demonstrate Free behavior, not merely changed feature flags.
- Native: fresh unlicensed folder/import/list/open; reopen pre-existing schema-v1 fixture unchanged; invalid stored paths and unsupported schema rejected.
- Handoff: missing/stale generation rejected before launch; source lock held through admission; unknown/mismatched slicer rejected; Unicode/spaces/metacharacters preserved as literal arguments; launch failure reported.
- Frontend: library/handoff work with absent, invalid, revoked and unavailable license state; license changes do not close/lock Free library; paid colorways/palette remain restricted.
- Community: formatting/lint/typecheck, frontend tests/build, Rust tests/clippy, standalone app build.
- Pro: same applicable checks against updated pinned core, remaining licensing tests and unified app build.
- Local runtime: Free Library visible/import/search/open; slicer handoff UI resolves supported current model; Pro controls remain locked without activation.
- Windows/Linux behavior and availability verified on those OSes before cross-platform release claims; existing unsupported paths documented.
- No push, release, provider changes, production keys or paid-service setup in this task.

## Findings and dispositions

| Finding | Severity | Evidence | Disposition / owner |
| --- | --- | --- | --- |
| Private source transfer needs informed approval | Resolved | Explicit informed user approval on 2026-10-10 | Selected library and handoff source authorized under MPL-2.0; no push |
| Free cannot depend on Pro library initialization | High | Pro library commands use LicenseState::admit_feature | Phase 1 native migration; native worker/coordinator |
| Entitlement updates could relock Free UI | High | Pro frontend supplies gated LibraryService and handoff entitlements | Phase 1 shared frontend + transition tests; frontend worker |
| Removing a license gate must not remove launch guards | High | with_original_source generation/current-model locks and allowlisted discovery | Retain and test; handoff worker |
| Duplicate registration/state across core and plugin | Medium | Existing Pro plugin owns library handlers/state | Remove private duplicates; coordinator |
| Existing managed import is not in-place catalog parity | Medium | Current Pro catalog implementation versus Printventory FAQ | Explicit phase 2 work; coordinator |
| Website/repo versions disagree | Medium | public.version 2.2.2 versus README 2.2.9 | Pin/recheck milestone baseline; coordinator |
| Browser extension capabilities not yet established | Medium | Insufficient primary-source evidence | Phase 6 discovery gate, blocks full-parity claim; coordinator |

## Execution record

2026-10-10:
- Product scope accepted from the user and previous comparison; ideation/office-hours are bypassed because direction and first slice are explicit. Remaining uncertainty is advanced-feature scope, recorded above.
- Model-router activated for independent frontend, library and slicer work. Three GPT-5.6 Sol workers selected: native data integrity and native process launch require frontier routing; shared frontend crosses core/Pro entitlement boundaries. No ensemble or downgrade.
- Tokensave unavailable; Semble CLI failed with exec format error. Direct source inspection used as fallback.
- Existing Pro untracked payment/runner planning files preserved.
- Source transfer blocked before integration. No code completion, test success, packaging or installation claims.
- Skill-update/build/release steps remain unexecuted while the source gate is blocked. No checkpoint commit yet; only planning and the unwired handoff draft are present.

- Informed source transfer approved; resumed implementation on codex/free-library-handoff in both repositories.
- Skill repository updates skipped: both installed skill source repositories contain unrelated local modifications; preserving them and using the already-read installed instructions.

## Phase 1 validation

- Public native workspace tests and clippy passed; existing unrelated compiler/clippy warnings remain.
- Public frontend formatting/lint/typecheck and 51 tests passed; production web build passed.
- Pro frontend checks and 44 tests passed against the updated core pin; final production web build passed.
- Focused native handoff tests (6) and generation/extension tests (5) passed using injected launch closures.
- Real schema-v1 STL blob/revision/folder/tag fixture and license-free reopen tests passed with identity, bytes and schema retained.
- Browser Library fixture passed import callback, metadata search, folder filtering, model-open callback and restored focus; at 480px viewport the dialog is 448px wide without horizontal overflow.
- Independent backend/wiring review found no blocking issue. Review-driven fixes cover symlink escapes, stale import status and cyclic legacy folder rendering.
- Existing missing/empty database initialization and orphan-blob crash cleanup remain follow-ups for phase 4; no new migration behavior was introduced.
- Standalone Community and unified macOS app packaging passed. Unified app signature verified and installed at /Applications/GoggleLab.app. No external release is authorized.

### Final local result

- Community implementation checkpoint: 731e723. Pro consumes the public core; duplicate private Library/handoff handlers, state, implementation and permissions were removed.
- Pro native workspace tests and clippy passed. A stale acceptance example was updated to test still-paid colorway admission. Existing compatibility claims remain parseable and legacy handoff admission is test-only.
- Lockfiles changed only for the library workspace/dependency move; no unrelated dependency upgrade retained.
- Full unified-app browser check with a deliberately unavailable license service passed: Unlock Pro stays visible, Library lists/imports via public commands, and Customize colors remains Pro. Synthetic native responses were used; no real user model or license data was changed by this check.
- Previous installed bundle preserved at /Users/bradmcalister/Documents/DEV/gogglelab/output/app-backups/GoggleLab-before-free-library-20261010.app.
- Installed unified executable SHA-256: 1adbe7fd65e75ae68c68ec4ce084e256272bc7503a466ce59fa6f6530820efa0.
- Installed process launched successfully; a native stack sample confirmed AppKit and WebKit event-loop activity. Computer-use returned cgWindowNotFound for both GoggleLab and Finder, so native window appearance/interactions were not verified. Browser UI checks do not substitute for that native check.
- Windows/Linux runtime verification, full parity phases 2–6, native real-slicer launch verification and publication are outstanding. Local handoff tests use injected launch closures rather than starting a user's slicer.
- Both repositories keep local codex/free-library-handoff work; nothing pushed. Unrelated Pro payment/runner planning files preserved.

## Phase 2 local completion

- Community commits bf140e8 / 0e806b0 implement in-place roots, safe reconciliation,
  cached thumbnails, view modes, bulk generation, configurable rendering and scanning.
- Community frontend: 61 tests; storage: 21; focused native: 23; recovery: 12;
  full workspace/default clippy and standalone/unified macOS builds passed.
- Unified workspace checkpoint c997102 pins 0e806b0 with only necessary png/base64
  dependency edges; no dependency-version upgrade.
- Browser catalog verification includes all controls, view modes, cache invalidation,
  source settings/dates, full thumbnail containment, desktop/480px rendering and zero errors.
- Installed unified phase-2 executable SHA256:
  4d5a6c573a91dfec51127742f75deae06e996c56a209e415eaf9b63a7cfa4e74.
- App is running. Previous bundle: output/app-backups/GoggleLab-before-phase2-20261010.app.
- Native capture was retried and still returns cgWindowNotFound. Actual native window
  appearance is not asserted. Windows/Linux runtime validation remains a release gate.
- Remaining phases 3–6 retain their full scope, plus pinned-guide file-management,
  archive indexing, thumbnail-image formats and browser-extension discovery requirements.


## Phase 3 local completion
- Public 33e1e8e / unified dacadd9 add Free organization, categorized tags, versioned
  batch edits, explicit filters, print history, parent relations and stable folder/ZIP bundles.
- Native ZIP tests include actual nested 3MF parsing/PNG generation and verified extraction.
  Original archives remain unchanged. Incomplete scans do not reconcile or advance dates.
- Final checks: 72 Community frontend tests, 111 native tests, 31 storage tests, 16 recovery
  tests; remaining workspace suites and default Clippy passed with pre-existing warnings
  and two ignored tests. Unified frontend 44 tests, native checks and packaging passed.
- Installed executable hash:
  c1ff4ace5775176e23191dff0d4b16359c57658aac447d1ffab3450a2ab2f9d8.
  Previous bundle: output/app-backups/GoggleLab-before-phase3-20261010.app.
- Native app launched; CUA capture remains unavailable. No native visual or cross-platform
  release claim. Remaining phases 4–6 retain full scope and remain entirely Free.

## Phase 4 local completion
- Public b28ff37 / unified a50835c deliver recoverable removals and source undo,
  verified backups/restore, duplicate review, stable multipart handoff, custom
  and embedded image previews, filtered roulette and remaining catalog filters.
- Final Community: frontend 80 tests/226 assertions; native 138 plus one ignored;
  core35/backup4/recovery16 and other suites pass. Both app builds and unified
  checks pass. Existing unrelated warnings and ignored tests remain documented.
- Installed SHA256 f98718830607da8941a837a9db47a56a4c65b96af798e09eed751fe6dc97706d,
  process observed; previous app backed up. Native visual/slicer/cross-platform
  runtime gates remain. Phase5 proceeds; all phase5–6 capabilities remain Free.

## Phase 5 local completion (2026-10-11)
- Public4a34597/unifieddfcd037 add Free filaments, assignments/filters/history pickers,
  complete pull-only Spoolman reconciliation, secure configurable AI adapters and
  explicit atomic reviewed tagging. Schema4 recovery/backups retain older schemas.
- Community86 frontend tests/241 assertions; native151 plus one ignored;
  core40/recovery17/backup4/network9 and other suites pass. Unified44 frontend tests,
  native checks and both app builds pass. Both dependency audits are clean.
- Installed SHA25650e778a4e3668a90bd09a8b868be611140819251457baed559530af517bd5328,
  process observed; previous app preserved. Native capture remains unavailable.
- AI category sharing is now a user-controlled local/provider setting, local by
  default, following the owner's October11 decision. Provider sharing occurs only
  on explicit generation; no provider calls occurred during development.
- Phase6 proceeds with this same sharing boundary and the reviewed engine/transport/
  capture contract. All functionality remains Free; no publication/live exposure.

### Phase6 implementation checkpoint scope

Shared runtime, authenticated HTTP/MCP, headless scans and thumbnails, geometry/3MF, bundle downloads, browser management, backups/restores, ratings/favorites, and the MV3 capture inbox are locally built and verified. Schema6 adds rating/favorite defaults with a migration backup. See the [Phase6 contract](PHASE-6-SERVER-MCP-CAPTURE.md) and [installation guide](SERVER-INSTALL.md). Final parity and app installation are not yet claimed: physical-source trash/move/purge/cleanup and enabling AI/Spoolman network calls await the owner’s explicit answers; release builds, the final Docker rebuild, and independent review remain.
