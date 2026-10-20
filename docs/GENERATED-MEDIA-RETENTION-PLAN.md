# Generated media retention

Status: anonymous geometry implementation and independent review complete; final serial workspace, Clippy, formatting and Community production build/signature checks pass; live Linux packet/restart checks pass. Unlicensed combined-app stock/index/preview and the repaired native scan/collection workflows pass at `0ffc262`, including keyboard focus and actual synthetic slicer handoff. Thumbnail/orphan proof remains a subsequent gate.

## Required outcome

Normal thumbnail generation/regeneration and geometry preview lease retirement
must not accumulate recoverable source holds for rebuildable derivatives. Keep
these capabilities in Free, including valid regeneration after renderer changes.
Preserve original sources, custom/uploaded previews, configuration, unknown bytes,
and existing historical holds. No historical sweep or manual deletion is in scope.

## Trust boundary

- A path shape, PNG validity, or a generated-looking catalog flag alone is not
  ownership. Current cache admission accepts valid preexisting PNG bytes at the
  semantic default path; such a catalog row cannot authorize permanent removal.
- New generated stages/derivatives can carry opaque authority retained from the
  original created descriptor, exact rendered bytes/hash, and pinned parent.
- Keep the generated-control receipt restricted to its reviewed 8 KiB transaction
  metadata/32-byte marker domain. Media requires a separate typed contract.
- Protect ordinary public-path replacements through private exclusive capture and
  verification before retirement. Preserve replacements and surface failure.
  Existing writable-FD mutation by a non-cooperating same-user process remains the
  documented internal-file residual; no privileged broker is authorized.
- Do not downgrade original/download/upload/archive-member bytes into derivative
  ownership merely because they were staged.

## Candidate design questions

1. Keep regeneration functional while preserving different unknown or legacy PNG
   bytes. Compare content-addressed output publication against mismatch rejection;
   rejection alone is insufficient if it disables valid renderer/codec upgrades.
2. Use exclusive publication, compare a concurrent winner with the actual rendered
   output, and retire only the app-created losing stage through retained authority.
   Native-only serialization cannot protect shared headless callers.
3. Carry typed geometry artifact ownership through lease admission, cancellation,
   expiry, explicit close, and runtime drop. Respect active readers/leases.
4. Define platform behavior from existing primitives; no unsupported cleanup may
   be advertised as accepted, and preview capability must not disappear silently.
5. Revisit orphan-cleanup authorization where cache adoption could misclassify
   foreign bytes; generated flags/fresh fingerprints are insufficient evidence.

## Verification gate

- True RED/GREEN normal lifecycle no-growth tests, not filename-only assertions.
- Identical/different render output and deterministic concurrent publication.
- Original/custom/unknown bytes preserved, including same-size replacements.
- Lease close/expiry/drop/cancellation and active-reader behavior.
- Fault injection around admission, capture, hashing, unlink and synchronization.
- Independent contract and implementation review, affected tests, build and actual
  isolated runtime acceptance before advancing. Windows/hardware remain separate
  acceptance gates.

The public branch starts this slice at `bfbc5ae` (verified code `8ea0a4e`). The
previous goal turn completed generated-control metadata delivery and the exact
approved build cleanup; that cleanup approval is consumed.

## Geometry direction: anonymous backing

Prefer making the newly created, exclusively opened empty geometry file anonymous
before writing any packet bytes. Verify its retained original descriptor, capture
its public entry into a private geometry-only name, unlink that claim, and complete
required directory synchronization before payload writing. Linux anonymous-file
facilities may avoid names directly; macOS needs the pinned-directory fallback.
No transaction-metadata receipt is broadened.

An `Arc` backing owns the original descriptor and reserved byte budget through the
last reader. Lease expiry removes token access but does not release backing quota
while a reader still owns it. Use positional reads; retain principal, expiry, epoch,
source/hash admission and I/O leases. Do not create a runtime ownership cycle.
Anonymous files cannot follow a Library rename into a preserved previous Library.
On process exit the OS reclaims unnamed packet bytes. Pre-anonymization interruption
can leave an empty/unknown entry: preserve it and charge bounded retained storage,
without a filename-based cleanup sweep. Native in-memory geometry remains intact;
current headless Windows limitations remain explicit pending acceptance. Exact API
and implementation gate are still under review.

