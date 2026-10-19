# Verified generated transaction metadata

Status: primitive and shared native/runtime caller implementation passed independent review; final serial workspace, Clippy, formatting, and Community production build/signature checks pass; isolated launch and refreshed Linux image acceptance remain pending. This closes the remaining normal
purge/restore journal growth under the original-sources-only hold policy. It is
not a sweep of historical holds or build output and performs no manual cleanup.

## Boundaries

- Preserve generic source-hold primitives, original model files, previous Library
  directories/backups, settings/configuration, uploaded previews, and unknown bytes.
- Restrict the change to generated purge/restore journals, their `.tmp`/`.commit`
  scratch entries, and the purge staging nonce marker (currently 32 bytes, with a
  64-byte read bound).
- Use pinned parent/directory handles and admitted file identity plus exact bounded
  bytes/hash. Names, file size, valid JSON alone, or a fresh fingerprint of an
  arbitrary replacement do not prove generated ownership.
- Retain schema v1, operation nonce/root/sibling-name bindings, transaction state,
  durable recovery ordering, maintenance admission, and platform availability.
- Keep unknown or changed entries intact and fail closed. Unsupported native
  exchange/synchronization retains existing disabled behavior.

## Implementation slices

1. Add a separate generated-control-file receipt/helper in `secure_fs.rs`, reusing
   existing atomic exclusive publication/exchange. Admit app-created descriptors
   against their exact intended bytes. Recovery admission follows strict journal
   validation. Verify both post-exchange identities/content; only admitted displaced
   generated metadata may be permanently discarded. Deletion must atomically
   claim the public entry into an exclusive nonce-bound private cleanup name,
   verify the captured identity/content against the original descriptor, then
   unlink only that claim. Do not delegate to the existing public-name
   hash/check/unlink primitive: its final check-to-unlink window is insufficient.
   Do not create another generic
   replacement journal or change generic `replace_child`/`unlink_file` behavior.
2. Expose the narrow receipt/helper through `GuardedDirectory` and
   `GuardedDirectoryParent` for native consumers. Keep raw source-hold APIs intact.
3. Update runtime and native journal creation/publication, commit, clear, recovery,
   and purge-marker removal. Carry receipts across the operation; do not rediscover
   arbitrary scratch ownership from filenames. Recovery accepts only strictly
   bounded/parsed metadata matching the expected root, nonce, sibling names, and
   allowed prepared/committed forms.
4. Authenticate and clean scratch entries before removing the main authoritative
   journal. If cleanup or synchronization fails, retain recovery authority and
   maintenance state. Interrupted exchange may leave `.commit` holding either the
   prepared or committed form; recovery must handle both safely.

Private cleanup claims are part of the transaction recovery protocol. Names must
bind the expected journal/root namespace, operation nonce, and role; recovery
recognizes them with bounded strict validation, including a claimed main journal
when its public name is absent. Unknown or ambiguous claims are preserved. Receipt
bindings update immediately after successful rename/exchange/unlink, including on
later synchronization error; callers must never use an obsolete name to authorize
cleanup. A program actively discovering and replacing the private claim remains
outside the agreed non-malicious same-user boundary; no absolute protection is
claimed. A process already holding a writable descriptor can also mutate the
captured inode after its final hash check without learning the claim name. These
internal nonce-bound control files have one cooperating app writer under process
and maintenance ownership. This primitive must not be generalized to originals,
configuration, custom uploads, or arbitrary files that legitimately have other
writers; it provides no OS-level compare-and-swap unlink guarantee.

The final main-journal unlink is a finalization step after durable data decisions
and durable scratch cleanup. If its directory synchronization fails after unlink,
report failure and keep in-process maintenance armed, but do not promise that a
main file remains on disk. Restart idempotently handles any valid remaining public
or claimed journal; if neither remains, all data moves/rollback were already
finalized. Tests must establish that the current and previous Library data remain
intact in every finalization outcome.

## Verification

- RED/GREEN: successful native/runtime purge and restore create no new generated
  holds, generic replacement journals, scratch entries, or staging marker.
- Preserve previous Library backups and existing original-source hold behavior.
- Inject replacement before exchange, after exchange, and after cleanup hashing;
  also replace the public entry immediately after the last identity check and
  before destructive unlink. Preserve replacements and stop safely. Include
  parent-path swaps and malformed/ambiguous private claims.
- Inject interruption before/after exchange, before scratch cleanup, and after
  committed-marker publication; recover the correct Library deterministically.
- Reject unknown, malformed, oversized, symlinked, wrong-root/nonce, and unexpected
  scratch data without deleting it. Inject sync/unlink failure and retain the main
  journal until required cleanup succeeds.
- Independent review of primitive and caller contracts, focused tests, full serial
  workspace tests, Clippy/formatting, and current app builds before advancing.

No historical hold deletion is authorized by this implementation plan. Existing
artifacts remain preserved unless a separate exact cleanup approval applies.

## Separate generated-media follow-ups

The synthetic container scan exposed another normal-growth path: the default
thumbnail renderer staged an already verified model buffer as a temporary source,
then retained that generated copy through generic source-hold cleanup. A separate
regression/fix removes that staging step and renders from the verified buffer.
No existing hold is removed.

Different thumbnail replacement still requires proof distinguishing app-generated
cache output from unknown or custom bytes. PNG validity or a cache-like filename
is insufficient. Uploaded/custom previews are excluded. Generated geometry lease
retirement also needs a typed provenance-aware receipt; source-download, upload,
and archive-member staging are not authorized derivative cleanup merely because
they are temporary. These are separate follow-ups, not grounds to broaden the
control-metadata API or sweep existing holds.

## Review outcomes

The primitive's 14 focused recovery/race tests pass. Runtime journal-cycle and
thumbnail no-growth regressions reproduced the old retention before their fixes.
Independent implementation review found no remaining actionable findings after
adding native helper accessors and correcting an error message that had promised
metadata preservation even after final unlink. Scratch cleanup errors propagate
before main finalization, including errors whose receipt is already `Unlinked`.
An old test expecting arbitrary partial temporary-journal deletion now requires
preservation and failure closed. The final serial workspace suite passes under the isolated macOS temp parent,
including 201 native, 120 catalog-core, 254 runtime, and 62 server tests. Workspace
Clippy (existing warnings), formatting, and diff checks pass. The optimized
Community app builds and passes deep strict signature verification; executable
SHA-256 `202eecf5079d48cbc3d330f1f631251caffb079f12191b386a4b4acbb8454d31`.
Isolated launch and updated Linux container acceptance are the remaining package
checks for this slice.

### Bounded parent scan and test isolation

Recovery examines at most 4,096 parent entries and 16 namespace claims. A Library
whose parent exceeds that entry bound fails closed, even without a visible public
journal, because an interrupted claimed journal may exist. Keep Libraries under a
dedicated parent. This is a deliberate availability limit, not permission to
ignore claims or delete unrelated entries.

The first final workspace run hit that limit in 12 native tests whose Libraries
were direct children of the system temp directory (16,110 existing entries).
The same failed test passes unchanged under a new dedicated synthetic `TMPDIR`.
The workspace-local isolated parent passed all 201 native tests, but one existing
macOS VFS alias test requires its fixture path under `/private/var`. Final workspace
acceptance therefore uses a fresh dedicated subdirectory of the macOS temp root,
preserving both requirements. Existing system temp files are untouched; the
4,096-entry bound and all test assertions are unchanged.
