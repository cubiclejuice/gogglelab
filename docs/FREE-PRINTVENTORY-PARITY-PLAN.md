# GoggleLab Free: full Printventory parity

Date: 2026-10-15 (updated 2026-10-20)

## Current checkpoint: standard metadata retention (2026-10-20)

Following verified JSON checkpoint `490d2a1`, the runtime's five standard embedded
metadata fields use a fixed selector and allocate retained values only for empty
recognized destinations. Unknown and already-filled fields still undergo their
previous text decoding/validation. Matching aliases, first-nonempty-event behavior
and public strings remain unchanged. Each selected value uses a checked/fallible
exact reserve with the same observer before copying.

Nine focused tests pass, including a regression that previously copied 1,205
values while returning five, plus actual read-only proposal checks for loose and
archived Stored/Deflate models and terminal failure injection. Clippy introduces
no new diagnostics. Remaining verification for this unit is tracked in
`output/standard-fields-implementation-gates.json`; focused success alone does not
establish the broader workspace, combined app or packaged acceptance.

The five slots and existing 64 KiB XML text acquisition bound imply at most five
controlled retained field-value allocations and 320 KiB logical returned payload
per scan. SDK workspaces, core model data, catalog metadata, later clones and total
allocator/RAM use are separate. This adds no 16 MiB copy counter, new quota or
producer admission. Aggregate owned metadata/model storage and immutable
publication remain open.

## Bounded project JSON checkpoint `490d2a1` (2026-10-20)

Project filament metadata now uses a streaming visitor instead of a production
JSON DOM and whole auxiliary-array copies. An allocation-free scan precedes Serde,
using the existing 8 MiB project input limit and default 127-container boundary.
Only the first 256 positions per recognized field are retained. Labels keep the
existing trim-then-256-Unicode-scalar behavior; the entire document, including
ignored values and later array positions, is still validated. No smaller JSON
string/token limit or new supported-input policy is introduced.

Controlled prefix, label, joined-name and result growth is bounded and fallible.
Terminal causes use the same operation observer and propagate through the actual
cached-root parser before whole-build fallback. Parsing retained project text
adds no inflation or collected-input charge. The old DOM is a test-only oracle.