## Thumbnail prerequisite

Content-addressed immutable publication keeps valid regeneration functional while
preserving different legacy/unknown bytes. Before enabling it, fix orphan eligibility:
old generated-looking names and adopted `is_custom=false` rows are not independent
provenance. Evaluate verified re-render proof from admitted source bytes and a
supported bounded recipe. Unsupported/missing proof must preserve bytes. New
staging retirement can use its original descriptor; cross-restart retirement
cannot authorize itself from a fresh candidate fingerprint.

## Approved geometry implementation slice

Independent contract review found no remaining high-severity defect. Implement
geometry first; thumbnail/orphan changes remain a subsequent gate.

- `secure_fs.rs`: crate-private `GeometryFileWriter` and `SealedGeometryFile`;
  `Directory::create_anonymous_geometry_writer()` accepts no pathname/source FD.
  Empty exclusive creation, atomic private capture, original FD/empty identity
  verification, private unlink, zero link count, and required parent sync must
  succeed before any payload. `write_and_seal(self, bytes)` retains that FD; sealed
  `bytes/sha256/verify/stream_to` use bounded positional reads. A common pinned
  directory fallback is sufficient on supported macOS/Linux; `O_TMPFILE` is an
  optional optimization, not a requirement to add another correctness path.
- `geometry.rs`: checked packet length `16 + 36 * triangle_count`, maximum 128 MiB,
  reservation before packet allocation/materialization; return an Arc packet plus
  existing model/plate/material metadata.
- Runtime: typed `ContentBacking::ExistingPath` versus `Geometry(Arc<...>)`. Move
  shared temporary-resource state behind Arc; transfer the existing owned-byte
  reservation into a standalone `GeometryByteCharge`. The sealed file drops before
  its charge. Never reference Runtime from the charge or release geometry bytes
  separately during lease pruning.
- Preserve principal/expiry/epoch/source/hash and I/O admission, retain original
  path-backed lease semantics, filter anonymous geometry out of active path lists.
- Replace filename-based geometry hold conversion with conservative bounded
  retained-entry accounting, including unexpected capture names, nonregular entries
  and symlink refusal. No legacy geometry or unknown bytes are removed.
- No new public release API in this slice: preserve existing lease expiry, eviction,
  drop and streaming behavior; prompt viewer-close release remains an optional
  follow-up rather than a condition for this internal no-growth correction.

Prove RED/GREEN lifecycle no-growth, exact bytes/metadata, original/custom/source
preservation, last-reader quota lifetime, budget overflow before allocation,
anonymization/write/seal failures, public replacement and parent swaps, epoch/owner
rejection before reads, and unchanged previous Libraries. Full affected gates and
real isolated runtime checks precede the next implementation slice.

## Geometry finding ledger

| Finding | Evidence and disposition |
| --- | --- |
| Named packet retained after generation/expiry | Real STL producer and expiry regressions reproduced named packet/hold growth before the fix; anonymous backing removes this path. |
| P2: derivative allocation before quota | Fixed: checked packet length and geometry reservation precede borrowed whole/selected-plate work. Real-flow rejection tests cover both paths. |
| P2: inventory races empty mode-000 birth | Fixed: constructor and full inventory snapshot share one brief gate, released before quota-state locking. A paused-birth concurrent runtime request proves waiting/success. |
| P2: geometry uncertainty blocks thumbnail jobs | Fixed: distinct Geometry reservation; ordinary Owned thumbnail buffers keep existing admission. Unsafe geometry storage is preserved while an actual thumbnail job succeeds. |
| Reader lifetime/quota | Focused tests prove expiry/runtime drop retain an admitted reader, no runtime cycle, and a one-packet budget remains unavailable until the final reader closes. |
| P1: orphan PNG authority from name/adopted row | Still open for the next slice; prevents enabling content-addressed thumbnail publication until independent re-render proof is in place. |

