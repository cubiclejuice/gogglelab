# Phase 5: Free filaments and reviewed AI tagging
Date: 2026-10-10
Status: locally implemented, built, verified and installed with names-only provider metadata.
Baseline: Printventory GUIDE at f551c4601c03072362b426c652b5dd37e4c55601.

## Product contract
All capabilities are public MPL-2.0 Community and work without a Pro license.
Local filament records have stable IDs, name, brand, material, color and notes.
Models can assign up to five catalog IDs; filters compose with existing scope.
Print records select catalog filaments and preserve unknown legacy references.
Deletion archives referenced filaments rather than silently erasing history.

Spoolman is an explicit pull from a configured origin. GET collection pages only;
never send POST/PATCH/DELETE. Filaments and physical spools remain distinguishable.
Repeat pulls upsert origin/resource/external ID, preserve stable local identity,
and retain referenced entries absent from a later pull. Partial network failure
must not erase a prior catalog. Configure a local/private HTTP or remote HTTPS
endpoint deliberately; source URLs in model metadata never choose destinations.

AI tagging sends an explicitly selected model's bounded rendered thumbnail and
minimal disclosed metadata only after user action. Support OpenAI-compatible
endpoints (including local keyless servers), OpenAI, Claude and Gemini adapters.
Model, endpoint, maximum tags, category mode, detail, strategy (merge, replace,
append) and concurrency are configurable. Persist nonsecret options; credentials
remain session-only unless a vetted OS secret store is added. Never include keys
in catalog exports/backups/logs or provider error bodies. No automatic network
calls or real provider charges in development tests.

Suggestions are untrusted data. Parse a bounded strict tag shape; show per-model
results and failures for review. Accept/reject explicitly. Apply accepted changes
with model versions; a stale response cannot overwrite later edits or a restored
library. Category associations and merge behavior are deterministic and tested.

## Ordered implementation and ownership
1. Data/schema/recovery: index_storage owns library-core, library-backup and
   recovery adapters. Back up before schema migration; schema4 roundtrip preserves
   all earlier model/history/bundle/source references. Define DTOs before UI wiring.
2. Network adapters: index_native owns new integration modules/crate and native
   integration commands/dependencies. All responses/requests bounded, redirects
   disabled, timeout/concurrency limits; mock localhost endpoints only.
3. UI: catalog_ui owns Library/IPC/main/styles and new filament/AI dialogs.
   Explicit scopes, disclosure, accessible controls, partial errors and review.
4. Parent owns native registration, docs, integration, independent review,
   browser QA, full Community/unified builds, local installation/checkpoints.

## Gate
Data fixtures cover v1-v4 upgrade and recovery, referenced archival, stale edits,
unknown history IDs, exact assignments/filter scope, repeated and partial pulls.
HTTP fixtures verify GET-only pagination, identity, redirect refusal, size/time
limits, provider request shapes, key redaction, malformed suggestions and failures.
UI fixtures verify selection/review/merge-replace-append, rejection, stale results,
retry and keyboard/narrow layout. No provider/user catalog data is used.
Independent data/network/UI/security challenges must resolve high/critical findings.
Both app packages must build; install and launch unified app before Phase 6.
Native capture/cross-platform/runtime-provider limits remain explicit.

## Research and workflow
The accepted overall parity plan supplies product direction; repeated ideation is
bypassed. Three frontier workers are justified by migration, integration secrets
and optimistic UI/data boundaries. No ensemble. Skill updates remain skipped to
preserve modified installed skill repositories. Equivalent scoped plan reviews
and independent security/code reviews produce the finding ledger below.
Primary Spoolman source returns a bare array with limit/offset and x-total-count;
OpenAI-compatible advertising does not establish vision/JSON-schema support.
Official contracts cached in output/spoolman-filament.py, spoolman-spool.py and
openai-chat-api.html; research discovery is output/phase5-research.json.
Primary links:
- https://github.com/TechJeeper/Printventory/blob/f551c4601c03072362b426c652b5dd37e4c55601/GUIDE.md
- https://github.com/Donkie/Spoolman/blob/master/spoolman/api/v1/filament.py
- https://github.com/Donkie/Spoolman/blob/master/spoolman/api/v1/spool.py
- https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

## Findings and evidence
- Independent data and network plan reviews resolved identity ambiguity: import
  filament:<id> and spool:<id> as distinct resources, each origin-scoped.
- Spoolman archived entries are explicitly requested; no partial snapshot commits.
- Strategy: replace exact accepted suggestions; append deduplicated union; merge
  replaces tags in suggested categories and preserves unrelated existing categories.
- Credentials are session-only; restart clears them. Config export contains no keys.
- Remaining implementation and independent code/security/UI validation pending.

## Final local contract and reviewed findings
- Current external payload: bounded normalized preview + display name + assigned tag names.
  Category names/objects and full taxonomy are local only. Automatic approval review
  rejected their export and required explicit human authorization. A concrete optional
  proposal is AI-CATEGORY-SHARING-APPROVAL.md; approval is pending. This does not enable
  any external request and does not gate Free categorization/review functions.
- Native category rules enforce none/existing-only/allow-new locally. Known tag identity
  resolves locally for exact before/after; rename/deletion is revalidated at apply.
- Independent review fixed single-flight admission, source IDs after restore, malformed
  Spoolman type coercion, taxonomy races, explicit rejection/all-reject no-op, retryable
  failed reviews, bounded preview failures, and atomic heterogeneous filament batches.
- UI review fixed missing model legends, Reject-to-Accept controls and late-token disposal.
- Browser fixture at480px retains unavailable/unknown refs until explicit removal;
  exact versioned assignment is captured. AI tests verify exact merge vs rejected
  unchanged model, visible conflict/retry, cleared dummy session key, busy Generate,
  late-response discard and invoker focus. Dialogs are448px wide without overflow.
- Audit identified pre-existing development advisories. Vite6.4.4 plus minimal
  tinypool2.1.2/source-map-js1.2.2 overrides clear the public Bun audit. Unified audit
  requires repinning the new public package before its final check.
- Focused final data: core40, recovery17, backup4, network9, native integration13.
  Full Community gate: frontend86/241 assertions, native151/one ignored,
  core40/recovery17/backup4/network9 and remaining suites pass with one existing
  ignored test. Strict owned Clippy and both default workspace checks pass; new
  warnings removed, unrelated existing warnings retained.
- Unified frontend44/1198 assertions, native tests/Clippy and macOS packaging pass.
  Both public/unified Bun audits report no vulnerabilities. Public4a34597/unifieddfcd037.
- Signature verified; installed executableSHA256:
  50e778a4e3668a90bd09a8b868be611140819251457baed559530af517bd5328.
  Process48906 observed; previous app output/app-backups/GoggleLab-before-phase5-20261011.app.
- Native capture retry still returns cgWindowNotFound; no native visual/slicer or
  Windows/Linux GUI runtime claim. Phase6 may proceed with current sharing boundary.
