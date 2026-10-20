# Local bounded-constructor patch

Source: cached crates.io zip 8.6.0, upstream commit `771dfc534d2614158af5497ea3dff4d4208d7db1`.
Locked registry checksum: `2d04a6b5381502aa6087c94c669499eb1602eb9c5e8198e534de571f7154809b`.
Copied locally without downloading. Upstream MIT LICENSE and package feature declarations are retained.

Only the opt-in metadata constructor is bounded. Existing Config layout and new/with_config behavior remain unchanged.
Data-descriptor validation, member payload/inflate limits, mutable-source consistency and producer admission are separate gates.

Allocation charges are conservative cumulative requested-work bounds, not RSS claims.
Central raw fields are bounded before allocation. Name/comment decode+boxing is charged at 16 times input bytes;
validated extra fields at max(128, 32 + 2*sizeof(ExtraField)) times encoded bytes plus Unicode source-field processing. These cover raw/decoded
coexistence, lossy UTF-8/CP437 expansion (at most 3x), Arc/Box copies, processed-extra Vec growth/scratch,
Unicode replacements and structured extra-field vectors (each record has at least a four-byte header).
Record Vec and IndexMap capacities/key copies are charged separately before reservation.
Unknown/truncated extra-field length records are rejected by the bounded constructor before nested parsers can
allocate from their declared sizes; the ordinary constructor retains upstream behavior.
Footer comments/extensions use fallible bounded buffers and are charged before allocation.
All counters span every retry; exhaustion is sticky and cannot become a malformed-footer fallback.

Default policy: 10,000 physical central records across attempts; 16 MiB cumulative encoded metadata including local headers;
64 combined EOCD/ZIP64 candidates; 64 MiB actual metadata reads; 1,000,000 combined read/seek calls;
64 MiB conservative cumulative allocation work. Small fixtures can override these separate limits.
A 16 KiB fixed constructor allowance plus 4 KiB per candidate covers fixed search/error-format overhead.
The read-work adapter exists only during construction and is not retained for member payload reads.

No production caller has been migrated to this API. No descriptor or payload/source-consistency
guarantee is added by this patch. Tests under stl-core exercise the public opt-in API, not vendored
standalone/default-feature tests.

The bounded path currently requires the declared central-directory span to contain exactly the declared
central file headers/fields; unknown trailing central structures fail closed. The default constructor
retains upstream interpretation. Supporting an optional central-directory digital-signature tail, if
needed by a compatibility fixture, remains a separate explicit decision; no broad trailing-byte fallback.

Actual fallible allocator failures are operational, reported as fixed typed
`MetadataAllocationFailed` / OutOfMemory, not MetadataLimitExceeded. Both failure
classes are sticky across constructor retries. Checked size/capacity arithmetic
remains deterministic admission exhaustion. Bounded underlying read/seek OOM is
canonicalized to the same operational failure without exposing its source message.
The bounded path additionally requires consistent disk-zero, single-volume footers
and entries, resolving ZIP64 disk-start sentinels from captured extra data.

The opt-in constructor also admits each physical local header eagerly under the
same cumulative read/seek/encoded/allocation budgets. A temporary charged copy of
the original central wire name and raw method survives Unicode/AES interpretation
until the local name/flags/method and bit3-aware CRC/sizes have been checked.
Local extra fields stream through fixed scratch storage; no local source length
causes an unchecked buffer allocation. Actual local lengths and ZIP64 size fields
are bounded before use. Checked data starts are cached before returning the archive.
A fallibly preallocated, charged three-u64 range vector retains every physical
header/payload interval until an in-place sort rejects overlaps and aliases before
filename deduplication. Default new/with_config behavior remains unchanged.

Data descriptors remain unverified gaps: the admitted physical intervals end at
the central compressed payload end, not at a parsed descriptor end. The constructor
does not establish payload CRC, source identity or immutability. Later reads retain
cached starts and bounded central compressed lengths rather than reparsing changed
local lengths. Explicit unsafe metadata reuse retains its original caller boundary.
