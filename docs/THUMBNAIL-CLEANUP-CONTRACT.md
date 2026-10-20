# Thumbnail cleanup caller contract

Implemented caller contract. Workspace, transport, frontend and static checks pass;
packaged native and synthetic container acceptance are the current delivery gate.
Cleanup now uses bounded independent raster proof and retained PNG receipts.
Content-addressed producer publication remains a separate, later slice.

## Shared API and review results

Use one coordinator in `library-runtime/src/thumbnail_cleanup.rs`, shared by
native and remote adapters. Reuse `ManagementEngine::plan_action/execute_action`
and its action-token store. Shared additions:

```rust
// Protocol: camelCase; reject unknown request fields.
pub enum ThumbnailCleanupMode { Orphans, ResetDefaults }
pub struct ThumbnailCleanupRequest {
    pub mode: ThumbnailCleanupMode,
    pub continuation: Option<String>,
}
pub struct ThumbnailBatch {
    pub mode: ThumbnailCleanupMode,
    pub reviewed_files: u32,
    pub eligible_files: u32,
    pub eligible_bytes: u64,
    pub preserved_files: u32,
    pub protected_source_copies: u32, // subset of preserved_files
    pub default_records_to_clear: u32,
    pub partial: bool,
    pub has_more: bool,
    pub remaining_files: Option<u32>,
    pub continuation: Option<String>,
}
pub struct ThumbnailProgress {
    pub review: ThumbnailBatch,
    pub default_records_cleared: u32,
    pub stopped: bool,
}
// Runtime adapter; delegates to the same management action/token implementation.
pub fn review_thumbnail_cleanup(
    &self, ctx: &RequestContext, request: ThumbnailCleanupRequest,
) -> Result<ActionPlan, RuntimeError>;
```

Add `ManagementEngine::discard_action(ctx, token)` as a narrow owner-checked
resource release, with native and REST/MCP adapters using the existing token request
shape. It never executes an action or changes files. Wrong-principal requests must
not consume the owner's plan. Remove/drop owned receipts and their pending
continuation under short store coordination; replay is harmless/stale. Cancel,
Escape, dialog replacement and disconnect attempt discard before clearing the key.
Use a distinct discard command/route. Never send a discard flag to an older
execute endpoint that might ignore it and perform deletion on Cancel. A failed
request must not be reported as confirmed release; TTL pruning/runtime
shutdown remains the bounded fallback. Do not auto-execute to release resources.

Replace unit `ManagementAction::CleanupOrphans` with
`CleanupOrphans { continuation: Option<String> }`, defaulting the new field so
existing `{ "kind": "cleanupOrphans" }` JSON remains valid. Add a distinct
`ResetDefaultThumbnails` action. Add optional `thumbnailReview: ThumbnailBatch`
to `ActionPlan`, and optional `thumbnailProgress: ThumbnailProgress` to
`ManagementResult::CleanupCompleted`. Retain existing `files`/`bytes` fields.

Native adapters use the trusted local principal:
`library_review_thumbnail_cleanup(request) -> ActionPlan` and
`library_execute_thumbnail_cleanup(token) -> CleanupResult`. Native `CleanupResult`
retains `files`, `bytes`, `issues`, `handoffCleanupSkipped`, adding optional
`thumbnailProgress`. Migrate old native cleanup/purge callers; remove their old
physical-delete implementations rather than retaining an unreviewed fallback.

Fields serialize camelCase. Checked file/record counts fit u32; new u64 byte totals
use canonical decimal strings consistently in REST/MCP/native DTOs. Existing wire
field types remain unchanged. Update OpenAPI and clients together. Unknown remaining
counts are null, never guessed zero. Partial/protected-only reviews are not a
complete “no orphans” success.

## Authority, receipts and budgets

Make `PlannedAction` non-Clone. Authorize with `actions.get(token)`, then move the
actual plan with `actions.remove(token)` once, atomically under the same short
store lock. Wrong-principal access must not
consume the owner's token. Store owned non-Clone receipts, not deletion-authority
clones. Bind plans/cursors to principal, mode, pinned root/namespace identity,
Library epoch, catalog revision, selection and expiry. No caller-supplied root/path.

Keep five-minute plan TTL. Consume continuation tokens once; start their fresh
five-minute TTL when successful execution activates them, preserving the original
confirmation-plan expiry. Limit cursor storage to 64 small entries
containing the last inspected name, not retained directory listings/source buffers.
A review's continuation becomes usable only after that review token is consumed;
protected-only continuation consumes a zero-delete plan before reviewing the next
page. Activate continuation only after successful execution/zero-delete consumption;
a stopped or failed execution invalidates it and requires a fresh review. Cancelled/expired reviews release resources and require a fresh review.
Restart, namespace/root replacement or epoch/revision change discards authority.

