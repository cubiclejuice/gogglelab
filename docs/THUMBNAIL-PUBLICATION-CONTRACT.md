# Immutable thumbnail publication contract

The cleanup caller slice passed automated, packaged native and synthetic container
gates. The first **unwired** publication foundation now passes its workspace,
final affected catalog, Clippy, formatting and Community package/signature gates:
selected-row catalog snapshots/pure PNG validation, Owned reservation ownership,
and raw NoReplace ordering. Producer admission, parser hardening, stage lifecycle,
immutable publication and caller migration remain pending. This contract enables
no new publication or deletion by itself.

## Cache contract and migration sites

Normal reuse starts from a catalog reference, not a directory or guessed filename.
All display reads require bounded valid PNG bytes and the mandatory stored PNG
hash. Custom and source-hash-NULL legacy rows may display offline from safe nested
catalog paths; this display-only result never qualifies as current producer reuse,
creator ownership or deletion proof. Source-bound generated rows require the
supported key to match current catalog source/settings and, for CA names, the
output hash. Custom selection behavior stays intact. Only a compatible source-bound
current-recipe result may avoid verified production; a missing/corrupt/stale row or
explicit regeneration goes through verified rendering when generation is requested.
Passive display itself remains read-only.

Remove both shared semantic adoption paths in `thumbnails.rs`: the first
`ensure_thumbnail` cache check before source verification, and the valid-PNG
shortcut in `ensure_thumbnail_from_bytes`. A cold miss must not adopt a foreign
legacy PNG merely because it occupies the expected recipe name.

Native `library_commands::cached_thumbnail` already returns None when no row exists;
its cold caller indirectly reintroduces adoption through shared `ensure_thumbnail`.
Replace its legacy canonical-path equality check with shared validation of the
actual catalog entry, so CA rows remain reusable. Update these production callers:

- Native `library_get_catalog_thumbnail` / cached-only helper for passive cards and
  duplicates: read-only catalog validation only, never source reads or generation.
- Native `get_thumbnail`, now used by explicit jobs/refresh, its two catalog-cache
  checks, and recipe lock key.
- Runtime `catalog_jobs::render_model` and `render_default_thumbnails_for_root`:
  an entry being Some is insufficient; use shared bounded catalog-file validation.
- Native `library_thumbnail` re-exports and relevant standalone producer tests.

Replace `thumbnail_relative_path` with `thumbnail_recipe_key(source_hash, settings)`
returning a logical String used only for producer locking/coalescing. It is not a
filesystem path and must never be joined to the Library root. The sealed payload
alone derives the final CA basename; `ThumbnailArtifact.relative_path` carries the
actual published path. The shared catalog-key parser reads stored basenames only.
No directory scan is introduced for normal cache lookup.

## Payloads and proposed narrow API

Keep `RasterGeneratedPng` and the source renderer's algorithm unchanged. Add a sealed
producer payload, constructible only inside `thumbnails` after source size/SHA
verification and normal raster-versus-embedded selection:

```rust
// Proposed crate-private contracts; fields/constructors stay private.
struct ProducerCachePng { /* source/output binding, origin, owned budget charge */ }
enum ProducerOrigin { Raster, EmbeddedCopy }
fn render_producer_cache_png(
    permit: ThumbnailProducerPermit,
    source: Vec<u8>, expected_hash: &str, expected_bytes: u64,
    settings: &CatalogSettings,
) -> Result<ProducerCachePng, String>;

// In secure_fs, adapt existing original-FD/prefix stage machinery only.
fn create_cache_stage(&self, payload: Arc<ProducerCachePng>)
    -> Result<SealedCacheStage, CacheStageFailure>;
fn publish_cache_stage_exclusive(&self, stage: &mut SealedCacheStage)
    -> Result<CachePublicationOutcome, CachePublicationFailure>;
fn retire_created_cache_stage(&self, stage: &mut CreatedCacheStage)
    -> Result<PngRetirementOutcome, PngRetirementFailure>;
fn retire_sealed_cache_stage(&self, stage: &mut SealedCacheStage)
    -> Result<PngRetirementOutcome, PngRetirementFailure>;
// Partial CreatedCacheStage may be returned only on creation failure.
// SealedCacheStage owns a completed CreatedCacheStage; no public constructor.
```