Workers verified 25 geometry-filter tests plus the exact packet-budget test after
an additional expiry-admission regression was fixed. Root's final serial workspace
suite passes: 201 native, 120 catalog-core, 277 runtime and 62 server tests, plus
the other workspace suites. Formatting and diff checks pass. Final workspace Clippy passes with existing warnings. Package and live isolated/container checks remain before delivery. Unknown legacy geometry entries are preserved and counted; previous
Libraries, source holds, uploads and archives retain their original semantics.

Routing: Codex high-risk frontier lane for descriptor/quota implementation and
independent data-integrity review. Configured router aliases are unavailable;
workers inherit the parent model rather than inventing model IDs. Three focused
roles cover primitive/producer, runtime callers, and independent review. Deterministic
fault/race tests resolve the design questions; no redundant whole-task ensemble.
The existing user-approved product direction substitutes for new ideation. No new
provider, network, license, or public geometry-release surface is introduced.

### Expiry admission finding

The first implementation prepared/reserved new geometry before pruning expired
tokens. Under a full one-packet budget, a later request could stay Busy until an
unrelated token read pruned the map. An explicit `describe_content` in the original
reader test masked that flow. Removing it reproduced the failure. The request now
prunes expired leases after capability/I/O/model/hash admission and before packet
reservation, with the lease mutex released before parsing or filesystem work. A
late prune/admission check remains for concurrency. Direct next-request tests cover
both retained-reader Busy and no-reader automatic reclamation. Independent review
found no remaining issue in that flow.

A host Windows GNU cross-check stopped in bundled SQLite's build script because
`x86_64-w64-mingw32-gcc` is absent. It did not establish Windows Rust compilation
or runtime acceptance. No host compiler/toolchain was installed; friend-run native
Windows acceptance remains required.

The final optimized Community app builds and passes deep strict signature verification. Executable SHA-256: `aa495584633fd3a9e33beab7cdd6e2ec7de8cbc66c60e51edef6d92a10d902fe`. Current-source Linux runtime and updated development composition checks remain before closing the geometry delivery slice.

## Geometry delivery and platform follow-up (2026-10-19)

The immutable `9331722` Linux image passes authenticated packet streaming without
a named staging file, repeat reads, stale-token rejection after restart, new
lease streaming, and source-hash preservation. The fixture is stopped; its volume,
scripts, packet/hash and logs remain under `output/container-acceptance-9331722/`.

An isolated Windows GNU cross-compiler container reached actual Rust checks and
found two compile defects: a missing non-Unix directory inventory stub and unstable
Windows metadata identity calls in JSON export. The follow-up adds only an
Unsupported inventory stub and reuses the existing stable pinned-file/handle
identity helpers for exports. Verification fingerprints a freshly opened current
handle, preserves the created writer's cursor and sharing protections, and never
adds path deletion. An equal-byte replacement/cursor regression passes; the full
278-test runtime suite passes on macOS. The Windows runtime/test-target cross-check
passes with the exact synthetic JSON/PEM fixtures included. This is compilation
evidence, not Windows execution or installer acceptance. Independent review found
no actionable defect.

Native development-composition acceptance closes at public `0ffc262`: fast scan
refresh, visible collection create/rename/validation, keyboard focus fallback,
finished thumbnail job and actual synthetic slicer handoff pass without a license.
Same-output native regeneration leaves staging empty. The isolated app is stopped;
normal profile/install and official pin are unchanged. This closes the geometry/UI
checkpoint; the separate thumbnail/orphan provenance design is the next media
implementation gate. Known native purge/MainboardID prompt follow-ups remain.


### Native prerequisites closed at `9f64119`

Visible purge and ephemeral manual printer-ID dialogs pass actual isolated native
acceptance, including exact-token rejection/cancellation, verified synthetic backup
and previous Library, unchanged original sources, ID clearing and QA network denial.
Dialog styling and host/origin mapping defects discovered during testing are fixed.
See the parity plan for exact source, test/build evidence and transport limits.
The next media slice remains independent thumbnail publication/retirement proof;
these UI results do not authorize orphan deletion from flags or filenames.