Per runtime, cap outstanding proof output at **64 MiB** and **64 receipts** across
all plans. Reserve one receipt slot plus 4 MiB before rendering/admission, reconcile
to actual bytes afterward, and share immutable proof Arcs without copying PNGs.
Successful discard, stale-plan pruning and runtime drop close FDs/release permits
without unlink or holds. Expiry is enforced before admission/execution; document
that idle expired plans are reclaimed on the next prune or runtime shutdown unless
an existing housekeeping tick can perform the same pruning. Do not promise an
independent exact-time expiry cleanup that does not exist. Prune expired plans/cursors before capacity reservation, so expired
ownership cannot cause permanent Busy. Accounting permits must not retain Runtime Arcs. No budget/token
map lock spans source reads, rendering or retirement.

Use one global proof worker per Runtime, acquired before source reads or parsing.
Concurrent reviews return Busy before source allocation; this is not one worker
per request. Reserve source buffers through existing Owned byte admission before
allocation; keep its RAII charge through parsing/rendering. Do not hold the budget/token-map mutex while doing the work.

Each page: at most **four retained candidate receipts and four uncached renders,
400 MiB source bytes,
128 million reserved raster samples, 4 MiB per PNG**, existing 100 MiB per-source
and parser-expansion limits. Reserve before allocation. Add only the read-only
`ThumbnailRecipe::raster_sample_budget()` accessor needed for admission. Check a
**two-second cooperative deadline** before each admitted proof unit. Finish the
bounded source-read/render/receipt-admission unit already admitted, even if it
overruns; never discard completed proof just because it crossed the soft target.
Synchronous parsing/encoding is not hard-preemptible. The same source-I/O budget and checkpoints apply to
execution revalidation; do not hash every source twice in a prepass plus per-file
checks. Four retained candidates bound per-file source rechecks independently of
render deduplication. Bound enumeration using the existing namespace limit,
sort names, resume after the cursor and advance past **every inspected entry**,
including unprovable/malformed/protected entries. If budget prevents the next
inspection, leave that entry for continuation. Unsafe/oversized PNG enumeration reports partial/unknown remaining and performs
no deletion. Uncertain non-deleting source-copy counts preserve all copies and
report unknown totals, while retaining independently proven PNGs and their
continuation. No broad fallback scan.

## Admitted source and exclusion rules

Factor existing `verified_model_source_bytes` into a short DB admission snapshot
and a verified loose-source read outside the DB lock:

```rust
struct ThumbnailSourceSnapshot { // private, immutable fields
    model: Model, source: ResolvedModelSource, index_root_limit: Option<u64>,
    library_epoch: u64, catalog_revision: u64,
}
fn admit_current_thumbnail_source(
    runtime: &LibraryRuntime, db: &LibraryDb, recipe: &ThumbnailRecipe,
) -> Result<Option<ThumbnailSourceSnapshot>, RuntimeError>;
fn read_verified_loose_thumbnail_source(
    runtime: &LibraryRuntime, source: &ThumbnailSourceSnapshot,
) -> Result<Vec<u8>, RuntimeError>;
```

Review holds the existing runtime active-I/O lease across admission snapshot,
source read, rendering, candidate admission and the final root/epoch/revision check
before storing a plan. Release the lease before returning the confirmation dialog;
retained receipts do not hold a long-lived I/O lease. Purge/restore cannot cross
proof reads. Execution already holds maintenance, uses its guarded DB directly,
and must not acquire nested active-I/O or runtime `read_db` access. Native viewer
execution gating does not span the user's modal lifetime.

Use a bounded current-content-hash DB lookup, existing operator root admission,
source size/hash and model version. Historical/removed/missing/unadmitted sources
are explicitly unsupported and preserved. Under maintenance use its guarded DB
connection: runtime `read_db` correctly rejects maintenance. Drop DB access before
reading/parsing/rendering; verify a regular no-follow source, identity, exact size
and hash against the snapshot. Never search arbitrary roots from a filename hash.

First coordinator slice supports **loose sources only**, including an admitted
loose 3MF whose whole file is charged. Preserve `source.archive_member != None`.
The private archive member reader currently performs outer preflight/hash/parsing
without this batch's outer-I/O accounting/cancellation; temporary extraction also
creates original-source copies. Neither is a proof shortcut. Future archive support
requires bounded in-memory extraction charging outer I/O and expansion. Normal
archive thumbnails remain functional. Embedded preview copies always remain
protected; use the opaque raster proof only, with its source/recipe/output binding.