The payload accepts no arbitrary raw PNG constructor, File, upload/download/model
path, or existing cache entry. Its embedded variant never becomes raster proof.
Existing final-raster cleanup remains based on independent reproduction and its
separate `PngFileReceipt`; do not broaden geometry/control/source receipts.

Callers must admit the source using current root/operator rules and retain active
I/O across source read, generation/publication and catalog CAS. No DB mutex spans
source reads/rendering. Archive-member producers must admit the outer archive too:
pinned identity/fingerprint, outer size and actual reader passes, entry count,
member size and all expansion layers. Existing bounds are 1 GiB outer archive,
10,000 entries, 512 MiB total archive expansion and 100 MiB selected model bytes;
a nested 3MF also retains its own parser-expansion cap. Do not charge only the
extracted member's declared bytes. Preserve supported archive generation through
streaming/admission; if finite bounds or configured capacity cannot be established,
fail honestly without source mutation or semantic cache adoption. Do not change
original-source extraction cleanup in this phase.

Raster CA key: `<source>-isometric-v1-<size>-<background>-<PNGhash>.png`.
Proposed embedded key: `<source>-embedded-copy-v1-<PNGhash>.png`, with no irrelevant
background/size suffix. Embedded final copies remain excluded from raster orphan
proof even when their bytes match a raster result. Their source archive is untouched.
A typed shared cache-key validator must recognize the separate read-only families.

## Shared producer admission

The required admission does **not** exist today: native `get_thumbnail` has only a
concurrency limiter, default scan rendering lacks a common byte reservation, and
standalone `ensure_thumbnail` allocates raw source/output Vecs. Add one per-Runtime
`ThumbnailProducerBudget`, shared by native, catalog-job and default-scan entry
points. No producer may create an independent budget or use an unbudgeted fallback.
The shared Runtime facade is
`admit_thumbnail_producer(admitted_source, settings) -> Result<ThumbnailProducerPermit, RuntimeError>`;
the source snapshot is constructed by trusted Runtime admission and includes any
outer-archive work, not client-provided size claims. The permit grants resources
only, never source access or mutation permissions.

Proposed additional work cap: **two active producers and 2 GiB reserved work**.
This is an extra restriction, never an independent ledger that bypasses or raises
configured `RuntimeOwned` byte/global-memory limits. Admission must satisfy both.
Before reading/extracting source bytes, reserve input and finite output/scratch
buffers against the existing RuntimeOwned policy and acquire the non-Clone producer
permit bound to source identity/hash/length and validated settings. Charge each
buffer once through a shared owner; no uncharged native/default-scan path remains.

Initial loose-source work estimate: `source_bytes + 512 MiB inflated-parser allowance
+ 192,000,000 geometry bytes + 64 MiB image-decode allowance + size²*8 raster scratch
+ 8 MiB output/winner buffers`. Archive admission must add all outer-input passes
and expansion layers. These allowances are **implementation-gated proposals**:
verify actual parser peaks and finite allocation caps without changing the renderer
before choosing RuntimeOwned buffer reservations. Existing source/parser caps stay
in force. This formula is not proven RSS accounting and cannot enable production
by itself. Lower configured limits must cause bounded waiting/Busy/refusal, not a
larger allowance or fallback. Reject unsupported sizes/overflow before allocation.

Native must acquire before `extract_member_to_temporary` or source read; catalog
jobs before `verified_model_source_bytes`; default scans before the same read.
Retain existing input/render policy through the shared permit; consolidate charge
ownership without dropping configured limits or charging one buffer twice. Source reading borrows that permit; the render helper
then consumes the non-Clone permit and owned source Vec. Its sealed output owns the
charge, so output/stage buffers cannot outlive admission. Raw helper calls cannot
bypass it or reuse the same permit for another producer. If capacity is unavailable, return Busy before source Vec,
parse/raster/PNG allocation or stage creation. Existing native queue limiting may
remain, but is not authority to bypass shared admission. Permit ownership spans
source/render/winner buffers and stage handling; close/drop resources before
releasing its byte/slot charge. Use accounting-state Arcs without Runtime cycles.
Tests can lower these limits; defaults are implementation-gated until allocation
accounting and cross-producer refusal tests pass.