The original regression failed at 771 auxiliary normalizations against a 768-slot
bound; it now passes. All 223 core tests pass, with one existing ignored fixture,
including 18 new private/public JSON tests. Broader workspace, composed and hosted
verification is tracked in `output/project-json-implementation-gates.json` and
[PR #4](https://github.com/cubiclejuice/gogglelab/pull/4); core success alone does
not establish those gates. SDK scratch is a separate versioned profile estimate,
not recoverable SDK OOM, a hard RAM ceiling or producer Owned admission.

Aggregate owned metadata/capacity, 3MF input collections, producer admission and
immutable publication remain open. Current-source container acceptance, native
composition/release and the previously documented platform/hardware gates remain
separate. No cleanup or installed-app replacement is implied.

## STL output checkpoint `e2ed018` (2026-10-20)

Following shared-runtime checkpoint `743c863`, both STL formats now enforce the
existing two-million-triangle policy before output allocation/append. This closes
the exact-size binary bypass and the ASCII extra-facet growth. Binary/ASCII
recognition, malformed/nonfinite error priority and recorded nonfinite normals
remain compatible. Checked/fallible paired reservations share the existing
observer, while legacy triangle-limit error fields remain unchanged.

The test-first control recorded three intended failures and four passing controls
before implementation. The corrected core suite passes 205 tests plus one existing
ignored fixture, including 15 new focused admission/allocation checks. The full
workspace regression suite passes. See `output/stl-before-growth-implementation-gates.json`
and [PR #4](https://github.com/cubiclejuice/gogglelab/pull/4) for current verification.
This does not finish 3MF input collections, JSON/owned metadata, producer admission
or immutable publication. The JSON migration above follows this correction.

## Runtime context checkpoint `743c863` (2026-10-20)

Evidence in this section belongs to its named earlier sources.


The current runtime integration follows core checkpoint
`128da5ecc0c47bd2864d295f886187ee0844715a`. Shared contexts now span hidden
archived-source validation and subsequent geometry, previews, metadata, bundle and
native viewer loading. Plain source/readback files use exact before-growth
collection without spending ZIP inflation. Runtime image/metadata XML is guarded,
root metadata streams without a full XML buffer, and image inputs use a 16 MiB cap.
The existing bounded model-ZIP/typed error boundary remains. Package contexts share actual 512 MiB inflation,
100 MiB model-part input, relationships 64 KiB, settings/project 8 MiB each, and
16 MiB cumulative collected **INPUT** text. All four core XML paths use the opaque
guard: 64 KiB wire token, 4 KiB values, 128 attribute items, depth 128 and 256 KiB
live raw names. Ordinary malformed optional fallback and interrupted-read retry
remain supported. These are not bounds on every owned metadata String, JSON tree,
mesh/map allocation or whole-producer RAM.

Local full-workspace regressions pass: 190 core tests (one existing ignored),
366 runtime, 209 native and 70 server tests, with other suites. The reset control
fails when validation gets a fresh counter while keeping the same observer, then
passes after exact restoration. Independent collector/caller reviews are closed.
Current evidence is recorded in `output/runtime-context-implementation-gates.json`.
Earlier `128da5e` passed workspace Clippy, 12 Python notice tests and disposable
composed Rust checks; its composed Cargo inventory has 558 components and omits
12 local packages. Final unified packaging/notices/source-kit acceptance is separate.
Hosted checkpoint verification is tracked in [PR #4](https://github.com/cubiclejuice/gogglelab/pull/4);
check its actual status rather than treating local gates as hosted completion.

The actual macOS hosted Rust 1.99.1 SDK attribute-workspace estimate was re-reviewed:
16 KiB remains a versioned **profile estimate**, not a RAM hard cap or an
Owned permit. JSON/model/owned-metadata allocation bounds, defensible producer admission and
immutable publication are still **OPEN**. The two-slot/2 GiB producer proposal is
unproven and disabled. See [parser bounds](THUMBNAIL-PARSER-BOUNDS.md) and
[publication contract](THUMBNAIL-PUBLICATION-CONTRACT.md).

Full parity/release acceptance remains open: [Windows #2](https://github.com/cubiclejuice/gogglelab/issues/2)
retains the stable `431256a` UI and `9e480a7` checkpoint provenance; Bambu hardware/MQTT
stays deferred in [#3](https://github.com/cubiclejuice/gogglelab/issues/3). Current-source
native/composed acceptance, native Linux desktop delivery, official
main/private core-pin integration, release artifacts and final review are separate
remaining gates. No installed-app replacement or new cleanup is implied.

Current-source synthetic Linux/container acceptance now passes at `128da5e`:
loopback-only hardened server, REST/MCP, indexing, geometry leases, restart
persistence, and an authored 3MF's XML/geometry/default thumbnail. The actual
server image regenerates 550 Cargo and 37 npm notices and retains ZIP's exact MIT
text. Evidence: `output/container-acceptance-128da5e/README.md` and `checks.json`.
The fixture is stopped and preserved. This closes that server checkpoint's
container gate; later runtime sources and final unified/native composition require
their own acceptance. No cleanup occurred; free space was 11 GiB at that checkpoint
and 9.7 GiB at the later runtime build check (displayed `df` precision).


## Historical cleanup caller slice (2026-10-19)

The following native/container/package evidence belongs to its named earlier
sources; it is not fresh acceptance of checkpoint `128da5e`.

The shared cleanup coordinator is now wired into native, browser, REST and MCP
callers in Free. Cleanup reviews a bounded page of candidates, independently
reproduces supported raster previews from admitted current loose sources, and
requires a fresh explicit confirmation for each page. Results distinguish removed,
preserved, protected and partially reviewed files; continuing reviews another page
without automatically deleting it. Cancel/replacement/disconnect use a separate
owner-checked discard action. An unconfirmed release falls back to bounded expiry
and pruning; a failed or timed-out execution may have completed some work and must
not be retried with the same token.

Default-preview reset changes catalog metadata only and keeps every file. Removing
newly unused previews requires a separate fresh review. Custom and embedded
previews, unknown or unprovable files, referenced/actively read previews, and
indexed-cache source/archive-member copies remain protected. No historical holds
or original sources are swept. Durable guarded cleanup remains unsupported on
Windows; it must fail explicitly rather than fall back to pathname deletion. See
[the cleanup contract](THUMBNAIL-CLEANUP-CONTRACT.md) for bounds and authority.

Current delivery gate passes at `431256a22f277839072c511b44cab8e0158390d9`:
202 native tests and 365 frontend tests / 1,523 assertions pass, including a real
card-flow regression for reset entries being silently recreated. Passive native
catalog and duplicate-preview reads now use a read-only IPC; explicit generation
remains available. The prior full workspace gate includes 124 core, 311 runtime,
65 server and five server-binary tests; those backends are unchanged by this fix.
Workspace Clippy, the final affected native Clippy gate, formatting and the signed
Community build pass.

Isolated unlicensed native acceptance confirms Cancel preserves rows/files,
reset keeps both PNGs while clearing default rows, a fresh confirmed review removes
two independently reproduced previews (7,072 bytes), close/reopen does not
regenerate entries, and explicit generation restores exact PNG hashes. Source
hashes remain unchanged; staging is empty; the QA app was closed without installing
or using the normal profile. The composed app uses private revision `2755084` plus
the candidate public core in a disposable snapshot, not an official core-pin update.

Synthetic Linux container acceptance passes at backend source `dda35c8`: separate
reset/cancel/replay, custom fallback, explicit four-plus-one cleanup pages,
unknown/custom/source-copy preservation, exact regeneration, restart and original
hash checks. The native-only reader fix does not change those runtime/server
implementations. The container is stopped; test data and images remain preserved.
Windows GNU core/runtime compilation passes at that source; this is not Windows
execution. No full parity, Windows durable-cleanup, hardware or release claim is made.

The next publication foundation is now implemented but remains **unwired**:
a selected-row snapshot and pure stored-digest PNG validator; an Owned-only quota
owner that retains the existing shared reservation through its last owner; and a
raw cross-directory NoReplace helper, with synchronized caller behavior preserved.
Independent review found and corrected Windows-native path normalization; portable
Windows-mode tests cover the whole validator, unsafe aliases and device names.
The full workspace passed before that final normalization correction (202 native,
126 core, 327 runtime, 65 server and five binary tests); all 12 affected catalog
tests pass afterward. Workspace Clippy, formatting and the signed Community build
pass. This adds no producer/file-reader wiring, schema change or new deletion.

Next: enforce the reviewed [parser resource bounds](THUMBNAIL-PARSER-BOUNDS.md),
then derive a defensible shared producer allowance before immutable publication
and caller migration under the
[publication contract](THUMBNAIL-PUBLICATION-CONTRACT.md). Remaining platform,
composition and release gates still apply. Historical and unknown generated media are not
swept. Build cleanup performed in this slice: none; space recovered: zero.

### Earlier checkpoint (historical)

The phase-status paragraphs below retain earlier evidence. Their 326-test frontend
count and “latest” verification wording refer to that earlier checkpoint; the
current automated gate and remaining acceptance scope are stated above.

Open Phase 6 release gates: friend-run native Windows runtime/DACL/junction/replacement checks and final release review. Ordinary concurrent changes are guarded by pinned file/directory identities; hostile same-UID swap-and-restore windows are documented residuals outside the requested threat model. At the owner's direction, Phase 7 and subsequent phases proceed in order while native validation remains pending; no cross-platform parity claim is made until those checks pass.
Status (2026-10-19): local phases 1–10 include reviewed printer/parts/slicer JSON in Free and schema v11. Delivery audit exposed missing native stock IPC and browser fleet coverage; those backfills are pushed at `2a6847fa605422e122c5f370041a4baa99457579`. Complete isolated native slicer import/export/restart acceptance now passes. Browser/MCP discovery and selected-printer checks are implemented with separately admitted host policy, default-off multicast/broadcast permissions, one-use review, version checks, and cancellation/revocation guards. The latest full serial Rust workspace suite, Clippy, and 326-test frontend gate pass; the network slice also passes isolated native default-off inspection, synthetic browser acceptance, and a Community production build/signature check. Synthetic browser acceptance covers discovery, unapproved profile saving, approval, fake Moonraker/Carbon printing status, cancellation, and disconnect; no real printer request was made. Remaining gates include the official unified core pin/composition, remaining Linux desktop delivery, friend-run Windows checks, hardware acceptance, provenance-aware generated-media follow-ups, and final release review. Bambu MQTT remains deferred in issue #3. The [Windows handoff](WINDOWS-FRIEND-ACCEPTANCE.md) is embedded in issue #2 with exact source provenance. Verified prior candidates and active builds are preserved; no cleanup or installed-app replacement occurred. No full parity or Windows/hardware acceptance claim is made.

## Decision

GoggleLab Free will include the complete Printventory capability baseline. Basic library management and slicer handoff become Community functionality. They must work in both independent Community builds and the unified official app without a Pro license. Pro retains editable/saved colorways and slicer palette access. Other paid differentiators require a separate product decision and cannot remove a parity capability from Free.

This supersedes the earlier product boundary that reserved all managed Library and slicer launch for Pro. The Free core now exposes basic library management and slicer handoff without a Pro entitlement. One official customer app still starts Free and unlocks remaining Pro features through licensing.

## Evidence and baseline

Primary sources reviewed on 2026-10-10:
- https://printventory.com/
- https://printventory.com/public.version (returned 2.2.2)
- https://github.com/TechJeeper/Printventory/blob/main/README.md (documents 2.2.9)
- https://github.com/TechJeeper/Printventory/blob/main/GUIDE.md
- https://printventory.com/faq.html

The homepage, dynamic public version file, and repository documentation differ. Repository-documented functionality is part of the target, but is not represented here as runtime-verified stable behavior. Before each later milestone, pin the upstream commit/build and verify the relevant behavior. Parity is functional, not copying Printventory code or its UI. Browser-extension distribution/capabilities remain an investigation item; they must be resolved before declaring full parity.

Initial (2026-10-10) GoggleLab baseline was checked against Community README, architecture, extension API and Pro library/frontend/native handlers. Existing Pro library is an imported managed catalog with folder filtering and text search, not a completed thumbnail catalog or metadata editor.

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
| Browser extension confirmed in current upstream `main` | 6 | Editable page review, supported catalog routes, attributed downloads, and a reviewed safe inbox adapter; keep local extraction default and imports authenticated |
| Slicer profile management beyond basic handoff | 7 | Current-main parity baseline confirms detection and profile CRUD; implement and verify in Free |
| Printer fleet, discovery, status, maintenance logs and reminders | 8 | Current-main parity baseline confirms CRUD, discovery/status, and maintenance flows; implement and verify in Free |
| Parts stock and print-linked consumption | 9 | Current-main parity baseline confirms stock CRUD and per-copy transactional consumption; implement and verify in Free |
| Windows/macOS/Linux delivery and usable responsive UI | Every phase | Native OS checks for implemented capabilities; no blanket parity claim from macOS-only tests |

No phase is a paid add-on. Provider API fees are separate from app entitlement; AI credentials remain user controlled. New server/MCP surfaces must be opt-in, with explicit local/remote trust boundaries; current desktop exposure must not increase implicitly. Phases 7–9 are confirmed against Printventory main at `59171cb88a2d9160ed06931c01a29d41904fe552`; execute them sequentially while Phase 6's friend-run Windows validation and final release gate remain tracked.

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
| Current upstream browser extension has broader routes, editable metadata, and inbox behavior | Medium | Verified at commit `59171cb88a2d9160ed06931c01a29d41904fe552` | Phase 6 implementation and compatibility tests; do not trust upstream absolute file paths |

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

Shared runtime, authenticated HTTP/MCP, headless scans and thumbnails, geometry/3MF, bundle downloads, browser management, backups/restores, ratings/favorites, and the MV3 capture inbox are locally built and verified. Schema6 adds rating/favorite defaults with a migration backup. The MCP adapter declares all 49 pinned Printventory tool names alongside GoggleLab additions. The prior host-local fixture advertised 69 capability-authorized tools; that Docker image predates the latest BackupOnly isolation fix, direct `export_library` alias, external-integration opt-in, and desktop slicer tools. Authenticated desktop MCP exposes path-free `list_slicers` and multi-model `open_in_slicer`; headless MCP returns an explicit path-free client command with short-lived verified download leases. Search parity is at Free commit `79f02aa`; Pro core is pinned at that commit in parent commit `a07af08`. The unified ARM64 app and DMG pass strict code-signature and disk-image verification. The rebuilt Community app was launched under an isolated temporary HOME and visually verified against a two-model synthetic catalog. The refreshed ARM64 Docker image passed authenticated loopback scanning and search checks with a synthetic model; external provider tools, source-write tools, and desktop slicer tools were absent. See the [Phase 6 contract](PHASE-6-SERVER-MCP-CAPTURE.md) and [installation guide](SERVER-INSTALL.md). Final parity is not claimed: Phase 6 security and Windows runtime gates remain open, followed by Phases 7–9 and full export/import round-trip compatibility.

### Phase 6 owner follow-up (2026-10-13)

The owner approved guarded source Trash/undo and moves, including composite multi-model plans, and approved the synthetic loopback Docker fixture. Free MCP and REST expose `trash_file`, `undo_trash`, and `move_files` through explicit `confirm:true` one-use plans with writable-root admission, model versions, and source hashes. Batches cap at 100 targets and 1 GiB. Trash preserves one verified recovery copy per model; move verifies destination copies and commits catalog locations atomically before removing originals. A crash can leave duplicate copies, but not a catalog path pointing at an internal stage. Desktop MCP handoff streams verified model bytes to private staged files, caps batches at 256 MiB, retains at most 1 GiB, and prunes owned stages after 24 hours on later handoffs. Unix uses owner-only staging permissions. Windows stages under `LOCALAPPDATA` only after resolving it beneath `USERPROFILE`, rejects reparse points, and applies and verifies a protected DACL for the current user and SYSTEM. The GNU application check and Windows test-binary link pass; actual Windows junction and effective-DACL behavior remain unverified.

The owner also approved guarded `PurgeCatalog`, `CleanupOrphans`, and a synthetic Docker fixture. Purge is confirmed and token-bound, requires catalog/maintenance/backup capabilities plus an admitted `backupOnly` destination, verifies the backup restore and managed asset hashes, preserves root configuration and the prior managed directory, and leaves configured model files untouched. Its no-overwrite journal is atomically published and synced before the staging directory is created. Startup validates the Library root and exact staged/previous sibling names, removes journal-owned incomplete staging and unpublished journal temp files, and restores the prior catalog after either interrupted rename. Marker or journal cleanup errors keep the runtime in maintenance. Cleanup selects only unreferenced, hash-verified generated thumbnails and indexed-cache files, skips active content leases, then deletes candidates after rechecking their bytes and file identity. These generated artifacts are rebuildable, so cleanup does not create recoverable holds; original model files remain outside its scope. MCP reports completed file and byte counts. Purge remains unavailable on non-Unix hosts without durable directory synchronization.

Embedded 3MF proposals select the relationship-defined package root (with the standard root-path fallback) and can resolve a parent only to an existing, unique catalog model. Duplicate searches cap explicit and filtered scopes at 1,000 models. Explicit-root hash refresh starts a scan even when the catalog has no indexed rows yet. Subtree scans reconcile missing sources only inside the requested scope; legacy absolute paths under the root normalize for reconciliation and keep stable model IDs. The standalone server and Docker defaults remain read-only. Runtime/Tauri source mutations and startup purge/restore recovery use pinned handles and recoverable holds; native OS Trash is no longer used for model source changes. The catalog VFS now binds main/journal/WAL/SHM operations to the pinned catalog identity, and backup/recovery snapshots use pinned readers. Remaining same-user review covers configured-root admission and recovery-name replacement windows; hostile same-UID replacement-and-restore inside one filesystem callback is outside the requested model. ZIP backup/restore is the full-fidelity recovery path; the pinned Printventory source does not expose a JSON import tool or JSON schema. Native Windows checks and final release review remain open, so full parity is not yet claimed.

### Phase 6 pinned MCP behavior audit (2026-10-13)

Every pinned Printventory MCP tool name is present in GoggleLab's static inventory, but the local source audit found these remaining differences in behavior or request shape:

| Operation | Pinned Printventory behavior | Current GoggleLab behavior | Phase 6 follow-up |
| --- | --- | --- | --- |
| `search_models` | Designer, tags, file type, printed state, offset, directory, rating, and favorite filters | MCP now accepts `tags` (all supplied names, case-insensitive, max 100), `fileType` (case-insensitive extension; optional leading dot; matches parsed format, managed filename, indexed source path, and archive-backed models), upstream `directory` plus the existing `sourceDirectory` alias, and a bounded first-page `offset` up to 100,000. Directory filters require `rootId`, which resolves either an admitted handle ID or its catalog-root ID; unknown, ambiguous, and non-catalog roots are rejected. Offset pages return a revision-bound keyset cursor for stable continuation. The `search` alias, designer, print-state, rating, and favorite filters remain available. | Search request-shape parity is implemented. The pinned snapshot does not specify tag AND/OR behavior or normalization rules; GoggleLab uses all-tags matching and explicit case/extension normalization. Offset-mode page boundaries can shift if the catalog changes between separate offset requests; use the returned cursor for stable continuation. The current importer indexes STL and 3MF geometry; matching an OBJ, STEP, or ZIP source extension does not add geometry import support for those formats. |
| `remove_model` | One call can remove multiple models | The alias now accepts a bounded `targets[]` list of versioned model IDs, retains the legacy single-target form, and uses one recoverable removal snapshot. Stale/duplicate targets and batch restoration are tested in `1e80023`. | None for target cardinality; retain the stable-ID/version and recoverable-plan safeguards. |
| `trash_file`, `move_files` | Accept one or more model files | Both aliases retain their singleton forms and accept 1–100 unique targets. One principal-bound one-use plan snapshots each version/hash, checks admitted writable roots and a 1 GiB aggregate byte cap, and executes under one maintenance guard. Trash and move use atomic source holds with post-move identity/hash verification; no path-only OS Trash or permanent source unlink is used. Results include `sourceHolds` with opaque root IDs, relative paths, hold/journal names, byte counts, and hashes. Synthetic tests cover single/batch operations, interrupted quarantine undo, replacement before/after rename, rollback collisions, REST confirmation, and stale/hash conflicts. | Requests contain no caller filesystem paths. Unix mutations fail closed if handle-relative operations are unavailable; Windows remains disabled pending native runtime/DACL verification. Compatibility, archive, thumbnail, backup, and staging paths use pinned handles or identity checks; native Windows and final release checks remain open. |
| `open_in_slicer` | Accepts one or more models; server mode can issue a client command | Desktop MCP rejects unsupported slicers before issuing leases, preflights all versions, streams and verifies each source into owner-only stages, then launches once with literal paths. Batches cap at 100 models and 256 MiB; owned stages expire after 24 hours on a later handoff and total retained storage is capped at 1 GiB. Headless MCP returns stable IDs and short-lived authenticated content leases through the MCP response. | The pinned snapshot documents the client command but omits the server-side sender and acknowledgement implementation. GoggleLab returns the command in the MCP response instead of pushing an unsolicited WebSocket event; headless callers must choose a `slicerId`. Windows staging uses the per-user profile location and applies a protected DACL for the current user and SYSTEM; native verification of the effective ACL and reparse-point behavior remains open. |
| `pull_3mf_metadata` | Accepts multiple files and reads parent-model metadata | The alias accepts up to 100 stable model IDs with optional expected hashes and returns path-free per-model proposals. `EmbeddedMetadataFields` now includes `parentModel`; `apply_embedded_3mf_metadata` can explicitly select it and resolves the embedded value only to an active model ID or one unique active display name. Unknown and ambiguous names fail; source hash and optimistic metadata version checks remain required. | The pinned snapshot omits its implementation, so the custom parent-name-to-catalog-model mapping remains an explicit compatibility rule. |
| `update_models_batch` | Accepts a different metadata patch for each model, including tags and filament assignments | The shared-patch form remains. The bounded `updates[]` form applies scalar metadata, replacement tag names, and stable local `filamentIds` in one core transaction; each model's version advances once and stale/invalid entries roll back all rows and relationships. | GoggleLab uses stable string model/filament IDs and expected versions instead of Printventory's numeric IDs or file paths. Keep those safer identity rules and explicit replacement semantics. |
| Thumbnails | `set_thumbnail` replaces; `add_thumbnail` appends; reads include image data | `set_thumbnail` replaces custom gallery entries while `add_thumbnail` appends (`d8bb368`). `get_thumbnails` now adds short-lived authenticated media leases for callers with `MediaRead` (`8a54d92`). | The caller fetches bytes through the authenticated content route; keep capability intersection and lease expiry enforced. |
| Duplicates and hashes | Supports search, designer, tags, directory, file type, printed state, ZIP inclusion, and group limits; hash status/work accepts catalog filters | `find_duplicates` accepts the pinned filters, requires an admitted `rootId` with `directory`, excludes archive members unless `includeZip:true`, caps explicit and filtered scopes at 1,000 models and groups at 200. `get_hash_status` reports scoped stored-hash counts and does not claim current bytes still match. An explicit `rootId` starts a hash refresh scan even if that root has no catalog rows. | Filtered refresh scans the resolved root as a unit; other files there may be indexed too. Supply `rootId` when a filter spans several roots, and verify current sources before file-management decisions. |
| Missing thumbnails | Can exclude IDs that repeatedly fail rendering | `get_models_missing_thumbnails` now accepts up to 2,000 stable model IDs and filters them in SQL before keyset page limits (`d708972`). | None for exclusions; retain the bounded list and cursor behavior. |
| Folder scans | Can scan a selected folder | `scan_directory` accepts an admitted `rootId` plus an optional safe root-relative `directory`. Subdirectory scans reject absolute paths, parent traversal and symlinked components; they add/update files and mark absent sources missing only inside that subtree. Full-root reconciliation remains unchanged. Root-contained legacy absolute source paths normalize before matching and updates retain their IDs. | No arbitrary paths or new root permissions are accepted. |
| `check_files_exist` | Accepts file paths or omits them to scan a bounded part of the library | Accepts stable `modelIds`, or omits them to check up to 500 catalog models by default (2,000 maximum). `missingOnly` defaults on for the catalog scan. Results contain model IDs and source states, never host paths; each request uses a 256 MiB source-verification budget. | GoggleLab deliberately substitutes catalog IDs for caller paths and keeps the scan bounded. Use `limit` for the one-shot scan or provide smaller ID batches from a paged catalog query; one source larger than 256 MiB cannot be checked by this tool. |
| Library JSON import/export | MCP exposes export/backup; the app imports JSON with an implicit, partial `{models:[]}` contract | `export_library` writes bounded Printventory-style JSON under `backupOnly` with no-overwrite and unresolved-filament safeguards. The app now offers a separate reviewed importer that maps selected rows only to already-indexed sources; filename suggestions and `filePath` hints are reviewed in memory and never passed to the OS. Unsupported printer/part/slicer fields are reported, and ZIP backup/restore remains full-fidelity. | The importer is intentionally partial and does not provide full JSON round-trip compatibility. Keep paths opaque, reject ambiguous matches, and retain ZIP recovery as the authoritative backup format. |

These are contract gaps to resolve before claiming parity, not permission to weaken the current security boundaries. Continue to reject caller-supplied source paths, use stable model IDs and expected versions, constrain file access to admitted roots, and require explicit confirmation for destructive source actions.

### Phase 6 current-upstream parity follow-up (2026-10-13)

Current upstream verification (2026-10-14): GitHub branch metadata confirms main points to commit 59171cb88a2d9160ed06931c01a29d41904fe552, created 2026-10-13 20:47:22 UTC; package.json identifies it as 2.2.27. The website stable download page reports 2.1.0, while the README still says 2.2.9, so the repo code at the pinned commit is the current parity baseline. Its code confirms slicer profile add/edit/delete/save and automatic detection (https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/slicer.js, https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/slicer-detect.js); printer CRUD, discovery/live status/connection checks, maintenance logs and reminders (https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/printer-manager.js, https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/printer-discovery.js, https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/preload.js); and parts stock CRUD plus print-event consumption by per-copy quantities, with stock deduction inside the event transaction and restoration on event deletion (https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/parts-stock.js, https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/print-events.js, https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/print-events.test.js). These capabilities are confirmed for Phases 7–9; the owner directed local implementation to proceed while external Phase 6 acceptance remains open. The upstream has no published JSON export schema, so extension round-trip is handled separately in Phase 10.

The MCP `get_all_metadata` request now accepts no arguments and returns counted designer, parent-model, and license rows; `get_model` remains a separate ID-based operation. The regression covers empty and populated catalogs and a soft-deleted parent. Other safe-contract differences remain intentional where GoggleLab requires stable model IDs, expected versions, admitted roots, or one-use plans instead of caller paths.

The browser extension now reviews editable title, designer, license, source URL, and notes before queueing; supports the pinned localized/alternate routes; captures safe model downloads with referrer and tab-snapshot attribution; and imports only through the explicit authenticated GoggleLab flow. Local extraction is the default. Optional Printables/Bambu enrichment requires a settings toggle, optional permissions, and a separate Enrich click; requests omit credentials, reject redirects, and are bounded by one six-second deadline and 64 KiB response limit. Development tests use mocked APIs only.

Windows handoff now canonicalizes the profile paths, rejects reparse points throughout the private stage tree, applies and verifies protected DACLs for the current user and SYSTEM, and checks the owning SID. A failed first-time permission setup removes only its new empty directory, and traversal enforces the 16,384-entry limit before queueing discovered children. The Windows GNU application cross-check and test-binary link pass; Windows-only runtime and DACL behavior remain unverified. Redirected `LOCALAPPDATA` and token owners differing from the current user SID fail closed.

The compatibility importer is bounded and user-reviewed. It assigns rows only to indexed sources, uses upstream filePath only as an in-memory match hint, and requires an explicit per-row confirmation for every assigned target before applying supported local metadata/tags/parents/filaments. Skipped rows need no confirmation. It reports unsupported printers, parts, slicers, and fields. For this app's own JSON, a matching stable parent ID wins; a foreign or absent ID falls back only to a unique local parent display name; an ID equal to the selected child fails closed, and ambiguous names remain unassigned. Phase 6 acceptance covers the originally implemented fields. Phases 7–9 extend import/export with their added data; Phase 10 covers complete printer/parts/slicer JSON round-trip acceptance. ZIP backup remains the full-fidelity recovery format.

**Phase 6 verification before same-user mutation redesign (2026-10-14):** CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target bun run check passed, including frontend checks, both web builds, serial workspace tests, and Clippy. The focused importer suite passes 39 tests / 151 assertions, including per-row target confirmation gating. A pre-existing replacement regression covered a file swap before its identity check but did not close the check-to-unlink race; same-user replacement is now in scope, and the quarantine redesign must replace that mitigation and rerun the full gate. The Windows GNU workspace cargo check and GoggleLab Tauri test-binary link pass in an isolated MinGW container. The Linux ARM64 Debian package metadata and extracted-payload Xvfb launch pass; package SHA-256 af824ae223659ecde4b60bded38cd9edb31255d5141ce65e9a0ed2e108d13be5. Focused backup/runtime/desktop tests also pass; cargo fmt --all -- --check and git diff --check pass. The backup compatibility walker charges twice each relative asset path and separately bounds retained absolute directory paths. Its long-root low-budget regression passed and independent review resolved the P3 finding. The rebuilt ARM64 Community bundle com.cubiclejuice.gogglelab.community was built at /tmp/gogglelab-phase6-created-at-target; codesign --verify --deep --strict and hdiutil verify pass, embedded browser assets are present, and the executable is arm64. DMG SHA-256: b2efd8435f713eb16ac05bbbaa0356bdd29bcc9c77d632728122d3da2fd3ca77; executable SHA-256: 44e3b2d8273ffadc8e86ebb6c70dcdbe479ea157b636f0770a8a62a5f3efdf71. The app was launched under an isolated temporary HOME in the later visual QA checkpoint below.

The updated linux/arm64 image gogglelab-parity-server:phase6-20261014-path-budget has image ID sha256:c72dcfefc76047b96288511a25bce20857419e6ce73c341bf113638b614d541d. Its synthetic fixture ran as UID 10001 with a read-only container root, read-only model and secret mounts, and only 127.0.0.1:3746 published. Health returned 200, unauthenticated capabilities returned 401, authenticated capabilities returned 200, and a one-use REST backup plan produced an archive whose on-disk SHA-256 matched the runtime result (14ed5ce3edb08aba0b50d782f7ab1127b838aa0101368d76fb6ba9fa1f466c76). The archive was the only file in the backup volume. The task-owned container, data/backup/secret volumes, token, and temporary files were removed; existing STL Vault containers and volumes were untouched. The image remains cached locally and was not pushed.

The importer/exporter preserves supported status, rating, favorite, parent identity, tag categories, local filament identities, and print-history event multiplicity. Standard `tags` remains a list of names and `parentModel` remains a display name; GoggleLab adds stable parent-ID and tag-category extensions. The exported JSON includes `filePath` for Printventory compatibility, while import treats it only as an in-memory match hint. The JSON path is not described as path-free. Reviewed import now treats explicit null/empty nullable text as a clear, absent fields as no-ops, and accepts safe plain source-origin labels and well-formed, credential-free HTTP(S) URLs with case-insensitive schemes. `printers`, `parts`, and `slicers` collections remain unsupported; archived filaments stay excluded from current model assignments while exact archived identities are preserved in print history; valid event `created_at` is preserved through a dedicated local import command, ordinary Tauri, MCP, and HTTP create/update inputs cannot override it, omitted values are generated, and malformed values are skipped. Exact upstream acceptance of extensions is unverified because the pinned source has no formal JSON schema.

Free MCP/REST source operations use one-use plans with hash/version snapshots and pinned handles on supported Unix hosts. Native Tauri commands use handle-relative filesystem operations with separate local version, hash, and command-confirmation checks; they do not consume MCP/REST plan tokens. Source Trash and moves stage originals in journaled recoverable holds and verify identity/hash after the atomic rename; generated thumbnail/cache cleanup permanently removes only verified rebuildable artifacts. Results expose source-hold receipts only for original model sources. Windows source mutation fails closed until native runtime/DACL verification. Native Windows testing and final release review remain open.

The current-main audit confirms slicer profile management, printer discovery/maintenance, and parts stock/consumption in the same commit above. Phase 6 remains open for friend-run Windows runtime/DACL/junction/replacement checks and final release review. Full JSON round-trip acceptance across printer, part, and slicer data remains after Phases 7–9. No full Printventory parity or native Windows runtime claim is made.

### Phase 6 catalog identity continuation (2026-10-14)

`LibraryDb` now retains a catalog file handle, compares its device/inode with the visible `library.sqlite3` path, and on Unix checks SQLite's `SQLITE_FCNTL_HAS_MOVED` immediately after connection open and after schema initialization. A deterministic test swaps in a valid decoy only for SQLite's open, restores the pinned path, and confirms the decoy connection is rejected before schema work. A SQLite commit hook rechecks the visible main database identity and aborts a transaction when a replacement is present at commit. Runtime reads and writes also check identity before and after each callback. Server quarantine, undo, and move retain the opened catalog connection through maintenance instead of reopening the database by pathname. Desktop source Trash checks identity before and after source holds and reports the deterministic recovery location if a swap is detected.

Regression coverage includes replacement after request preflight, after the guarded connection is acquired, during SQLite open, and before desktop source holds. `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target RUST_TEST_THREADS=1 bun run check` exited 0: 194 frontend tests / 729 assertions, both web builds, serial workspace tests, and Clippy. `cargo fmt --all -- --check` and `git diff --check` pass; existing lint and Clippy warnings remain.

The rebuilt ARM64 Community DMG passes strict code-signature validation and `hdiutil verify`; DMG SHA-256 `035f1ff22d4a134056e8bf38c7d4a03344f146246f56b0280d49faf10ca1711c`, executable SHA-256 `861d7cec9051145241867ac84fcfa6966ea7f549d1cb233a97e8a33f91039364`. Launch Services ran it with a fresh temporary HOME; the app displayed the two synthetic models, opened `qa-triangle.stl`, and showed its dimensions and slicer handoff control. The isolated profile was removed afterward.

The rebuilt `linux/arm64` server image `gogglelab-parity-server:phase6-20261014-catalog-guard` has image ID `sha256:e8d80455069f8b944eb51b120fa2398363a3f5864f2f3a9d8c601ba7a49a5942`. Its synthetic fixture ran as UID 10001 with a read-only container root, dropped capabilities, read-only model/secret mounts, and only `127.0.0.1:3748` published. Health/auth checks returned 200/401/200, the authenticated catalog reported one available synthetic STL before and after container restart, and the source SHA-256 stayed `7e012a48af9a7ed38ad10aa4e904a7d1b57801a244fa65f23b2b40c991c94b75`. The exact task-owned container and data/secret volumes were removed; the image remains local and was not pushed.

The catalog VFS routes main database, rollback-journal, WAL, and SHM operations through an identity-derived alias. It adopts existing public sidecars only after identity validation, admits exact `VACUUM INTO` outputs, and keeps known SHM files when SQLite unmaps them so live connections cannot split across inodes. Backup and recovery readers use pinned catalog state; disposable restore validation checks an opened file without leaving a persistent alias. Core regressions cover commit replacement, public WAL adoption, parent-directory rename, exact output admission, and two live SHM connections. A narrow final check-to-unlink window remains for private sidecar holds, and the delegated VFS cannot expose the mapped SHM inode during `xShmMap`; those require a hostile same-UID replacement-and-restore, which is outside the owner's requested model. Ordinary concurrent replacements are checked. Recoverable holds apply only to original model source files; generated thumbnails and previews remain rebuildable. Phase 6 remains open for friend-run Windows build/runtime, DACL, junction, and replacement checks plus final release review; Phases 7–9 remain gated.


## Phase 7: slicer profile management

Printventory main at `59171cb88a2d9160ed06931c01a29d41904fe552` confirms slicer detection plus profile add/edit/delete and save operations. Current GoggleLab detects a fixed set of installed slicers and stores a default, but does not provide equivalent profile CRUD.

Implementation: match Printventory's editable profile name/path rows, add/remove, automatic detection suggestions, and saved default. Require local file-picker selection and explicit trust for custom applications; preserve strict identity checks for built-in detected apps and revalidate custom executable identity at launch. Never trust imported/model-provided executable paths. Build and verify Phase 7 before starting Phase 8; native Windows acceptance remains part of the final release gate.

### Phase 7 local implementation and verification (2026-10-14)

Free Settings now lists detected built-ins and saved custom profiles, supports profile rename/remove and nullable default selection, and presents bounded auto-detection results as suggestions. Every new or changed custom path must be selected again with the local picker and explicitly trusted before save. A name-only edit preserves the stored executable identity; a replacement cannot become trusted through an ordinary list save. Desktop, bundle, MCP single, and MCP batch launches recheck that the profile is still saved and its identity is current at the launch boundary. The identity uses file ID, size, modification time, and bundle ID; hostile same-user in-place rewriting followed by restoring metadata remains outside the agreed ordinary-concurrency model.

Verification passed: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target bun run check` (208 frontend tests / 794 assertions, typecheck, production web builds, serial workspace tests, and Clippy); the Tauri library suite passed 189/189 after final changes; `cargo fmt --all -- --check`, `git diff --check`, and `cargo clippy --locked -p gogglelab --all-targets` passed; and the release executable built successfully with bundling disabled. The Impeccable detector returned no findings for the changed settings surface. The known-good Community DMG and app executable remain unchanged at SHA-256 `0d23160050ca92cf39642b7d9f415705e17499ed5308a9c0ceef2e87615cc602` and `1eec8b5fb7226a7e9b2422af887e440c18a3e8140b17ab2d236526ba20874d38`. Native Windows slicer-profile checks remain pending with the friend-run final gate.

## Phase 8: printer fleet and maintenance (active)

Printventory main at `59171cb88a2d9160ed06931c01a29d41904fe552` confirms printer listing/discovery, status and connection tests, printer CRUD, maintenance logs, reminders, and completion actions. GoggleLab's current printer model serves bed-fit geometry and slicer-detected profiles only.

The pinned implementation recognizes Bambu Lab (SSDP plus LAN MQTT/TLS), Klipper/Moonraker, and PrusaLink. The original Elegoo Centauri Carbon additionally uses an explicit, one-shot SDCP status refresh when the user tests a saved printer. GoggleLab will not start discovery or status polling automatically. Discovery must show and bound its selected IPv4 scope; candidate devices remain suggestions until reviewed and saved. Connection tests run only for a selected saved printer. Credentials stay out of settings JSON, the catalog database, library backups, logs, and compatibility JSON.

Implementation sequence:

1. Add catalog schema v7 for stable-ID fleet printers, maintenance logs, and reminders. Idempotently migrate existing `{id,name,bed_mm}` manual settings printers into the backed-up catalog while preserving the active printer selection. Keep slicer-derived presets visible as read-only entries distinct from editable fleet records. Schema v8 adds maintenance-log metadata versions so edit/delete operations can reject stale writes.
2. Add validated printer CRUD, explicit default/active selection, stale/unknown/online/offline/auth-required status, and optional print-attempt printer attribution with a historical name snapshot. Deleting a printer must not erase the printer label from existing print history.
3. Add maintenance-log and reminder CRUD with UTC dates, bounded text/intervals, and one-transaction completion/rescheduling. Reminders are local-only and never initiate network requests or notifications.
4. Add user-started bounded discovery and selected-printer connection tests for the pinned protocols. Limit each scan to at most 1,022 requested private IPv4 hosts, cap concurrency and total time, support cancellation, and return sanitized candidates/status. Use ephemeral connection credentials only; never persist printer secrets.
5. Include printer, maintenance, reminder, and print-attribution rows in full backup/restore and the supported Printventory JSON import/export contract. External printer IDs remain untrusted mapping hints; imported endpoints and credentials are never connection targets.

Verification must cover v6→v7→v8 migration and idempotent settings migration, printer/maintenance CRUD and version conflicts, reminder boundaries and transactional completion, historical print attribution after deletion, backup/restore and printer/history JSON round-trip, disabled-by-default network behavior, private-range parsing/cancellation/time limits, and synthetic protocol fixtures. Network tests use only injected transports or loopback fixtures; never probe the live LAN.

### Phase 8 local implementation and verification (2026-10-15)

Print-history import now uses one atomic batch command: canonical event identities are compared as a multiset, local printer references resolve to stable IDs and historical nickname snapshots, pre-existing multiplicity is skipped, and any invalid later item rolls back the complete batch. Concurrent import, multiplicity, rollback, attribution, and race-winner skip behavior have regression coverage. The importer no longer relies on a client-side read-then-insert sequence.

Printventory JSON export is bounded to 25 MiB and serialized from paged catalog data under one database read snapshot. It preserves pretty-JSON bytes, creates the destination exclusively only after serialization succeeds, and verifies the opened destination identity and content fingerprint before reporting success. On write or sync failure, a partial newly-created file may remain; the returned error explains this. Independent security review found no remaining P2 exporter issue and tracks this partial-file behavior as a P3 follow-up.

Local verification passed: `bun run check:frontend` (265 tests, 1,022 assertions, formatting and typecheck), `bun run build:web`, `CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target cargo test --manifest-path Cargo.toml --workspace --locked --offline -- --test-threads=1`, `CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target cargo clippy --manifest-path Cargo.toml --workspace --all-targets --locked --offline`, `cargo fmt --all -- --check`, and `git diff --check`. The Rust workspace run passed 646 tests with one existing ignored STL fixture; Clippy completed with pre-existing warnings.

### Phase 8 schema-v8 maintenance concurrency and recovery verification (2026-10-15)

Schema v8 adds a positive `metadata_version` to maintenance logs and backfills existing v7 rows to version 1. Edit and delete compare the caller's expected version in the database write; deleting a printer also increments linked log versions as it clears their printer references. Tests cover stale update/delete, including after parent-printer deletion, v7 data/backup preservation, and reminder interval bounds at 1, 36,500, 0, and 36,501 days. Full SQLite backup and portable recovery manifests preserve a version-2 maintenance log, while recovery reads v7 logs as version 1.

The printer/history JSON contract now has backend import-to-export coverage for one printer, maintenance log, reminder, and printer-attributed print event. Frontend parser/review tests use the export's exact maintenance and reminder identifier keys and ensure the external printer reference reaches the reviewed import payload. The parser also retains compatibility with the longer legacy aliases. Synthetic Moonraker and PrusaLink discovery/status tests pass. Bambu still lacks MQTT application exchange and an ephemeral credential flow. The user selected interface multicast on 2026-10-15; discovery sends a low-TTL SSDP request on routes selected by the entered range and discards replies outside that range. Devices elsewhere on those interfaces may receive the request.

The most recent full serial workspace run passed 656 tests with one existing ignored STL fixture. After adding one final test-only certificate-time case, the full Tauri crate suite passed 231 tests. The frontend gate passed with 268 tests and 1,034 assertions. Existing warnings remain: the frontend Oxlint `unicorn(no-useless-spread)` warning, Vite bundle-size notices, Clippy warnings, and STL test-fixture dead-code warnings.

Phase 8 external acceptance remains open. Bambu has user-started, bounded SSDP candidate discovery and cryptographic CA-chain/IP-name verification, but real printer leaf-chain and serial identity behavior are unverified; MQTT status exchange and an ephemeral credential flow remain deferred until a Bambu printer is available. Friend-run Windows DACL, junction, replacement, and runtime checks remain pending. Isolated native visual QA and a Phase-8-specific packaged app launch remain open. Full JSON round-trip across printer, part, and slicer data is tracked as Phase 10. By the owner's direction, Phase 9 and later local work proceed while these external gates remain open; no full parity or cross-platform runtime claim is made.

### Phase 8 Bambu SSDP scope decision and implementation (2026-10-15)

The user selected interface multicast after disclosure that devices outside the entered IPv4 range may receive an SSDP request. Discovery sends one TTL-1 M-SEARCH to `239.255.255.250:2021` per routed interface, only after the explicit Discover action. It never fetches a response's `LOCATION`; each response must match the Bambu service target, and candidate source addresses are filtered to the entered private IPv4 scope before any TCP certificate check. Datagram size, count, cumulative bytes, interface count, request duration, cancellation, and total scan duration are bounded. Serials are not returned or persisted. UI copy explains that other devices on those interfaces may receive the request.

The user approved public research on Bambu's `8883/TCP` LAN MQTT port, `bblp` plus access-code authentication, and the `device/<serial>/report` topic. A later public-only query found community protocol research describing MQTT 3.1.1, but no official Bambu source confirmed it. The exact client-ID rules, CONNECT flags, keepalive, complete status payload schema, and wire-level printer publish QoS remain unresolved. Community evidence supports subscribing to the report topic at QoS 0; publishing `pushall` is outside the receive-only status scope. Do not guess missing wire details. Research is recorded in `.perplexity-research/2026-10-15-16-59-bambu-mqtt-wire-details.md` and `.perplexity-research/2026-10-15-17-03-elegoo-centauri-and-bambu-wire-details.md`. A single approved GitHub Contents API fetch verified the official `BambuStudio/resources/cert/printer.cer` bytes: 6,540 bytes, SHA-256 `36f2bcee347ec7adce719b5fd350099591a4d3d0ec4e039c7019890d78e152a0`, Git blob `6d8dbcb39bdc068c9adac668193fba6a6b85b1d5`. The asset is now vendored at `crates/library-runtime/resources/bambu/printer.cer` (moved from the original native resource location during shared transport extraction) with provenance in its adjacent README. Runtime code pins the bundle hash and each certificate fingerprint, adds only the three self-issued CAs as trust anchors, and supplies the two cross-signed CAs as candidate intermediates. Rustls `WebPkiServerVerifier` now checks chain signatures, validity time, the configured IP name, and TLS handshake signatures; issuer-name text alone is no longer accepted as trust. Tests use public-only, locally generated certificates to verify that a valid chain passes for its matching IP SAN and fails for a different IP, while an untrusted Bambu-named certificate is rejected. The tests prove verifier behavior, not compatibility with a real printer chain.

Real printer leaf chains, model/firmware CA coverage, and the relationship between SSDP USN, certificate SAN/SNI, and the expected serial remain unverified. The current check binds the TLS certificate to the configured IP SAN; it does not bind the certificate to an expected printer serial. The official bundle contains no leaf certificates, and community reports of serial-based SNI, alternate partial chains, or `BBL Device CA N7-V2` are not a Bambu specification. Real-device Bambu compatibility is not yet verified. No MQTT application message or credential path has been implemented. Research is recorded in `.perplexity-research/2026-10-15-16-10-bambu-mqtt-tls-trust.md`, `.perplexity-research/2026-10-15-16-13-bambu-studio-official-tls-sources.md`, `.perplexity-research/2026-10-15-16-20-bambu-printer-ca-bundle-verification.md`, and `.perplexity-research/2026-10-15-16-59-bambu-mqtt-wire-details.md`. No credentials were sent, and no live multicast or printer probe was run. The SSDP path remains bounded by a 64-KiB/256-datagram ingress budget across interfaces, filters out-of-scope sources before candidate storage, checks every selected address's OS route, rejects truncated/trailing data, and checks cancellation before sending. The ingress cap can still miss an in-range printer if out-of-range devices exhaust all 256 datagrams first. Full Phase 8 gates remain open.
Real printer leaf chains, model/firmware CA coverage, and the relationship between SSDP USN, certificate SAN/SNI, and the expected serial remain unverified. The current check binds the TLS certificate to the configured IP SAN; it does not bind the certificate to an expected printer serial. The official bundle contains no leaf certificates, and community reports of serial-based SNI, alternate partial chains, or `BBL Device CA N7-V2` are not a Bambu specification. Real-device Bambu compatibility is not yet verified. No MQTT application message or credential path has been implemented. Research is recorded in `.perplexity-research/2026-10-15-16-10-bambu-mqtt-tls-trust.md`, `.perplexity-research/2026-10-15-16-13-bambu-studio-official-tls-sources.md`, `.perplexity-research/2026-10-15-16-20-bambu-printer-ca-bundle-verification.md`, and `.perplexity-research/2026-10-15-16-59-bambu-mqtt-wire-details.md`. No credentials were sent, and no live multicast or printer probe was run. The SSDP path remains bounded by a 64-KiB/256-datagram ingress budget across interfaces, filters out-of-scope sources before candidate storage, checks every selected address's OS route, rejects truncated/trailing data, and checks cancellation before sending. The ingress cap can still miss an in-range printer if out-of-range devices exhaust all 256 datagrams first. Full Phase 8 gates remain open.

### Phase 8 Elegoo Centauri Carbon SDCP path (2026-10-15)

The one approved read-only probe tried fixed unauthenticated Moonraker paths on ports 7125 and 80: port 7125 refused the connection, while port 80 returned 404. This is inconclusive and does not establish device incompatibility. Public-only Perplexity research found community documentation for the original Carbon's SDCP v3 WebSocket endpoint `ws://<ip>:3030/websocket`, no authentication, and a `Cmd: 0` status refresh on `sdcp/request/{MainboardID}`. A documented acknowledgement uses `sdcp/response/{MainboardID}`, echoes `Data.RequestID`, and reports success as `Data.Data.Ack: 0`; status uses `sdcp/status/{MainboardID}`. The complete firmware-specific status schema and numeric job-state mapping remain unknown. The UI does not document a menu for MainboardID, but public documentation identifies the `M99999` UDP discovery response on port 3000 and `Data.MainboardID` as the protocol-backed source. Evidence is recorded in `.perplexity-research/2026-10-15-22-15-centauri-and-bambu-public-protocol-check.md`, `.perplexity-research/2026-10-15-22-15-centauri-sdcp-status-shape.md`, `.perplexity-research/2026-10-15-22-18-centauri-sdcp-source-audit.md`, and `.perplexity-research/2026-10-15-centauri-mainboard-id-location.md`.

The local implementation prompts for MainboardID only on the original Centauri Carbon connection-test flow, validates it before network I/O, and does not persist it. A separate `Find ID & test` action invokes a native OS confirmation dialog inside the Tauri command before sending one `M99999` UDP broadcast on the OS-selected local interface for the saved printer; a direct command invocation cannot skip that dialog. The dialog identifies the printer and saved IP. After confirmation, the command rereads the saved record and aborts before transport setup if it changed or was deleted. Other devices on that LAN may receive the broadcast, while only a reply from the saved printer IP with matching `Data.MainboardIP` is accepted. The ID is used only for that same one-shot test. The WebSocket side sends a fixed upgrade followed by a masked `Cmd: 0` request with empty data, and discloses that the ID travels over unencrypted LAN traffic. It accepts only a bounded `Ack: 0` acknowledgement correlated by topic, command, MainboardID, and `RequestID`; synthetic tests cover discovery parsing/scope, the handshake, request framing, correlation, wrong-ID rejection, slow-trickling peers, and cancellation. Frame reads recompute the shared deadline before each socket read and check it after the final byte. The status parser recognizes a bounded `sdcp/status/{MainboardID}` envelope with `Cmd: 0` and an object payload, but does not retain or map payload fields. After a separately correlated `Ack: 0`, the app reports only that a matching SDCP status event was observed during this explicit test. The protocol does not provide verified request correlation or freshness for the event, so the UI does not describe it as a response; job status remains `Unknown`. The 350 ms event wait starts at the first correlated acknowledgement and duplicate acknowledgements do not restart it. One user-approved live check received the matching discovery reply and correlated `Ack: 0`; no MainboardID or raw payload was retained. Firmware-specific job-state mapping and live status compatibility remain open.

Verification after the discovery and native-confirmation updates: the Tauri library suite passes 252 tests, the frontend gate passes 271 tests and 1,045 assertions, full-workspace Clippy and formatting pass, and both Vite bundles plus the Free desktop binary build successfully. These are synthetic/local checks; they do not establish live Centauri firmware compatibility.

The approved isolated visual launch has not been performed: the current app has no supported runtime override for its fixed macOS app-data/config paths, and Orca's required desktop runtime is unavailable (`open` timed out and status remained `not_running`). Do not launch the app against the normal user profile to bypass this gate; an isolated profile mechanism or a later working desktop runtime is required.

The owner does not currently have a Bambu printer and asked to defer the one-host TLS-only certificate probe until a test printer is available. Do not probe the LAN or repeat this approval request in the meantime. Bambu model/firmware identity remains an open Phase 8 gate, so Phase 9 stays queued.

After the verifier change, the full serial workspace suite passed 656 tests with one existing ignored STL fixture. The full Tauri crate suite then passed 231 tests, including a validity-time rejection case. Full workspace Clippy passed with existing warnings, and `cargo fmt --all -- --check` plus `git diff --check` passed. The Free desktop debug executable built successfully with `CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target`. Focused tests cover the pinned bundle, trusted direct and intermediate chains, IP-name mismatch, invalid time, and issuer-marker rejection. No live printer or Windows acceptance claim is made.

The focused Tauri Clippy check and full workspace Clippy check passed with existing warnings; `cargo fmt --all -- --check` and `git diff --check` passed. A Free desktop debug executable built successfully. Both Vite bundles also built successfully to `/tmp/gogglelab-phase8-web-check` because the regular `dist` and `dist-browser` paths were held open by PID 44771. Native visual QA could not start: `/opt/homebrew/bin/orca open --json` timed out waiting for an Orca desktop window, and a subsequent status check still reported `not_running`. No Windows or LAN acceptance claim is made.

An attempted direct Windows GNU cross-check stopped before Rust compilation because `x86_64-w64-mingw32-gcc` is not installed. A standalone GNU Windows executable exists from the 2026-10-14 source snapshot at `/tmp/gogglelab-phase6-windows-artifacts-20261014/output/gogglelab.exe`, SHA-256 `7a5406ec73a90b0a2db0d18858be17bb9b119e3a2cad3b28429067f847315b8c`; it is not an installer and predates the current uncommitted work. The matching friend handoff and acceptance checklist are `docs/PHASE-6-WINDOWS-TEST-HANDOFF.md` and `docs/WINDOWS-FRIEND-ACCEPTANCE.md`. The manual-dispatch Windows Community workflow builds unsigned NSIS/MSI installers and uploads them with build metadata and SHA-256 checksums for 14 days, but it has not been dispatched, so no current Windows installer exists and the friend-run acceptance gate remains open.

### Phase 8 JSON round-trip fixture and timestamp correction (2026-10-15)

A shared `tests/fixtures/printventory-printer-history.json` fixture now ties the Rust exporter contract to the frontend importer. The Rust runtime test projects actual printer, maintenance, reminder, and linked print-event output to this fixture; the frontend reads the same file and exercises parse, reviewed printer creation, local ID mapping, and history apply, including an assertion that the event `createdAt` reaches the import writer. This found that locally created print attempts store `created_at` as epoch seconds; the Printventory importer requires RFC3339. Both the value builder and streaming exporter now convert epoch-second values to RFC3339 and reject invalid timestamps. After this change, the full serial Rust workspace suite passed 655 tests with one ignored fixture, full workspace Clippy passed, and the frontend gate passed 268 tests with 1,034 assertions.

The timestamp audit also found that ordinary locally created maintenance logs and reminders store epoch-second timestamps. The importer requires RFC3339 for maintenance-log `created_at` and reminder `created_at`/`updated_at`, so both materialized and streaming JSON exporters now normalize those fields through the same checked formatter used for print events. Existing RFC3339 timestamps remain unchanged. The runtime regression test verifies all three fields in both exporter paths; the focused test, full serial Rust workspace suite, workspace Clippy, `cargo fmt --all -- --check`, and `git diff --check` pass. The updated Free desktop debug app also builds successfully with `CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target cargo build --manifest-path Cargo.toml -p gogglelab --locked --offline`.

### Phase 8 SDCP event-receipt continuation (2026-10-15)

The status flag is named `statusEventObserved` across the Rust response, IPC, and frontend. UI copy states only that an SDCP status event was observed during the explicit connection test and that job state remains unmapped. A synthetic acknowledgement-only case keeps the generic Connected message; a matching acknowledgement plus event shows the event copy without changing `jobStatus`. Duplicate acknowledgements cannot extend the event grace, and Pong writes after Ack use the remaining grace deadline. Independent review confirmed that the event itself is not request-correlated; this is surfaced as an observed-during-test signal rather than claimed as a fresh response. The review also noted that the 16-frame cap can miss a later status event after unrelated frames, while connection success still requires a correlated Ack, and that control-frame bytes are bounded by frame/size limits but are not fully charged to the aggregate-byte counter.

The focused SDCP suite passed 15 tests; the complete Tauri library suite passed 254 tests; `bun run check:frontend` passed 273 tests / 1,052 assertions, formatting, lint, and typecheck. `cargo fmt --all -- --check` passes. The app was not launched and no additional LAN request was made. Build storage inventory after verification: `target` 22 GiB, `/tmp/gogglelab-phase6-created-at-target` 39 GiB, `/tmp/gogglelab-phase8-web-check` 23 MiB, `dist` 22 MiB, and `dist-browser` 688 KiB; the Data volume had 22 GiB available after the desktop build. A fresh Cargo clean dry-run for the approved `target` dev-profile `gogglelab` scope reports 16,264 files / 11.7 GiB. No cleanup was performed because the immediate open-file check found PID 44771 (`com.apple`) holding handles under `target/debug`. The prior approval does not cover the protected release directory or either `/tmp` cache; no space was reclaimed.

### Phase 8 isolated visual-QA profile and dev launch (2026-10-15)

The debug-only `GOGGLELAB_PHASE8_VISUAL_QA=1` guard requires the separate bundle identifier `com.sonicparke.gogglelab.phase8-qa` before any app-data/config lookup. It suppresses HOME-based slicer/printer discovery, skips background scanning, and blocks printer discovery/status commands, transports, and socket entrypoints. The per-run Tauri config also sets the QA window to an incognito webview. The QA app-data directory was absent before launch; no source models or normal GoggleLab profile data were imported.

The first Tauri dev launch exposed a real debug startup issue: `resource_dir()` returned `unknown path` before the prior fallback could run. `server_web_root` now falls back to the workspace `dist-browser` only in debug; release builds still require packaged resources. The isolated app now remains running from `/tmp/gogglelab-phase6-created-at-target/debug/gogglelab` (82 MiB) and uses the distinct QA profile. `orca open --json` timed out and `orca status --json` remains `not_running`, so no screenshot or visual inspection is claimed. App launch and profile isolation are verified; visual QA remains open.

Verification after these changes: the complete Tauri library suite passed 260 tests, package Clippy passed with existing warnings, `cargo fmt --all -- --check` and `git diff --check` passed. Build storage inventory: `/tmp/gogglelab-phase6-created-at-target` is 40 GiB, `target/debug` 19 GiB, `target/release` 2.5 GiB, `dist` 22 MiB, and `dist-browser` 688 KiB; the Data volume has 21 GiB available. No cleanup was performed: PID 44771 still holds files under `target/debug`, and the QA app is using the `/tmp` target. No space was reclaimed.

## Phase 9: parts stock and print consumption

Printventory main at `59171cb88a2d9160ed06931c01a29d41904fe552` confirms parts-stock CRUD and print-event part associations with per-copy quantities. Stock deduction occurs in the print-event transaction and is restored when an event is deleted. GoggleLab print attempts currently track outcomes, notes, quantity, and filament IDs, but no stock parts.

Implementation: add Free parts inventory and print-event associations; make stock changes atomic and versioned across event create/edit/delete; preserve the data in backup and supported JSON fields; and test duplicate replay, conflicts, and rollback before moving on. Continue this local phase while hardware-dependent Phase 8 acceptance remains deferred by the user.

### Phase 9 baseline and readiness (2026-10-16)

The existing `PartSummary` is 3D geometry data, not stock inventory; the catalog has no stock-part records or print-event part links. The JSON exporter currently emits empty `parts` collections, the importer reports part usage as unsupported, and portable recovery is hand-built rather than a raw SQLite snapshot. The pinned Printventory implementation defines stock fields as name, category, quantity, unit, notes, and low-stock threshold. Event `parts` quantities are per-copy values multiplied by event copy count. Its tests confirm failed events consume stock too, event deletion restores stock while the part still exists, and deleting a part preserves the historical name without restoring stock. Evidence: [parts-stock.js](https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/parts-stock.js), [print-events.js](https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/print-events.js), and [print-events.test.js](https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/print-events.test.js). Phase 9 adds schema-v10 stock and adjustment-marker tables, versioned parts CRUD, per-event per-copy usage through desktop/browser and server contracts, transactional stock adjustment on event create/edit/delete, and backup/recovery/JSON preservation. Verification covers duplicate replay, concurrency, rollback, and unlinked historical snapshots; the printer/part/slicer round-trip is the Phase 10 gate.

Core slice (2026-10-16): schema v10 stores versioned stock parts, historical event links, and an event-level marker recording whether its usage changed stock. Local event create/edit/delete adjusts stock atomically, including failed and cancelled outcomes; idempotent batches recheck receipts under an immediate write transaction. Historical imports preserve snapshots without deducting stock, and later edits/deletion do not refund imported stock. Explicit orphaned part references keep their event snapshot without linking to a same-name replacement. Omitted part links remain attached on event edits, with totals recalculated when copy count changes. Part deletion retains the event ID and name snapshot without refunding replacement inventory. Core tests passed all 115 tests. REST/MCP, desktop/browser UI, JSON import/export, portable recovery, and backup work are complete locally; cross-collection slicer round-trip compatibility is tracked separately in Phase 10.

### Phase 9 local implementation and verification (2026-10-16)

Free now has versioned parts-stock create/edit/delete, low-stock display, per-copy event usage, and printed/failed/cancelled event support. Stock changes and event writes are atomic; stale versions map to conflicts; duplicate batches do not charge twice; edits preserve omitted event links while recomputing totals; deleted part snapshots remain on history; and deleting a stock-adjusted event refunds only stock records that still exist. Historical imports preserve snapshots without consuming stock or refunding on later deletion. An explicit missing part ID cannot be remapped to a same-name replacement; the event remains a bounded, unlinked historical snapshot. Desktop IPC, browser REST, authenticated REST, and MCP all expose the feature with capability and field bounds. Both Printventory JSON export paths include current stock and event usage; reviewed import supports bounded parts and stable/local reference mapping. Full SQLite backup and schema-v11 portable recovery preserve stock-adjustment markers and historical usage, including selected-model filtering. Follow-up review found v9 catalogs cannot distinguish native events from imported history even when an event has no part links. Schema migration and portable recovery now preserve unknown stock history for every pre-v10 event; adding or deleting usage on an imported v9 event does not change current stock. The complete serial workspace suite passed 716 tests with one existing ignored STL fixture; `bun run check:frontend` passed 294 tests. `cargo fmt --all -- --check` and `git diff --check` pass. After the final serializer correction and import-preview notice, `bun run build` completed both Vite builds and the optimized Rust release, produced `target/release/bundle/macos/GoggleLab.app`, and strict code-signature verification passed. No native visual inspection or installer launch was performed. Current storage inventory: `target` is 22 GiB, `/tmp/gogglelab-phase6-created-at-target` is 44 GiB, `/tmp/gogglelab-phase8-web-check` is 23 MiB, `dist` is 22 MiB, `dist-browser` is 696 KiB, and `/System/Volumes/Data` has 15 GiB available. A fresh `cargo clean --dry-run` could not read `target/debug/.cargo-lock` because the sandbox returned `Operation not permitted`; no cleanup ran. PID 44771 continues to hold regular files under `target/debug`, so the cleanup approval's idle condition is unmet and no disk space was reclaimed.

## Phase 10: complete printer, parts, and slicer JSON round-trip

Printer, maintenance/reminder, stock-part, and event-usage JSON now round-trip through bounded parsing, explicit review, apply, and export. A shared non-empty fixture covers stock plus event usage; historical import records those quantities without deducting stock. New printer profiles preserve exported creation/update timestamps, while mapped local profiles are not overwritten. `source_kind`, credentials, approval, and runtime status remain intentionally reset or excluded; imported endpoints stay unapproved. Maintenance/reminder entries with null, skipped, or unresolved printer refs remain as unlinked records with their nickname snapshot, including an empty snapshot. Schema v11 preserves those empty names and portable recovery supports the new schema. The value and streaming exporters now normalize printer timestamps to RFC3339 so ordinary local printer records can be re-imported. The user approved the direct pinned-source fetch on 2026-10-17; public `slicer.js` confirms saved `{name,path}` entries, and `slicer-detect.js` emits the same shape. Evidence: [slicer.js](https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/slicer.js) and [slicer-detect.js](https://github.com/TechJeeper/Printventory/blob/59171cb88a2d9160ed06931c01a29d41904fe552/slicer-detect.js). Desktop JSON now exports saved custom profile names and path hints through the same bounded value/stream contract; headless exports keep an empty list because they have no workstation profile settings. No executable identity, trust, default selection, or launch arguments are exported. The reviewed importer rejects malformed/oversized slicer arrays, strips unrecognized profile fields, and requires create/skip choices. Create requires a fresh local picker selection and explicit trust; the JSON path is text only and is never a picker default or OS lookup. The native command atomically appends to the freshest settings, rejects duplicate names/paths and changed existing executable identities, and preserves saved profiles and the local default. Imported historical event usage never deducts stock a second time. The shared populated printer/parts/slicer fixture proves stream/value export equality and reviewed frontend mapping, with local executable reselection intentionally replacing foreign path hints. Failure/retry coverage retains successful slicer batches without duplicate creation, and empty imports remain silent. Full external acceptance still depends on the deferred Bambu, friend-run Windows, and native visual/release gates; those must not be described as locally verified.

### Phase 10 local verification and external handoff (2026-10-17)

- `cargo test --workspace --locked --offline -- --test-threads=1` using the stable `/tmp/gogglelab-phase6-created-at-target`: 720 passed, one pre-existing ignored STL fixture; no failures. Populated printer/parts/slicer export equality, local trust, atomic append, and invalid-input checks are included.
- `bun run check:frontend`: 299 passed / 1,168 assertions, formatting, lint and typecheck pass. The existing browser-main spread warning remains. Workspace Clippy exits successfully with existing warnings; the new clone-to-slice test warning was corrected. Formatting and diff checks pass.
- `CARGO_NET_OFFLINE=true bun run build` exits successfully; strict deep code-signature verification passes for `target/release/bundle/macos/GoggleLab.app`. No installation or publication was performed. The protected known-good release in the separate target remains preserved.
- Native UI smoke check used the built-in computer-use tool, without Orca. The generic app selector initially opened installed GoggleLab; it was closed immediately without any data interaction. A uniquely named temporary QA bundle then launched with the exact QA identifier, incognito webview, and enforced isolation flag. Startup and recovery UI were seen; native chooser automation later returned `timeoutReached`, so import/trust/export visual acceptance is **not** claimed. The temporary QA app and its loopback development server were stopped. Synthetic fixtures and diagnostic files were retained.
- [Windows Community acceptance: Claude handoff](WINDOWS-FRIEND-ACCEPTANCE.md) supplies source/artifact identity, checksum checks, isolated MSI/NSIS lanes, Free features, DACL/junction/replacement/hold checks, stock/replay/recovery/migration/JSON checks, and an evidence/report template. The Windows workflow packages this plan; its exact seven build commands match. Native Windows execution remains for the friend.
- Post-build storage was rechecked. Cleanup remains prohibited while VM PID 44771 holds files under the previously approved `target/debug` scope (3,609 handles at the last check). No permanent cleanup occurred and **0 bytes were reclaimed**. Data-volume free space moved from 4.7 GiB before the production build to about 6.3 GiB afterward; that fluctuation is not cleanup recovery. Current working output, known-good release, and synthetic QA evidence remain preserved.

Remaining external gates: friend-run Windows acceptance, deferred Bambu hardware/MQTT verification, complete native import/profile visual QA, and final release review. No full Printventory parity claim is made.

Post-build inventory: repository `target` 22 GiB; stable `/tmp/gogglelab-phase6-created-at-target` 44 GiB; old `/tmp/gogglelab-phase8-web-check` 23 MiB; desktop/browser assets 22 MiB / 696 KiB; retained temporary QA bundle 83 MiB. The duplicate debug outputs are candidates only after fresh idle checks and the exact approved cleanup scope; the working output, known-good release, and QA evidence remain protected.

## Community enhancement approved 2026-10-19: Centauri Carbon job status

The owner explicitly requested original Elegoo Centauri Carbon job-state support in Community/Free, beyond the pinned Printventory baseline. This enhancement has no Pro entitlement. Bambu MQTT and real-printer identity acceptance are separately tracked in [issue #3](https://github.com/cubiclejuice/gogglelab/issues/3), deferred until a Bambu printer/tester is available; Windows acceptance remains [issue #2](https://github.com/cubiclejuice/gogglelab/issues/2).

Implementation is grounded in the official Elegoo SDK at commit `46c7b814e055cf9675d58482d79f43d0bd2280da`, [original Carbon message adapter](https://github.com/elegooofficial/elegoo-link/blob/46c7b814e055cf9675d58482d79f43d0bd2280da/src/lan/adapters/elegoo_fdm_cc/elegoo_fdm_cc_message_adapter.cpp). It accepts an exact-target `sdcp/status/<MainboardID>` envelope with top-level `MainboardID` and `Status`, then reads `Status.PrintInfo.Status`. The directly represented codes are 0 idle, 6 paused, 9 complete, and 13 printing. Other/transition/resin codes and malformed or missing values remain Unknown. Older community enum tables conflict with the SDK and are not combined with this mapping. The previous fabricated nested `Data.Cmd` status fixture is replaced with the SDK's envelope shape.

The operation remains a user-started, one-shot, read-only connection check with a correlated `Cmd: 0` acknowledgement required independently of an observed event. An event may be observed before or after that acknowledgement; it is not a request-correlated or guaranteed-fresh response. UI text describes a reported state observed during the check, and the existing timestamp/stale-snapshot copy applies. Mainboard IDs and raw payloads are not stored. No subscription, polling, controls, credentials, or automatic printer requests are added. Prior live-check approvals were one-shot and consumed; this implementation does not itself authorize another live request. Firmware compatibility remains unverified until a separately authorized real-device check.

Local verification (2026-10-19): the source-backed envelope regression failed before the fix. Focused SDCP tests pass 20/20 and printer-network tests pass 26/26. `bun run check:frontend` passes 300 tests / 1,182 assertions, formatting, lint and typecheck (one existing browser-main warning). The complete native Tauri suite passes 268/268 outside the restricted sandbox; its first sandboxed run passed 264 with four folder-event timeouts, and the same binary passed all eight folder-watcher cases outside it, confirming the environment restriction without code changes. `cargo clippy -p gogglelab --all-targets --locked --offline`, formatting and diff checks pass with existing warnings. `CARGO_NET_OFFLINE=true bun run build` produces the Community-feature app at `target/release/bundle/macos/GoggleLab.app`, and strict deep code-signature validation passes. Independent frontend/native review found no actionable issue. No live printer request was made and the app was not installed or launched against a user profile. Bambu stays deferred in #3; Centauri hardware validation and the earlier native import visual/release gates remain open.

Approved live attempt (2026-10-19): a temporary harness compiled the current native discovery and SDCP modules with the existing build dependencies. One approved discovery attempt timed out without an accepted target-matching reply; the WebSocket status request was not sent. Retained summary: discovery timeout, status request not sent, no acknowledgement/event/job-state result. No raw discovery/status payload or MainboardID was retained. No retry, control command, credential exchange, or settings change occurred. This one-shot approval is consumed, and the result does not establish that the printer is offline or validate live firmware/job-state compatibility.

### Delivery audit and parity backfill (2026-10-19)

Current implementation and delivery are distinct gates. The isolated native import review exposed missing `library_list_part_stock` IPC registration, which prevented every JSON preview from opening. All four stock commands now delegate through the existing guarded database operations and are registered in the native handler. A consumer/provider contract test compares literal frontend IPC calls with the real native registry. Manual native file selection now opens the synthetic slicer preview; local selection, explicit trust, apply, export, and restart acceptance passed below.

The browser lacked printer fleet management. The shared protocol/runtime and authenticated REST/MCP now expose profile, maintenance-log, and reminder operations with capability checks, decimal-string versions/counts, typed stale-version conflicts, and strict inputs that cannot inject approval, credentials, or status. The browser supports these operations and print-history printer attribution. Independent review found and resolved missing OpenAPI advertisement, timestamp precision loss during notes-only edits, and invalid completion of paused reminders.

Fresh browser acceptance against a separate synthetic loopback server confirmed profile create/edit, maintenance create/edit retaining its exact seconds/milliseconds, reminder create/pause/resume, and atomic completion with a maintenance record. All records and exact timestamps survived a controlled restart of that fixture server. No printer network request was made. The final frontend gate passed 310 tests / 1,223 assertions, browser production assets built, and the serial Rust workspace suite, workspace Clippy, formatting, and diff checks passed. Full parity is not established: browser discovery/connection checks still need a shared guarded implementation and acceptance.

Native slicer acceptance confirms the local selection step and disabled Apply until explicit trust. Applying the synthetic reviewed import created exactly one profile, preserved the unset default, and persisted the identity of the locally selected file rather than the injected JSON identity/path/arguments. The fixture was not launched. The user completed the native file pickers/save dialog where automation incorrectly exposed disabled buttons. The actual exported JSON contains exactly the slicer name and locally selected path, with no trust, identity, arguments, default, or injected path. After verified quit and relaunch of the isolated app, Settings shows that same profile as available/trusted and the default still unset. The isolated listener stayed stopped and external integration access stayed off. Both temporary QA apps/listeners were stopped after acceptance; fixture files and screenshot evidence are preserved under the task-owned output directory.

Storage audit found identical generated-thumbnail regeneration accumulating displaced files and replacement journals. A regression reproduced eight entries instead of one; the narrow fix renders and verifies identical output, then reuses it without replacement. All ten thumbnail tests pass, including preservation of unknown bytes and symlink rejection. This deletes nothing. Broader successful restore/purge journal retention and replacement of different generated output still need provenance-aware handling; unexpected or concurrently replaced bytes must remain protected. Existing holds are not authorized cleanup candidates merely because of their names.

Printer transport extraction moves the existing bounded implementations and all 70 synthetic tests into the shared runtime, retaining native confirmation, QA isolation, and conditional persistence. Two additional guard tests verify disabled access and revocation at transport boundaries; all 72 pass. Certificate/provenance/fixture bytes and dependency versions are unchanged. No remote network API is enabled by extraction. The final workspace rerun caught stale native QA-test module references; the corrected child-process test now exercises the actual native guard through both shared Carbon paths. All three QA tests, the complete serial Rust workspace suite, workspace Clippy, formatting, diff checks, and the 310-test frontend gate pass after that correction. Independent extraction review found no remaining actionable issue.

The next slice is specified in [the guarded printer-network browser plan](PRINTER-NETWORK-BROWSER-PLAN.md). Security review required a final policy-generation/cancellation check linearized with status persistence, explicit per-operation capability checks, and separate default-off Carbon broadcast permission. Those findings are incorporated in the plan before implementation.

The standalone Community production app built successfully with `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/gogglelab-phase6-created-at-target bun run build:release --bundles app`. Deep/strict code-signature verification passes and its identifier is `com.cubiclejuice.gogglelab.community`. Executable SHA-256: `5de31f30f77f8cad28b0621d2da9ea612d1dfb065fbc53039837ac74238c1e46`. This is a local ad-hoc-signed candidate, not the official unified download or a published/notarized release. Native visual acceptance used the isolated debug QA app; no production profile was launched or replaced.

Post-build storage inventory: the reused `/tmp/gogglelab-phase6-created-at-target` is 35 GiB, repository `target/debug` 19 GiB and `target/release` 2.6 GiB, desktop web output 22 MiB and browser output 712 KiB. Data-volume free space was 34 GiB before packaging and 29 GiB afterward; those are whole-volume observations, not isolated build-allocation measurements. No cleanup occurred: space recovered is zero. The existing signed repository app and new Community candidate are both preserved, along with the QA fixtures and audit/license artifacts.

Delivery follow-ups remain: update the official unified app's public-core pin and validate its Free/Pro composition; build the official unified and current non-macOS release artifacts; retain current-source Linux/container acceptance evidence and verify native Linux desktop delivery; and complete final release review. The supported unified core-pin update requires a commit published on public `main`; the current work remains on the feature branch, so production pin advancement needs that integration gate first. Windows issue #2 and deferred Bambu issue #3 remain external gates. The latest Carbon discovery timed out as recorded above. No build cleanup was performed or space reclaimed; protected builds and active targets remain intact.

### Guarded browser/MCP networking accepted locally (2026-10-19)

The implementation and evidence are recorded in [PRINTER-NETWORK-BROWSER-PLAN.md](PRINTER-NETWORK-BROWSER-PLAN.md). Independent reviews closed scope-narrowing revocation and stale native-settings authority races. Actual browser acceptance required explicit review/start, unapproved save, endpoint approval, and selected fake status checks; the owner completed browser confirmations that automation could not reach. Cancellation and disconnect/key clearing passed. Native isolated Settings shows the server, AI access, printer access, multicast, and Carbon broadcast off. Rebuild, full serial Rust tests, Clippy, formatting/diff checks, and 326 frontend tests / 1,287 assertions pass. The Community production executable SHA-256 is `cd4ffa8c7354dccbca6c4f448124898630d63631e403a776ef809c4a2c35752e`; deep/strict signing and `com.cubiclejuice.gogglelab.community` identity pass. No installed app was replaced.

The previous verified candidate is preserved at `output/build-preservation/community-2a6847f.app` (59 MiB), and the existing repository app is intact. Post-build inventory: reused target 39 GiB, repository debug 19 GiB, repository release 2.6 GiB. Data-volume free space measured 26 GiB before the final package work and 22 GiB afterward. No cleanup occurred: recovered space is zero. The immediate idle recheck found VM PID 44771 still holding 4,604 handles under the repository target, so the existing conditional cleanup approval cannot be exercised. No process was stopped to bypass that condition.

### Current-source Linux/container acceptance (2026-10-19)

After the owner started Docker Desktop normally, the engine responded. An immutable
Git archive of `444e3aebc9e7f1c4c1404f98f72c07eab960dc77` built the Linux ARM64
headless image successfully. Source archive SHA-256:
`eae544e82aa2c5ed7f6e8609802ffd913201a8502fbc14ab18c797ceeba2cbb3`.
The fixture published only `127.0.0.1:3848`, ran as UID/GID 10001 with a
read-only container root and source/config mounts, dropped all capabilities, and
used a dedicated synthetic catalog volume. Authentication and origin rejection,
source indexing and thumbnails, versioned fleet CRUD/stale conflicts, maintenance
and reminders, MCP initialization/tool listing/fleet reads, restart persistence,
and license notices all passed. Source bytes remained unchanged; AI integrations
and printer networking stayed off. Restart changes the pagination snapshot
revision, while model records and fleet data remain identical.

Evidence: `output/container-acceptance-444e3ae/checks.json`, before/after snapshots,
fixture script, and `output/source-snapshots/444e3ae-provenance.json`. The test
container was stopped; its dedicated synthetic volume/image and evidence were
preserved. Other running containers were untouched. No real printer/provider
request or normal library access occurred. This verifies the headless Linux
container, not native Linux desktop, Windows, or real hardware acceptance.
Data-volume free space measured 27 GiB after this build; no cleanup ran and no
space recovery is claimed. Generated transaction-metadata retention remains
a separate implementation/review gate.

The container fixture also identified a generated-source-copy retention bug in
default thumbnail indexing. A focused regression first reproduced four additional
staging hold artifacts, then passed after rendering directly from the already
verified source buffer. The thumbnail still persists; original source bytes and an
unrelated staging file remain unchanged, and the staging entry set does not grow.
This removes redundant materialization and deletes no existing hold. Different
generated cache replacements and geometry lease retirement still need independent
provenance-aware treatment; custom/uploaded previews remain outside that work.

Before the next package rebuild, the verified `444e3ae` Community app was preserved
at `output/build-preservation/community-444e3ae.app`. Its executable SHA-256 remains
`cd4ffa8c7354dccbca6c4f448124898630d63631e403a776ef809c4a2c35752e`, and strict deep
signature verification passes. The prior candidate and installed app remain intact.

### Generated transaction metadata checkpoint (2026-10-19)

Shared native/runtime receipts now retire verified generated purge/restore control
files through private atomic capture, with bounded claim recovery and scratch
cleanup before main finalization. Original descriptors remain authoritative.
Unknown bytes, original-source holds, previous Libraries, and existing historical
holds are preserved. Fourteen primitive fault/recovery tests and the caller
regressions pass, and independent review has no remaining actionable findings.
The final serial workspace suite, Clippy, formatting, and diff checks pass under
an isolated macOS temp parent. The Community production package builds and passes
deep strict signature verification (executable SHA-256
`202eecf5079d48cbc3d330f1f631251caffb079f12191b386a4b4acbb8454d31`).
See the generated-control metadata plan for concurrency residuals, the 4,096-entry
parent bound, and preserved logs from the two environment-dependent test attempts.
The updated isolated app launch and Linux image checks remain pending at this
checkpoint. Existing Linux container evidence is for the earlier `444e3ae` source.

### Verified metadata delivery and approved cleanup (2026-10-19)

Code checkpoint `8ea0a4ed3fb7a4d415d4927dd7efa60bf97c57a7` is pushed. Its production
Community app passes strict signature verification, and the rebuilt isolated debug
app opens its Library and exits under the dedicated QA identifier/guard. Screenshot:
`output/generated-metadata-isolated-library.png`. The updated immutable Linux
ARM64 image passes the synthetic authenticated REST/MCP, read-only source indexing,
no staging holds, fleet/conflict/reminder, purge/restore, and restart checks. Parent
journals are retired; both previous Library directories and restore's previous
catalog ZIP are preserved. Source hashes remain unchanged. Evidence:
`output/container-acceptance-8ea0a4e/checks.json` and source archive provenance.
The fixture container and isolated app are stopped. Windows issue #2 contains the
complete current handoff and exact `8ea0a4e` source identity, verified by read-back.

PID 44771 exited. A fresh full target open-file scan and process inspection found
no build/app activity; the exact approved dry-run still reported 16,264 files /
11.7 GiB, all selected paths inside repository `target/debug`, with no app bundles
or tracked source. The owner's one-time conditional command then ran successfully:
`cargo clean --manifest-path /Users/bradmcalister/Documents/DEV/gogglelab/Cargo.toml --target-dir /Users/bradmcalister/Documents/DEV/gogglelab/target --profile dev --package gogglelab`.
Cargo removed that exact logical scope. Data-volume free space increased from
23,154,774,016 to 28,407,513,088 bytes: **5,252,739,072 bytes (4.892 GiB)** measured
recovery. Logical output size differs from physical APFS space. Repository debug is
now 14 GiB; repository release remains 2.6 GiB, the reused working target 40 GiB,
and preserved app candidates 118 MiB. Working/known-good executable hashes and deep
strict signatures, plus the repository release signature, still verify. No source,
Git state, configuration, user data, Docker data, or historical source holds were
cleaned. This exact cleanup approval is consumed; future scopes need fresh approval.

Remaining delivery gates: official unified core integration/pin and composition,
native Linux desktop and friend-run Windows acceptance, deferred Bambu MQTT/real
hardware checks, provenance-aware different generated-thumbnail/geometry retirement,
and final release review. The last Carbon discovery timed out; synthetic printing
status does not establish real hardware acceptance. Full parity is not claimed.

### Isolated development composition checkpoint (2026-10-19)

A mode-0700 source snapshot combines tracked private commit
`27550843d4e06c0684c00cef86bd767cd366ae86` with public `bfbc5ae`. The live Pro
checkout, recorded `f733c1288ec3bc72f4bdd747a4ced7b7dfae5759` core pin, unrelated
work, installed app and profiles remain unchanged. The snapshot uses physical
clones of dependency trees and verifies that workspace imports resolve inside
that snapshot. No setup/submodule/pin updater or normal-profile launch was run.

The original Pro lockfile fails `--locked` with the newer core. Reconciliation in
the isolated snapshot adds 12 dependency-edge lines, with no registry version or
checksum changes. Missing existing checksum-pinned public crates were fetched for
local compilation/license metadata only; no source or credentials were uploaded.
With that proposed lockfile, metadata validation, 44 frontend tests plus format,
lint, typecheck and both web builds, 67 private Rust/license/issuer tests, and
workspace Clippy pass. Private frontend tests include failure/revocation retaining
Free Library and slicer handoff. A read-only reachability check finds these paths
use public core commands, without private license admission. Legacy handoff
admission is test-only; colorways/palette remain paid.

This is development composition evidence, not official delivery acceptance.
Packaging and actual unified-app Free-profile interaction remain pending, followed
by the real supported main/pin integration and release checks. Local source/lock
provenance and logs are retained under the dedicated private temp snapshot, not
committed as private code in the public repository. The next media slice will
require composition refresh against its own final public source.

### Anonymous geometry code checkpoint (2026-10-19)

Runtime geometry now writes bounded STL2 packets only through already anonymous
owned descriptors. Arc-backed readers retain physical byte charges through their
last close; no packet bytes receive a filename or become source holds. Unknown
legacy geometry remains preserved and counted. Capability/owner/epoch/I/O admission,
original source/upload/archive behavior, and native in-memory geometry remain.

True RED/GREEN and independent review closed premature mesh allocation, concurrent
empty-file inventory, unrelated thumbnail admission, and expired-token quota
starvation findings. The final serial workspace suite passes, including 201 native,
120 catalog-core, 277 runtime and 62 server tests. Final workspace Clippy (existing
warnings), Rust formatting and diff checks pass. The optimized Community app builds
and passes deep strict signature verification; executable SHA-256 `aa495584633fd3a9e33beab7cdd6e2ec7de8cbc66c60e51edef6d92a10d902fe`.
The host Windows cross-check stopped before Rust validation because MinGW is not
installed; native Windows acceptance remains open.

Current-source Linux packet streaming/restart and refreshed unified development
composition/native smoke checks remain for this checkpoint. Content-addressed
thumbnail publication is intentionally held until independent orphan reproducibility
proof is implemented; unknown/custom/legacy bytes must stay protected. See
`docs/GENERATED-MEDIA-RETENTION-PLAN.md`. The verified prior Community build is
preserved at `output/build-preservation/community-8ea0a4e.app`; no additional cleanup
or installed-app replacement occurred.

### Geometry live checks and Windows portability (2026-10-19)

Code `933172238c68a809e130ce135157363795c465d9` passes the immutable
Linux/container packet lifecycle check: authenticated STL2 v2 geometry reads while
no staging file exists, repeat reads, stale-token rejection after restart, a new
identical packet and unchanged model-source hash. The synthetic container is
stopped and its dedicated volume/evidence remain preserved. No provider/printer
request or original-file deletion was performed.

Follow-up code `60e1630a7ae6a60bc2fd76ba87d117fdc901c41f` fixes real stable-Rust
Windows compile errors found by an isolated MinGW cross-compiler container. The
non-Unix inventory method still returns Unsupported; JSON export uses the existing
stable pinned-handle identity checks and fingerprints only a fresh verification
handle. Equal-byte replacement and writer-cursor preservation regression passes,
as do all 278 runtime tests, affected Clippy and formatting. Windows runtime and
test-target compilation passes; this does not establish MSVC/native runtime,
installer, DACL or guarded-mutation acceptance.

The development composition was refreshed to public `9331722` with the same
private `2755084` snapshot and isolated reconciled lockfile. Private Rust tests
and Clippy pass, and the separately named/signed debug QA app builds and launches
with no license in the isolated profile. Native Free stock creation/read-back,
in-place synthetic indexing and a generated preview work. Actual UI testing found
fast-scan catalog refresh and native collection-prompt defects; these remain open
until corrected and the actual app is rebuilt/retested. Slicer execution was not
performed in this session. Official main/core-pin integration remains separate.

Source-specific logs, hashes, signatures and native screenshots are retained under
`output/` and the private mode-0700 composition snapshot; no private source is
committed publicly. No additional build cleanup or installed-app replacement was
performed. Reused debug output has grown during composition builds; preserve the
current build and release and require fresh exact approval before any cleanup.

Known native-dialog follow-ups discovered during this checkpoint: catalog purge
uses `window.prompt` for its exact `PURGE` acknowledgement; manual Centauri
MainboardID entry does likewise. Replace these with visible in-app dialogs before
claiming their native workflow acceptance. Purge must retain exact token, backup
destination, cancellation/one-pending-action guards and existing verified-backup/
previous-Library protocol; it must not become thumbnail/orphan deletion. Manual
MainboardID stays ephemeral, strictly validated and unsaved; explicit submission
may call only the existing read-only status transport without discovery, after
revalidating the approved endpoint/profile. No new live printer check is approved.

### Native Library UI follow-up (2026-10-19)

Fast scan/thumbnail completions now invalidate catalog data even if the entire
job finishes between status polls or shares the previous job's timestamp second.
Successful starts invalidate older in-flight replies, and Library replacement
invalidates old polling/completion state. Collection create/rename use visible
labelled in-app dialogs, with validation, pending/error/cancel handling and Library
search focus fallback after a refresh removes the invoking button. True failing
regressions cover these timing/focus cases before the fixes.

The corrected frontend command uses explicit `./src` and build-argument test paths
so Bun cannot count archived source snapshots. All 334 tests / 1,322 assertions
across 38 current-worktree files pass, along with format, lint and typecheck
(existing warning only). Actual rebuilt combined-app QA remains the next gate.

### Native delivery checkpoint at `0ffc262` (2026-10-19)

The exact public code commit is
`0ffc262dff1b425f0aa880cd6d112d0800a3c679`. Both production web bundles and
the Community app build; the app's deep strict signature verifies, with executable
SHA-256 `3260384543ef9854c0e600256966e3b85e2279b0fc0b6034e2e6a84ec6b772fd`.
The prior `9331722` working Community app was physically cloned and its hash and
signature reverified before the new build.

The isolated development composition is refreshed to this public code, keeping
private `2755084`, the proposed dependency-edge-only lock reconciliation and the
live official core pin unchanged. Its 44 frontend tests, 67 private Rust/license/
issuer tests, Clippy, web builds and separately named/signed QA app all pass.
Actual native interaction without a license verifies:

- A fast scan changes the visible catalog from one to two models and updates the
  source scan time without closing/reopening Library. Both generated cards appear.
- Collection create/rename persist; invalid names stay in the dialog and Cancel
  performs no create. A keyboard-invoked save returns focus to Library search after
  refresh removes the old button. Mouse/AX invocation can legitimately restore the
  still-connected Library container. No speculative focus-timing patch was added.
- Regenerate all finishes at 2/2 and reenables controls. Staging has zero files
  before and after this same-output regeneration. Different-output/unknown PNG
  retirement still needs the independent proof work below.
- A freshly created harmless local slicer is selected with the normal picker,
  explicitly trusted and saved in the QA profile, temporarily selected as default,
  and invoked through the normal handoff shortcut. It receives exactly one model
  path whose bytes match the unchanged synthetic source SHA-256. The original
  `No default slicer` selection is restored and verified. No real slicer, printer
  or provider is contacted.

The first file-picker automation attempt timed out before opening the dialog;
observation recovered, and the full picker interaction completed without manual
input. The isolated app is now confirmed stopped. Read-back assertions and the
native screenshot are in `output/composition-0ffc262-native-checks.json` and
`output/composition-0ffc262-free-library.png`; private snapshot provenance stays
out of public source. The normal profile, installed app, Pro checkout and existing
uncommitted work remain untouched.

Windows issue #2 now embeds the complete revised Markdown plan with exact
`0ffc262` code provenance, including fast scan and native collection cases; issue
body equality was verified by read-back. Windows runtime/installer acceptance is
still open. Draft PR #4 remains unmerged. Remaining local work includes independent
PNG reproducibility/retirement proof and the two native prompt follow-ups above;
official main/core-pin delivery, native Linux desktop, hardware and final release
checks also remain. Full parity/release acceptance is not claimed.

No further permanent build cleanup was performed: **0 additional bytes recovered**.
The reused target and preserved working/release candidates remain. Data-volume
free space was about 14 GiB after these builds; any future exact cleanup scope needs
a fresh idle/open-file check, dry-run and explicit approval.

### Next native prompt slice: implementation contract

Replace the two remaining `window.prompt` dependencies without changing backend
authority. Catalog purge gets a labelled in-app confirmation with the exact
case-sensitive `PURGE` token and chosen backup destination, Cancel/Escape no-op,
one pending mutation, stale pick/dialog invalidation, inline errors and focus
restoration. Keep verified backup, maintenance, previous-Library and recovery
requirements; do not change orphan/thumbnail deletion.

The original Centauri Carbon's manual MainboardID dialog reuses fleet modal
helpers. Keep the approved endpoint/current-profile checks, explicit one-time
unencrypted-LAN/unsaved-ID disclosure and trimmed `[A-Za-z0-9._-]{1,128}` validation.
Only explicit submit invokes the existing status transport without discovery;
never persist/log the ID. Close/reopen/profile/endpoint changes invalidate pending
UI work. QA uses synthetic/fake services and the native QA network-denial guard;
previous one-shot printer checks are consumed and no new live call is authorized.

Parallel ownership is limited to management/confirmation UI and fleet/manual-ID
UI respectively. Require behavior regressions, independent review, one final
current-worktree frontend gate, build, and actual isolated dialog acceptance
before advancing to thumbnail/orphan proof implementation.


### Native prompt acceptance at `9f64119` (2026-10-19)

Exact tested code: `9f64119c634dd062eb3f31c7b3a1d829149b9b6c`.
This completes the native prompt slice above. Catalog purge now uses an in-app
confirmation with the exact case-sensitive token and selected backup destination.
Actual isolated native QA rejects lowercase input, cancels without mutation, then
completes an explicitly confirmed synthetic purge. The verified backup and previous
Library each retain both indexed models; both original STL hashes remain unchanged.
Purge code was tested at `2275a8a` and is unchanged in `9f64119`.

The manual Centauri ID dialog validates input, clears it on cancel/reopen/submit,
and discloses its unsaved, unencrypted, no-discovery behavior. Stale connection
results cannot update a changed, revoked or removed printer. Native QA at the exact
code above saves a fake approved endpoint, rejects an invalid ID locally, then
submits a synthetic ID into the compiled QA network-denial guard. The expected
rejection leaves connection/job state and last-check time unchanged; the ID is
absent from the database and settings. No real printer or LAN discovery is used.
The shared fleet dialogs are opaque, labelled and readable at 1000- and 482-pixel
window widths. The unlicensed isolated app is confirmed stopped.

Actual native testing also found a boundary mismatch: the UI submitted a full URL
in the backend's bare-host field. The adapter now separates authority and origin,
preserves an HTTPS origin through untouched edits, and decodes bounded structured
errors. Shared TS/Rust fixtures prove persistence for HTTP, HTTPS and host/port
metadata. These fixtures do not extend status transport compatibility: existing
private-IPv4 admission and fixed protocol ports remain unchanged.

Validation: 349 frontend tests / 1,463 assertions across 40 current-worktree files,
format/lint/typecheck and both web builds pass; 121 catalog-core tests, affected
Clippy and Rust formatting pass. Windows GNU compilation of catalog-core and
runtime with all test targets passes, without executing Windows tests. The exact
Community production build passes strict signature verification; executable
SHA-256 is `298ddba17d7176be18f076524264a377f6ab8f888b6cfc402fa764005f889463`.
The isolated private composition passes 44 frontend tests and packaging at this
source; its 67 Rust tests/Clippy passed at `0ffc262`, with equivalent production
Rust. The new Rust change is test-only. Earlier Linux live geometry acceptance
remains at `9331722`; production Rust equivalence is recorded separately from the
later Windows portability change. Docker Engine availability is rechecked.

Evidence: `output/native-prompt-purge-checks.json`,
`output/native-prompt-final-checks.json`, and the native screenshots they identify.
The normal profile/install, official private core pin and original sources are
unchanged. PNG retirement proof, Windows execution/installers, native Linux
desktop, deferred Bambu hardware/MQTT, official integration and release acceptance
remain open. Full parity is not claimed.

Build storage audit: about 13 GiB free on the Data volume, repository debug output
14 GiB and older generated targets about 1.1 GiB. Working target, release bundles,
provenance, licenses and assets remain protected. Inactivity of cleanup candidates
has not been established; no new cleanup is authorized or performed, so additional
space recovered is **0 bytes**. Any deletion needs a fresh exact dry-run, idle check
and approval.

### Model-reader resource-error checkpoint

The shared bounded ZIP facade is now used by the six indexing/thumbnail readers
and separate native manual extractor. All remain in Free. Concrete resource and
known allocation failures survive optional settings, project metadata, embedded
image and geometry handling; they stop fallback and return through existing
runtime/native error shapes. Verified archive reads finish before staging, and
terminal render/proof errors prevent catalog writes or new cleanup authority.
Ordinary malformed optional metadata and image-size fallback remain compatible.
Backup restoration uses a separate archive format outside this model profile.

Local validation passes 128 parser tests (one existing ignored fixture), 347 runtime
tests, 207 native tests and 70 server tests, with workspace Clippy and formatting.
The identical native binary proves the sandbox suppresses filesystem callbacks:
all eight watcher tests and the complete native suite pass with normal macOS
access, without changing source or deadlines. The disposable downstream app also
compiles against the exact reviewed public source and vendored dependency.
Hosted verification follows this checkpoint; compilation does not close Windows
runtime acceptance. Inflation, XML/JSON allocation and whole-producer admission
still precede immutable publication. See the [parser bounds contract](THUMBNAIL-PARSER-BOUNDS.md).

### Physical 3MF input-storage checkpoint (2026-10-20)

The Free parser admits physical records before caller attribute extraction and
storage growth, using one private counter owner per parsed Package: 6,000,000
vertices, 2,000,000 source triangles, 10,000 objects, 100,000 components and
100,000 build items. Root and actually reached child parts share these counts;
cached references reuse parsed data. Unused, disabled, duplicate and discarded
records count, while otherwise-unreferenced ZIP entries remain unread. This is
a new supported-input policy, separate from the existing flattened-output limit.

Source vectors and new-key object/part maps reserve fallibly. Flattened positions
and normals reserve before either append. Transform parsing retains twelve stack
values while validating all tokens, preserving malformed-input priority. A fresh
Package resets only its physical counters; shared context inflation/read work
remains cumulative. Ordinary structural errors precede admission; over-limit
records stop before malformed scalar fields, while under-limit behavior remains.

Five causal tests failed on the old parser's accepted over-limit records before
implementation. Final frozen-source workspace verification passes 247 core tests plus one existing
ignored fixture, including eighteen local admission/fault tests and six public
Stored/Deflate compatibility tests. The same run passes 375 runtime, 209 native
and 70 server tests. Downstream composition, Clippy, hosted and container results
are separate gates recorded under `output/input-collections-*`.

Derived part/plate/material containers and owned string lifetimes, allocator
capacity/overlap, global metadata/producer admission, immutable publication and
final platform/package gates remain open. These physical counts do not establish
a hard RAM bound, authorize cleanup or enable the proposed producer allowance.

### Bounded optional part retention (2026-10-20)

Free now shares one optional-part accumulator across whole-build and assigned-plate
emission. It retains at most the existing 1,024 rows, then drops part rows/text and
backing on the next nonempty boundary. Geometry and later validation continue;
the existing exact warning/empty-parts behavior remains. Zero-geometry, disabled
and unprinted candidates do not invalidate exactly 1,024 useful parts. IDs/names
are built after successful nonempty slot reservation, and vector growth is capped
and fallible with the same terminal observer. Appearance copies are suppressed
at capacity/after omission; below-cap in-flight appearance scratch remains a
separate contributor. Global material lists and public model fields are unchanged.

Two real parser RED cases demonstrated three retained rows/name builds under a
private two-row profile after geometry and warning assertions passed. The final
core suite passes 260 tests plus one existing ignored fixture, including nine
local retention/fault tests and four public Stored/Deflate exact-limit, authored
plate, late-error and geometry controls. The full serial workspace also passes
375 runtime, 209 native and 70 server tests; Clippy adds no new diagnostics.
Downstream and packaged verification remain separate gates under
`output/part-retention-*`.

This closes optional part-row accumulation before the existing overflow fallback.
Remaining plate/assignment/occurrence/diagnostic/material containers, other owned
strings and public clones, producer memory admission, immutable publication and
final platform/release acceptance stay in the original scope. No cleanup, original
source changes, installed-app replacement or live private core-pin update occurs.

Build cleanup (2026-10-20): after fresh idle/open-file and generated-output checks,
the user-approved exact `cargo clean` for the older repository target’s dev
`library-runtime` outputs completed. Cargo reports 23,487 files / 3.5 GiB; measured
Data-volume free space increased by 2.09 GiB (5.5 to 7.6 GiB). The existing working
test binary still passes and the protected release bundle still verifies its
signature. Source hashes were unchanged by cleanup. The original failed
post-check is preserved: Cargo’s printed dry-run paths omitted package-named
dependency artifacts counted in its summary, so the helper incorrectly expected
those generated hard-link aliases to remain. All such missing aliases were also
compiler output in the same older target, covered by the exact approved command.
The policy now requires inspecting that dependency namespace and linked outputs
as well. No further deletion or neighboring-target/cache cleanup was performed.
Evidence: `output/approved-runtime-cleanup-20261020/reconciliation.json`.

### Shared build index and streamed plate membership (2026-10-20)

Free now constructs one fallible physical build index for plate membership and
both geometry-emission paths, preserving disabled occurrence numbering. Streamed
membership retains only unique source indices across all plates. Unknown or
duplicate membership drops unusable resolved output while preserving the old
streaming structural return and EOF error order; it remains ordinary optional
metadata fallback. A completed ordinal and fixed 256-ID stack preserve the old
plate limit and deferred duplicate-ID checks. Deferred unclaimed-geometry checks
retain their old source-loading phase. Diagnostic paths deduplicate borrowed
normalized pairs before one fallible owned copy; all existing traversal-work
debits remain. No new metadata row quota or public field shape is introduced.

Two real RED cases showed three retained assignments for two build members and
four diagnostic copies for one unique pair after output compatibility checks.
Final frozen core verification passes 271 tests plus two doctests, with one
existing ignored fixture. Eight local causal/fault tests cover all ten reserve
sites and actual late typed-stop/immediate-structure boundaries. Five public
Stored/Deflate tests preserve error priority, the 256/257 boundary, empty plates,
disabled occurrence numbers, authored order and part IDs. The full serial
workspace also passes 375 runtime, 209 native and 70 server tests with unchanged
source hashes. Workspace Clippy adds no changed-area diagnostics; the previous
four match by exact code/message/source text without suppression. Downstream and
packaged checks are separate gates in `output/derived-maps-*`.

Material and slicer-settings containers, other owned text and public snapshots,
live metadata lifetime/capacity policy, producer memory admission, immutable
publication and final platform/release acceptance remain in the original scope.
This checkpoint authorizes no installed-app replacement, private pin update,
release or additional cleanup.

### Standard material acquisition and selected display copies (2026-10-20)

Free now classifies base/color rows with borrowed normalized values and a fixed
stack color before final owned-field creation. The existing local 1,024 distinct
(group,index) policy, invalid/clipped index advancement, first accepted duplicate,
first attribute error/local-name semantics, Unicode clipping and opaque color
behavior are preserved. Local row/lookup growth and ID/name/color construction
are fallible. Global display clipping remains separate and in part-load order;
only selected rows copy fields through the same terminal observer. Child local
materials still exist when the global list is full; no new package-wide material
quota or appearance remapping is introduced.

Three real RED cases observed excess local normalized String constructions after
output checks passed: 1,025 versus 1,024, or three duplicate-key values versus one.
Old global selection was already correct. Five public Stored/Deflate compatibility
controls passed before and after the migration. The frozen core suite passes
284 tests plus two doctests and one existing ignored fixture, including eight
local material controls covering nine actual acquisition sites, typed post-reserve
first causes and deferred child membership. The frozen full serial workspace also
passes 375 runtime, 209 native and 70 server tests with source hashes unchanged
(`output/material-retention-workspace-results.json`). Workspace all-target Clippy
passes; the existing changed-area diagnostics fall from four to two with no new
diagnostic, compared by code, message and exact primary source text
(`output/material-retention-clippy-results.json` and
`output/material-retention-clippy-diagnostics-audit.json`).

Those test and lint results precede the comment-only history rewrite. The current
material implementation and public-test bytes remain identical to their tested
freeze; only eight approved comments in three consumer files differ. The explicit
bridge is recorded in `output/material-retention-after-history-handoff.json` and
`output/material-retention-post-history-review.md`; it is not a new post-rewrite
test run. The committed material checkpoint `17f8b6e` subsequently passed
368-input downstream compilation, both hosted workflows/Community packaging and
nine synthetic container check stages. The first fixture-warning assertion
failure remains recorded; its reviewed exact-warning correction reused the same
source bytes without rebuilding, restarting or rescanning. Evidence is retained
in `output/material-retention-checkpoint.json` and its linked records.

SDK Cow scratch, other owned text/public clones, the live-metadata ownership
policy, producer admission, publication and final platform/release acceptance
remain open. Settings-map retention is next in the reviewed sequence. Cleanup
still requires exact fresh approval; this code change performs none.

### Slicer settings selection and guarded growth (2026-10-20)

Free now retains slicer settings only for the root-object and direct-component
keys consumed by the existing viewer boundary paths. Relevance of an object and
its nested part is independent. Irrelevant records still participate in scanner
state and attribute processing, preserving ignored nesting, duplicate-field
invalid flags, complementary merges, invalid/Empty/nested contexts and existing
EOF behavior. Bare EOF preserves completed records without flushing unfinished
records; actual ordinary malformed metadata still falls back without losing
geometry. No new ID quota, hierarchy validator or text-byte policy is added.

Relevant name backing and missing-key map growth reserve fallibly at three
actual sites. Pre/post checks and typed terminal-source propagation preserve the
same observer's first cause before optional fallback. Retained settings parsing
does not recharge text usage. SDK attribute scratch and temporary relevant-record
names remain explicit; root/direct-component relevance bounds do not establish an
aggregate live-memory or RSS allowance.

Two real Stored RED cases preserved geometry/name/slot output before exposing
copy/object/part tuples `(3,3,0)` versus `(1,1,0)` and `(4,2,3)` versus `(2,1,1)`.
The new path passes both Stored and Deflate, six local controls including 48
actual-site terminal-failure cases, and all 17 public compatibility controls
(18 XML inputs). The full parser suite passes 307 tests and two doctests, with
one existing ignored fixture. Workspace Clippy passes with the same two existing
changed-area diagnostics and no new warning or suppression. Full serial workspace
tests also pass, including 375 runtime, 209 native and 70 server tests, with all
150 frozen Rust/Cargo inputs unchanged. Exact downstream compilation also passes
with all 370 public inputs matching and 1,333 pre-Cargo entries preserved.
Current-source hosted/container results are tracked separately.

The next ordered work remains live metadata ownership/lifetimes and snapshots,
defensible producer admission, immutable preview publication and final
platform/release acceptance. This checkpoint does not authorize installed-app
replacement, private pin integration, release or additional cleanup. The exact
approved old-target library-core dev cleanup recovered a measured 1.54 GiB;
its receipt is `output/settings-relevance-build-storage-result.md`. That approval
has been consumed; future deletion follows AGENTS.md and needs fresh exact scope.