Protect every `thumbnail_relative_paths()` reference, including custom, inactive
and stale rows, plus active leases/readers. Maintenance must continue to reject
active I/O even after a reader's token expires. Native adapters retain the viewer
commit gate and wait for native thumbnail rendering to idle. Immediately before
**each capture**, recheck all references, active leases, source snapshot, pinned
root/namespace and epoch/revision. Cleanup needs current Maintenance+MediaWrite;
reset also needs CatalogWrite. Recheck capabilities on execution.

Primitive signatures, coordinated with its worker (confirm final build before use):

```rust
PngNamespace::open(expected_root: &Path, admitted_root: &Directory)
    -> io::Result<PngNamespace>
PngNamespace::admit_reproduced_png(&OsStr, Arc<RasterGeneratedPng>)
    -> io::Result<PngFileReceipt>
PngNamespace::retire_png(&mut PngFileReceipt)
    -> Result<PngRetirementOutcome, PngRetirementFailure>
// outcome: unlinked_now: bool, removed_bytes: u64
// failure: source: io::Error, outcome: PngRetirementOutcome
```

The receipt retains its original FD and exact proof. Atomically capture into an
exclusive private claim, update Claim state immediately, verify captured identity/
bytes, unlink only that claim, update Unlinked immediately, then sync. Public
replacements survive. Count post-unlink failures truthfully and stop further work;
retries cannot double-count. Unknown claims are never swept. No raw File/arbitrary
bytes/fresh candidate hash can mint authority. Generic delete_verified APIs must
not remain on PNG retirement paths.

## Default reset, protected copies and UI

Reset review retains only a constant-size count/digest of ordered thumbnail
metadata, not per-row path vectors. Stream at most 10,000 default records, 200,000
rows total and 64 MiB text, with 128-byte IDs, 4096-byte other fields and cooperative
two-second metadata checks. Oversize/malformed data is Unsupported with rows/files
unchanged; never truncate. Transactional reset compares that digest/count, reserves
bounded affected-model memory before allocation and rolls back if its work bound
is exceeded. Reference absence checks likewise stream with the 200,000-row/64-MiB/
4096-byte/two-second limits and cannot establish absence from a truncated scan.

ResetDefaults confirms then clears only noncustom default rows in one revision/
selection-checked transaction, keeping custom fallback behavior. It creates no PNG
receipts and deletes **no files**. Afterward offer a separate fresh orphan review
at the new revision; rows being reset are never exceptions to reference exclusion.
Remove native `remove_thumbnail_paths` unlinking. Core returned paths are hints,
not deletion authority. Unknown/adopted/embedded files remain untouched.

Both existing indexed-cache cleanup branches must preserve/skip source and
archive-member model-byte copies and count them as protected. Their hash directory,
fresh hash or missing reference is insufficient proof. Never put them into PNG
receipts. Source/download/archive lifecycles stay unchanged; separate reviewed
source-copy ownership is required for any later retirement.

UI examples: “Reviewed 12 files. Can remove 2 rebuilt previews; kept 10. More files
remain.” / “Reviewed 12 files and kept all 12.” / “Reset 8 default preview entries.
All files were kept.” Offer “Remove 2 previews”, “Cancel”, or “Review remaining
files” explicitly. Continuing cannot automatically delete another page. Completion
means this ordered pass ended, not that concurrently inserted or preserved files
are absent. No internal recipe/FD jargon, browser source paths or raw errors.

## Ownership and acceptance

- Primitive worker: secure_fs.rs only, initially unwired. Preserve generic source,
  anonymous geometry and control-metadata APIs.
- Coordinator worker: new thumbnail_cleanup.rs; runtime source snapshot, receipt
  ownership, budgets/cursors; narrow core lookup/reset transaction if needed.
- Adapter worker: native management/commands, protocol/OpenAPI/clients/UI. Replace
  both name/hash orphan eligibility and native purge pathname deletion.
- Publication worker: later separate gate; do not enable CA publication early.

RED/GREEN tests must prove forged names/adopted rows and embedded/custom/history/
missing sources survive; bound proof cannot pair with another recipe; active leases/
readers/viewer races survive; same-byte/same-size replacements survive around
capture; wrong-principal denial preserves owner tokens, owner discard/expiry/replay/root/
epoch/revision failures release budgets correctly; unprovable prefixes do not starve continuation; reservations fail before
allocation/I/O; post-unlink sync failure reports partial counts; reset keeps unknown
files/custom fallback; both indexed-cache branches preserve source copies; proof
performs no temporary extraction writes; DTO/OpenAPI/UI report partial/protected
results honestly. Affected runtime/core/native/frontend gates pass. Packaged native and synthetic
container acceptance must pass before this caller slice is declared complete.
No live network or historical cleanup is part of this acceptance.