## Parser prerequisite before producer wiring

See [the parser-bound plan](THUMBNAIL-PARSER-BOUNDS.md) for exact allocation sites,
proposed limits and compatibility gates.

The proposed work allowances above are not established parser peaks. Current 3MF
parsing can retain input mesh/object vectors before the flattened-triangle cap,
copy large attributes/model-settings text, and allocate ZIP central-directory
metadata before application-level entry checks. A source-byte reservation alone
cannot justify enabling the new producer budget.

The first unwired checkpoint adds only an Owned reservation transfer, a dedicated
catalog-read snapshot/pure stored-digest validator, and raw NoReplace ordering.
No source reader, parser, producer, cache adoption or publication caller is migrated
by that checkpoint. Review and verify finite preallocation limits next: bounded
ZIP entry count/central metadata before ZipArchive construction; aggregate input
vertices/triangles/objects/components/build items; retained text/token/attribute
sizes; actual inflated bytes; and archive outer-input/hash/extraction passes.
Use small test overrides for boundary tests and retain ordinary multipart,
materials, archive-member and embedded-preview fixtures. Do not silently disable
normal 3MF generation to ship immutable publication. Parser estimates must remain
labelled as estimates until allocation bounds are demonstrated.

## Fixed staging namespace and limits

Use pinned `thumbnails/.gogglelab-png-staging`, not an inventory cap over published
thumbnails. Open/create it exclusively relative to admitted handles; symlink/file/
identity uncertainty refuses new stages. Existing catalog reads and exact-winner
reuse need not open this child and must remain available.

Initial conservative staging limits: **256 child entries and 64 MiB retained/reserved
stage bytes per Library**, plus existing **4 MiB per PNG** and source/parser/raster limits.
Count every child entry, including unknown names/claims; inspect bounded metadata
without following links. Nonregular, oversized or uncountable residuals refuse new
stage allocation. No filename pattern grants cleanup authority.

Hold the existing cooperating CatalogLock and one shared creation/inventory gate
across inventory/admission/name transitions. Use the shared producer admission above
before allocating source/render buffers. Stage disk admission follows
an exact-winner miss and precedes stage creation/writing; it must not block an exact
winner reuse. Payload rendering/write work stays outside that short gate. Include in-flight stages,
retained partials and claims; do not create a check/create race between producers.
Use a fresh bounded inventory when admitting stages rather than trusting stale
counters after unknown replacements. Inventory alone undercounts empty/partial
in-flight writes: all namespace instances must share the Runtime's stage ledger.
Under the same short gate, conservatively charge fresh actual child bytes plus
each in-flight owner's full intended reservation before exclusive creation. This
may double-count visible owned bytes, but cannot undercount pending writes. Keep
that reservation through publication/retirement and handle closure; transfer and
release it under the gate. A separately opened namespace cannot create a fresh
ledger to evade limits. The retained CatalogLock excludes cooperating other
processes for the entire Runtime/stage-owner lifetime. No historical stage/claim sweep or adoption.
Bound descriptor and memory charges; owners drop handles before releasing permits.
Normal cache hits avoid this inventory entirely, regardless of published catalog size.

## Exclusive publication and truthful states

Stages retain their original exclusive read/write FD, identity, pinned parent and
exact intended payload/prefix. Private lifecycle:
`PartialStage -> StageClaim -> Unlinked` (retirement only), or
`PartialStage -> SealedStage -> Published(finalName)`.
No arbitrary replacement is readmitted through a fresh hash.

Seal only after the complete intended bytes have been written and synced. Verify
full exact bytes/length, the original FD, current stage-name identity and admitted
root/staging bindings **before** constructing SealedCacheStage. Partial owners can
only retire their verified intended prefix; publication cannot accept them.
Immediately before final rename, revalidate complete bytes and all bindings again.
Derive the final name from the sealed payload, then use cross-directory atomic
NoReplace rename into thumbnails. Update Published state immediately after syscall
success; verify/sync the file and both source/destination directories afterward.
The existing cross-directory rename helper syncs internally and can return an
error after a successful rename, so the publisher must use a narrow raw NoReplace
syscall helper, transition state immediately, then perform explicit durability
steps. Preserve the generic source caller helper's existing behavior. Test each
post-rename failure without misreporting an unpublished stage. Never use
`replace_child`, generic public-path unlink, or hold fallbacks.

An existing exact final winner may be verified/reused directly, including after a
race. Reuse grants **no receipt or ownership**. Different/invalid/symlink/uncertain
winners remain untouched; fail honestly instead of inventing another semantic name.
After a lost race, retire only this producer's own stage through private capture,
original-FD identity and exact-prefix verification. Preserve unknown replacements.

Publication result distinguishes `PublishedNew` from `ReusedExact` and contains the
artifact. Creation failure may return an owned partial stage; publication/retirement
borrow and preserve the caller's owner with its updated state. Failures report factual
`finalPublishedNow`, `stageUnlinkedNow` and `durabilityConfirmed` outcomes. These
flags describe completed operations, not a promise that a public name still exists.
A sync failure after rename cannot be reported as an unpublished stage. A failure
after stage unlink cannot be reported as a remaining named file or double-counted.

Both retirement methods refuse Published state; sealing does not grant final-file
delete authority. Published is no longer stage-retirement authority. **Do not delete final output on
catalog CAS failure or cancellation:** another producer may already reference an
exact winner. Leave the final file intact; raster orphans require the reviewed
cleanup coordinator, while embedded copies remain protected. No destructive Drop
implementation. Repeated retries with identical bytes converge on one exact CA key.

## Platform boundary and remaining decisions

Current handle-relative thumbnail production is already Unsupported on Windows;
this plan must not claim Windows generation or add a pathname-rename fallback.
Initial publication support remains macOS/Linux under existing platform gates.
Existing catalog/media reading and native in-memory model previews remain unchanged.
A safe Windows producer, including any bounded-memory alternative, needs its own
review and Windows acceptance before enabling it.

The staging limits and logical-key versus final-path roles above are fixed initial
contracts with refusal/boundary tests. Remaining implementation decisions are actual
parser/outer-archive peaks and finite reservations within existing configured policy,
the exact private typestate/error representation, and review of the separate embedded
final key family. Neither unresolved detail permits starting production publication early.

## Ownership and required evidence

Producer owner: `thumbnails.rs` typed payload/key helpers and removal of adoption.
Primitive owner: `secure_fs.rs` private staging child, stage-only lifecycle and
receipt-aware exclusive publication. Caller owner: runtime job/default render,
native commands/re-exports and shared catalog validation. No unrelated cleanup,
source-hold, Docker, credentials or archive-extraction redesign.

Required RED/GREEN tests: cold missing-row foreign legacy PNG preserved; valid
legacy/CA catalog hits reuse without scans; current settings/source mismatch renders;
exact/different concurrent winner; stage/winner/parent replacement; partial write,
file/directory sync and post-rename errors; attempting to publish a partial owner
never creates a final name; complete-stage replacement is detected before rename;
honest unlink outcomes; no generic holds;
unknown child entries/claims count but survive; inventory/create races; thousands of
published files do not consume staging capacity; 256-entry/64-MiB boundaries include
unknown entries/claims; shared low-budget native/catalog/default refusal occurs
before source allocation, including cross-producer saturation, configured RuntimeOwned
limits, outer-archive/expansion admission and permit release; passive cached-only
native reads never generate or touch staging;
DB CAS failure/cancellation preserve final output; embedded preference survives,
failed newly created copy stages are bounded, and final embedded copies never
qualify as raster orphans. Run affected package/native/frontend gates and packaged
synthetic acceptance before enabling publication. No live printer/network checks.
