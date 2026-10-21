# Thumbnail parser resource prerequisite

## Current implementation: funded private parser maps (2026-10-21)

`ModelPart.objects` and the package part cache now reserve every hash-table
bucket through the package's existing `Owned` funding host. The allocator receives
hashbrown's actual bucket `Layout`; the charged block includes an aligned credit
header and all bucket/control bytes. `try_reserve` runs before insertion, and a
funding refusal is latched as the existing retryable `FundingBusy` cause before
the collection error is returned. Object and part counts still use the existing
physical admission rules; this adds no quota.

Rehash keeps the old bucket block charged while the new complete layout is
acquired. The returned slice matches the requested layout exactly. Deallocation
frees the combined block before its credit is refunded. `ModelPart.objects` is
sealed before the part enters its `Rc` cache; sealing drops parser observer and
host handles from the map control while each retained block keeps only the
funding host needed for its eventual refund. The mutable package cache dies with
the package. Both maps retain the existing `RandomState` hasher and borrowed
`SharedPath` lookup behavior. Source vectors still control emitted order.

`hashbrown` 0.16.1 and `stl-core` resolve the pinned crates.io
`allocator-api2` 0.2.21 package. Sharing that registry package identity keeps
the allocator trait type consistent when `stl-core` is consumed from a parent
workspace. Only its `alloc` feature is enabled, and `Cargo.lock` records the
resolved checksum. Its upstream MIT/Apache license texts remain included in
the generated notice inventory.

Focused tests prove actual host denial at both map growth sites, full old/new
layout overlap, deallocation before refund, and null-allocation accounting. The
normal non-test artifact was also exercised with one and 32 empty objects: the
larger map issued more host admissions, reached a 7,240-byte peak, and released
all acquired bytes after parsing. These requested-byte observations do not bound
allocator size classes or process RSS. All 258 `stl-core` unit tests pass, with
one existing ignored test; workspace Clippy, formatting and whitespace checks
pass. The GoggleLab library target passed 207 tests, while four unchanged
`folder_watch` event tests timed out, including a standalone rerun on this macOS
host. Using a task-specific temporary directory cleared other initialization
failures caused by the shared test directory's 16,882 entries exceeding the
existing 4,096-entry recovery bound. The offline composed-consumer check now
passes for all 374 public inputs. It preserved 1,410 unaffected candidate
entries and all 112 original directories, then restored the private root lock
byte-for-byte after adding only the pinned registry allocator. Hosted workflow
and current-source container acceptance remain open for this phase.

Other settings/material maps, `Rc` control blocks, semantic strings, public
output/geometry backing, other parser routes, SDK/native frames, producer sizing
and immutable publication remain open. No 16 MiB policy or producer profile is
selected. Evidence is preserved in `output/owned-private-maps/`.

## Accepted checkpoint: four private input buffers (2026-10-20)

Source vertices, source triangles, components and build items now use one unique
owned buffer implementation. Each growth funds the complete new header/payload
layout on the existing host ledger while the old block remains charged. Elements
move without cloning; a provisional block owns no elements until the infallible
ownership transfer. Cleanup drops the initialized prefix and frees the block
before refund, including when an element destructor unwinds.

All four private fields, construction/push sites and borrowed consumers migrate
together. Existing physical ceilings, cumulative counts, eight reserve-site
probes and validation priority remain in place. Temporary funding refusal stays
typed through the existing observer. Cached ModelPart Rc owners retain backing
and credits past package/context drop. Public model fields, cloning, field moves
and native Arc shapes remain unchanged. The exported layout diagnostic is
read-only; it provides no allocation, funding or policy override.

Eight parent-observed behavioral failures preceded implementation: four runtime
requests reached a fitting packet without input-buffer funding, and four core
tests proved the exact input reserve site never called the host. Those tests now
pass. Core verification passes 354 non-doc tests and two doctests (one existing
ignored fixture); five runtime checks cover healthy and denied Stored/Deflate
fixtures for every category. Seven owner tests cover growth overlap, null
allocation, ordered stops, non-Copy relocation, destructor unwinding, capacity
bounds and alignment. Independent owner and normal-probe reviews are recorded.

The final normal artifact passes twelve real parses: success, Busy and one
targeted null allocation for each actual private type. On the verified toolchain,
vertex/triangle elements occupy 12 bytes, component/build elements 64 bytes, and
the heap header 40 bytes. Capacity128 requests are 1,576 and 8,232 bytes,
respectively. The trace verifies old/new overlap and physical free before refund;
distinct child geometry verifies component path use after growth. These are
local requested-layout observations, not public ABI, allocator/RSS or private
field/refcount introspection. The unchanged preserved corpus also passes all four
1024-part Stored/Deflate fixtures, retaining borrowed rows and the 20,635,304-byte
legacy-copy lower bound. The probe driver, four fixtures and all 153 compiled
inputs remained hash-identical during these checks.

Final serial workspace verification passes with 1,219 top-level tests and one
existing ignored test: 392 runtime, 211 native,
65 server-library and five CLI tests, alongside the core results above. The
Clippy has exactly the same 46 actionable diagnostics as the accepted baseline,
with no additions or removals; formatting passes. The 153 compiled Rust/Cargo
inputs stayed unchanged throughout each final check.

Maps, Rc controls, semantic strings, public output/geometry backing, other parser
routes, SDK/native frames, producer sizing and immutable publication remain open.
No 16 MiB policy or producer profile is selected. Verification evidence and the
preserved first compiler/probe findings are in `output/owned-input-buffers/`.

## Accepted checkpoint: shared path funding (2026-10-20)

The selected-model geometry route creates a funded context before verified-source
acquisition. An indexed archive's hidden validation and the visible model parse
use that same context. Internal root/reference paths, model-part paths, cache keys
and diagnostic path sets use immutable shared backing. The complete requested
header/payload layout is admitted before its allocation; clones share that block,
and its final owner deallocates it before returning the credit.

Admission competes with existing Owned, Geometry and Bundle reservations on the
same runtime state and current limits. It introduces no separate allowance.
An accounting underflow persistently marks that state uncertain and denies later
admission in those categories; a later refund does not reopen authority. Upload
accounting remains separate. The funding handle retains state and limits, without
retaining the runtime. Its once-created control allocations are explicit bootstrap
terms. Retained-file inventory refresh occurs at operation entry, not per path.

Funding refusal uses the named operational `FundingBusy` category through the
existing single first-cause observer. It maps to retryable runtime Busy and is
never ordinary malformed metadata, allocator failure or a deterministic input
limit. Adding variants to exhaustive `StlError`, `ZipOpenError` and
`ProcessingError` enums requires external Rust exhaustive matches to be updated;
unchanged result signatures and wire fields do not remove that source impact.

The public compatibility context remains explicitly unfunded. Public output
strings, maps/array capacities, SDK scratch and controls, independent bundle
previews, other parser routes and native response ownership are not covered by
this path-backing increment. It selects neither a 16 MiB metadata policy nor a
producer profile and does not prove allocator/RSS bounds. Test instrumentation
enlarges the owner header; normal-artifact allocation evidence is tracked
separately. Verification and review records are in `output/owned-metadata-service/`.

Parent-observed behavioral failures preceded both the admission implementation and
the persistent-underflow fix. Final serial workspace verification passes 339
non-doc core tests plus two doctests (one existing ignored fixture), 387 runtime,
211 native, 65 server-library and five CLI tests. All 152 compiled Rust/Cargo inputs
remain frozen. Stored/Deflate indexed sources reject funding during hidden
validation before the visible parser/derivative, preserving source and catalog.
Independent path, allocation-safety and runtime reviews are closed.

A newly compiled normal-artifact probe observes a 40-byte header, eight-byte
alignment, and a 72-byte requested block for a 29-byte path on this toolchain.
The last clone outlives its context; actual system deallocation completes before
credit release. A targeted null allocation returns AllocationFailed with one
refund and no null deallocation; Busy performs no targeted block allocation.
These local layout observations are neither public ABI nor allocator/RSS bounds.

Workspace Clippy exits successfully with 46 inherited actionable diagnostics:
45 retain identical source identities; one existing eight-argument warning has
only the required String-to-SharedPath signature-span change, independently
matched exactly. The strict raw delta and supplemental proof are both preserved;
no warning suppression was added. Formatting and whitespace checks pass.

The unchanged normal-artifact corpus driver also passes all four preserved
long-path/distinct-label Stored/Deflate fixtures, with 1,024 parts, materials and
triangles and the original borrowed row/string identities. Its result log matches
the prior accepted run. Explicit public owned-copy controls still exceed 16 MiB;
the new private path ownership does not remove that separate output-copy term.

## Accepted checkpoint: metadata projections (2026-10-20)

A validated borrowed plate-part view is shared by the runtime and native summary
consumer. It checks every range before yielding, keeps source ordering and the
existing inclusive zero-length boundary behavior, and preserves missing-plate,
partial-overlap and outside-range overflow results. The existing owned
`plate_parts` API delegates to the same view and remains source-compatible.

The runtime consumes its exclusive parsed result after packet preparation,
moving original part and plate strings into response rows. Whole-model requests
keep their existing validation behavior. Native summaries read borrowed rows
directly, avoiding an intermediate owned part vector; their final public string
copies remain explicit. The original cached model `Arc`, public JSON fields,
packet/source/charge handling and source bytes remain unchanged.

Two actual runtime regressions first preserved packet/row/source/no-staging
assertions and then failed because final nonempty strings had copied backing.
Both now pass with the original backing retained. Focused verification passes six
core model tests and eight runtime geometry tests. Final serial workspace checks
pass 326 non-doc core tests and two doctests (one existing fixture ignored), 378
runtime, 210 native and 65 server-library plus five CLI tests. The native checks
include exact public JSON, invalid-selection behavior and the real cached model's
original Arc identity. All 150 compiled inputs stayed frozen. Workspace all-target
Clippy retains all 46 existing actionable diagnostics with zero new diagnostics,
no suppression and no exception; formatting passes. Core/runtime and native/API
reviews found no blocking issue.

A newly compiled driver linked to the current normal core artifact passes all four
preserved long-path/distinct-label fixtures, Stored and Deflate. Each retains
1,024 triangles, parts and materials and original borrowed row/string addresses.
Actual local iterator/view control sizes are 32/16 bytes. Selected field capacities
are recorded separately; they are not total metadata or allocator/RSS accounting.
Explicit legacy owned-copy controls still duplicate the 10,317,652-byte selected
field subtotal to 20,635,304 bytes, so they do not fit a 16 MiB policy. Required
source-bound composed, hosted and immutable server gates have separate records;
no older source revision's results establish this checkpoint's acceptance.

This is a representation prerequisite for complete live-capacity funding; it does
not enable the proposed 16 MiB policy, a producer allowance or a new transport.
Native public response copies, SDK/control/frame ownership, whole-result
retention and immutable publication remain open.

## Accepted checkpoint: settings sidecar lifetime (2026-10-20)

Model-settings text is released after the existing slicer scan and compact plate
preparation, before project-settings text is collected. Plate preparation retains
owned names/indexes and a mandatory pending result. Final source-dependent child
checks and plate emission remain after project handling. Each settings sidecar is
collected once per parser invocation; inflation and cumulative collected-input
accounting remain unchanged.

A private reader over the collected immutable slice separates fixed XML validation
from actual operation failures. Fixed plate-validation results activate at the
original plate phase, preserving project/plate diagnostic priority. Actual source,
workspace, cancellation and fallible acquisition failures use the original
observer immediately. Newly earlier acquisition failure is an explicit effect of
preparing plate data earlier. Retained plate/default names reserve fallibly;
ordinary structural short circuits, duplicate/membership priority, names, geometry
and palette behavior remain covered. SDK normalization/error-allocation residuals
remain separate.

The original Stored regression preserved fields and usage before failing because
the actual model-settings String was still live when project collection began.
Both compression methods now pass. Seventy-one focused tests cover the revised
sidecar family, XML behavior, existing derived controls and public settings/maps;
the three default-name tests are included in the thirteen-test sidecar family.
Final serial workspace verification passes, including 324 core tests and two
doctests (one existing ignored fixture), 375 runtime, 209 native and 70 server
checks. Workspace all-target Clippy retains the three existing diagnostics across
the changed files with no new warning or suppression. Complete source/log hashes
and the preserved first failures are under `output/sidecar-sequencing/`.

The first workspace attempt used the shared macOS temporary parent and lacked
native event access. Controlled same-binary comparisons identified the existing
4,096-entry recovery guard and native watcher access as the two causes; verification
uses the established isolated temporary directory and confirmed native access.
Product guards, assertions and deadlines were preserved. This unit is accepted
at source commit `1daced7fb76bd13c0c18e7ae19869033388d0744`: the disposable
composed application check passed; both fresh hosted workflows passed, including
macOS packaging, Linux headless tests/build and Windows GNU compilation; and all
eleven synthetic container checks passed with 76 unchanged source models. The
container includes eight new Stored/Deflate sidecar cases, exact geometry/field
assertions and 550 Cargo / 37 npm license entries. Its runner stopped only the
image-bound task container and preserved all evidence and data. Independent
publication, hosted and final container reviews found no actionable issue.

Container attempt 1 stopped before compilation because the sandbox denied Docker
buildx activity bookkeeping. A separately reviewed explicit attempt 2 used the
same bound source/command with local permission and passed; the first failure
remains preserved. Both live and extracted 370-file source sets matched before
that attempt, and the extracted context matched after building. The actual image
is `sha256:0beb630adc367b05f53e154fb1bf15844fc5df3c621d6f56119558135acd12ee`.
See `output/sidecar-sequencing/hosted/review.md` and
`output/container-acceptance-sidecar-sequencing/final-review.md`. Hosted Windows
cross-compilation does not close native Windows acceptance; synthetic headless
Linux does not close native Linux desktop acceptance. No cleanup was performed.

This removes simultaneous raw model/project sidecar backing. Full live metadata
ownership, internal snapshots/response lifetimes, SDK/control layouts, producer
admission and immutable preview publication remain open. The proposed 16 MiB
metadata target remains unselected; existing input collection is a separate metric.
No source mutation, networking, cache cleanup or new allowance follows from this
prerequisite.

## Slicer settings retention checkpoint (2026-10-20)

Settings maps retain records only for root object IDs and the diagnostic map's
direct-component target IDs, the existing consumers' relevance superset. Object
and part relevance are independent: useful part settings beneath an irrelevant
object survive. Irrelevant open objects/parts still occupy scanner state;
ignored nested-part depth, invalid IDs, Empty tags, permissive nested objects,
End-event merging and EOF behavior remain unchanged. Completed records survive
bare EOF; unfinished records are not merged. Actual ordinary reader/scanner
errors still discard optional settings while preserving geometry.

Only relevant records obtain retained name backing and map entries. Names retain
trim/256-Unicode-scalar behavior; duplicate IDs merge complementary fields, while
equal repeated/invalid fields remain permanently invalid. All attribute lookups,
including unknown keys and irrelevant records, preserve existing first-error and
first-local-name selection. Existing ignored-depth paths remain undecoded.

Name backing and missing-key object/part map growth reserve fallibly, with
pre/post acquisition checks on the same observer. Private typed terminal errors
cross the actual optional caller before ordinary fallback. Retained settings
parsing does not recharge package text usage. The relevance bounds follow already
admitted root objects/direct-component targets; no new arbitrary-ID quota or
stricter XML hierarchy is introduced. Temporary relevant-record names and SDK
attribute key/value normalization scratch remain separate contributors.

Two compiled RED tests first preserved geometry and consumed fields, then
observed old copy/object/part tuples `(3,3,0)` versus `(1,1,0)` and `(4,2,3)` versus
`(2,1,1)`. Those failures occurred on Stored inputs before reaching the Deflate
iteration. Current GREEN executes both methods: six local controls include 48
actual-site before/post checks across three reserve sites, four terminal causes,
first-cause/fallback/commit checks and shared-context accounting. Seventeen public
controls (18 concrete XML inputs, Stored and Deflate) passed on the unchanged
scanner and on the new path. Full parser verification passes 307 tests and two
doctests, with one existing ignored fixture. Workspace all-target Clippy passes
with the same two changed-area diagnostics and no new warning or suppression.

Full serial workspace tests pass, including 375 runtime, 209 native and 70
server tests, with all 150 frozen Rust/Cargo inputs unchanged; see
`output/settings-relevance-workspace-results.json`. Exact downstream compilation
also passes with all 370 public inputs matching and all 1,333 pre-Cargo entries
preserved. Hosted/container checks have separate immutable provenance records. Actual evidence
is under `output/settings-relevance-*`; the frozen verification covers all Rust
sources/Cargo manifests/locks and both new owned files. Count/relevance and named
fallible-growth controls do not complete live owned-text/snapshot lifetimes,
SDK/allocator overlap, producer admission or immutable preview publication.
Those and final platform/release gates remain open.

## Standard material acquisition checkpoint (2026-10-20)

Standard base/color material rows keep the existing 1,024 accepted distinct
(group ID, source index) limit in each parsed model part. Every matching physical
row advances its group's index, including invalid, duplicate and clipped rows.
Invalid earlier keys remain available to a later valid group; accepted duplicates
keep the first row. Global display retention separately selects only the first
remaining 1,024 rows in part-load order. A full global list does not prevent child
parts from retaining their own local rows, and existing dangling-ID suppression
still applies.

Classification preserves the original first attribute-iterator error and first
matching local name. It borrows normalized Cow values where possible and validates
RGB/RGBA into a seven-byte stack color. SDK normalization can own bounded scratch;
that remains separate from controlled retained-field allocation. Local duplicate
and row admission precede final owned name/color/ID construction. Row vectors and
lookup buckets reserve fallibly; IDs use checked exact extent before formatting,
names keep trim/256-scalar behavior, and alpha remains discarded as before.
Selected global rows copy ID/name/color through fallible field builders instead
of ordinary Clone. The same observer preserves known allocation/resource stops,
including a failure reached while checking an unassigned child's geometry.

Three actual RED tests first preserved geometry and retained appearance, then
observed 1,025 name/color constructions for 1,024 accepted rows or three for one
duplicate key. The old global selection was already correct and is not claimed
as an excess-copy RED. Five public Stored/Deflate controls passed on the old path
and the new path. Final frozen core verification passes 284 tests plus two
doctests, with one existing ignored fixture; eight local material tests include
all nine before/post-reserve sites and the deferred-child terminal-error case.
Full serial workspace verification also passes 375 runtime, 209 native and 70
server tests with unchanged frozen source hashes; see
`output/material-retention-workspace-results.json`. Workspace all-target Clippy
passes with no new changed-area diagnostic: four existing diagnostics become two,
compared by code, message and exact primary source text without suppression
(`output/material-retention-clippy-results.json` and
`output/material-retention-clippy-diagnostics-audit.json`).

These results were executed before the comment-only history rewrite. Current
material source/test bytes still match the tested freeze; the only source-hash
changes are eight approved comments in three consumer files. The evidence bridge
is separate in `output/material-retention-after-history-handoff.json` and
`output/material-retention-post-history-review.md`, rather than a claimed new test
run. The committed material checkpoint `17f8b6e` subsequently passed exact
368-input downstream compilation, both hosted workflows and Community packaging,
and nine synthetic container check stages. The original material fixture-warning
assertion failure remains preserved; the reviewed correction required the exact
existing whole-build warning without rebuilding, restarting or rescanning. See
`output/material-retention-checkpoint.json` and
`output/material-retention-hosted-review.md`.

This bounds existing row/lookup acquisition and selected field copies. It does
not impose 1,024 materials across the entire package, recover every SDK/std/Rc
allocation, bound all names/paths/cache/output clones, or select the proposed live
16 MiB ownership policy. Slicer-settings maps, aggregate live ownership, producer
admission and immutable publication remain open. No cleanup authority follows.

## Derived index and plate-retention checkpoint (2026-10-20)

One fallible build index now supplies physical occurrence numbers to streamed
plate membership, authored emission and whole-build fallback. Disabled build
items keep their occurrence slots. Index cardinality derives from the actual
admitted root build, rather than an XML length hint or a new metadata row quota.
Only unique resolved source indices are retained across all plates, so retained
assignment count cannot exceed that build's length. The first unknown or duplicate
assignment drops the unusable resolved output and its backing; parsing continues
only to preserve the old structural-validation boundary.

Streaming structural errors still return immediately. At EOF, open/no-plate
checks precede deferred duplicate plate IDs, which precede membership errors.
The independent completed-plate ordinal preserves the existing 256-plate close
check before required-ID validation. A fixed 256-ID stack preserves deferred
ID checks without a heap set. Unclaimed geometry is checked only after structural
and assignment validation, retaining the existing source-loading order.

Diagnostic paths use the existing leading-slash normalization as a borrowed
lookup before copying each new object/path pair. Existing component-work debits
remain. Controlled vector, map, set and path growth is fallible; the same
observer's typed stop remains terminal, including a stop observed while parsing
after invalid membership. Plate output moves existing names instead of cloning
those names into another plate vector.

The genuine RED run retained three assignments for two build members and copied
four diagnostic paths for one normalized pair, after geometry/warning assertions
passed. Final frozen core verification passes 271 tests and two doctests, with
one existing ignored fixture. Eight local tests cover the causal regressions,
all ten reserve sites, post-reserve first causes and actual plate-phase tail
handling. Five public Stored/Deflate controls preserve structural error priority,
plate limits, disabled numbering, empty plates and authored order. Full serial
workspace verification also passes 375 runtime, 209 native and 70 server tests,
with frozen source hashes unchanged. Workspace Clippy has the same four
changed-area diagnostics as the previous checkpoint, compared by exact code,
message and primary source text; no suppression was added. Composition and
packaged results remain separate gates under `output/derived-maps-*`.

These are cardinality and controlled-growth bounds, not an exact aggregate
allocation/RSS bound. Geometric spare capacity and map bucket layout remain
separate contributors. Material acquisition is covered above. SlicerMetadata maps, other owned strings,
public snapshots and clone lifetimes, the proposed live-metadata policy, producer
admission and immutable publication remain open. No new producer allowance,
source mutation or cleanup authority follows from this checkpoint.

## Optional part-row retention checkpoint (2026-10-20)

One private accumulator now retains at most the existing 1,024 optional viewer
part rows across the entire build, including all authored plates. Geometry is
always emitted and validated first. A further nonempty boundary permanently
omits part metadata and drops its retained rows/text/backing, preserving the exact
existing part warning and all geometry. Empty, disabled and unprinted candidates
at the limit do not cause omission. Final range/coverage validation still handles
ordinary unusable boundaries; plate ranges, display order and warnings keep their
independent behavior.

Optional row-vector growth is checked, capped and fallible. IDs and names are
constructed only after a nonempty candidate receives a reserved slot. Known
reserve failures and post-reserve stops preserve the same observer's first cause;
they cannot become ordinary optional fallback. At capacity or after omission,
appearance observation uses its existing nonuniform state to suppress optional
material-ID copies. Below capacity, one in-flight Uniform String can still be
created during geometry emission before row admission. That scratch and other
String constructors are separate allocation/lifetime contributors.

Both public-parser RED cases retained three rows/built three names under a private
two-row profile after geometry/warning assertions passed. Final frozen core
verification passes 260 tests plus one existing ignored fixture: nine local
retention/fault controls and four independent public Stored/Deflate cases cover
exact limits, global authored-plate overflow, late errors and geometry preservation.
Full serial workspace verification also passes 375 runtime, 209 native and 70
server tests. Workspace Clippy introduces no new diagnostic; its existing
eight-argument emitter warning changes only the part-output parameter type.
Downstream and packaged results remain separate gates in `output/part-retention-*`;
these local results alone do not close them.

The row limit bounds retained row count and requested vector growth, not all
String payloads, allocator capacity/overlap, public Clone lifetimes or process RSS.
Derived indices and diagnostic paths are covered by the checkpoint above;
material containers, aggregate owned metadata, producer admission and publication
remain open. No
source mutation, cleanup or new producer allowance follows from this change.

## Physical input-record storage checkpoint (2026-10-20)

The parser enforces fixed per-Package limits of 6,000,000 vertices,
2,000,000 source triangles, 10,000 objects, 100,000 components and 100,000 build
items. These are new supported-input restrictions: records in unused, unprinted,
duplicate or discarded definitions count within every actually parsed model part.
Root and reached children share the counters; a cached child counts once. This
policy does not read otherwise-unreferenced ZIP entries or disabled targets.

Existing structural prerequisites precede admission. Admission then precedes
caller attribute extraction and destination growth; an over-limit record is an Input resource
failure even if its scalar fields are also malformed. Under-limit malformed and
transform behavior, mesh/component selection and deferred emitted-coordinate
finiteness remain unchanged. A fresh Package starts fresh physical counters;
shared context read/inflation work remains cumulative across independent parses.

Source vectors use checked, capped geometric reservations before appending, and
object/part maps reserve fallibly for new keys. Positions and normals both reserve
before either output vector appends. Transform parsing uses twelve stack values
while validating every tail token, preserving ordinary error priority. Concrete
reserve failures and deterministic admission stops preserve the same observer's
first cause. HashMap bucket layout and fixed Rc control-block allocation remain
allocator residuals; this does not promise universal allocation recovery.

Five causal regressions failed on the previous accepting branch. Final workspace
verification passes 247 core tests (one existing ignored), including 18 local
admission/fault tests and six independent public Stored/Deflate compatibility
tests; 375 runtime, 209 native and 70 server tests also pass. Exact downstream,
hosted and container status must be read from `output/input-collections-*`; these
local results do not imply those gates passed.

The physical limits do not bound all owned strings, map buckets, spare/reallocation capacity,
simultaneously live output, SDK/decoder workspaces or process RSS. Optional part
rows and derived indices/diagnostic paths are covered above; material containers
and aggregate owned metadata remain later work. No producer allowance or
publication authority follows from this policy selection.

## Current standard-field retention checkpoint (2026-10-20)

Runtime standard embedded metadata retains at most one value for each of five
fields. A fixed borrowed-name selector replaces owned name/lowercase intermediates;
unknown or occupied selections still decode text before declining retention.
Selected nonempty values reserve their admitted byte length fallibly, recheck the
same observer, copy once and move into the empty destination. A final observer
check dominates tentative output and ordinary errors. Public strings, aliases,
XML event order, strict malformed handling and source/context accounting remain
compatible.

This gives at most five controlled retained-value backing allocations and
5 × 65,536 = 327,680 logical payload bytes per returned source-field set, derived
from existing XML acquisition bounds. It does not bound all SDK allocations,
allocator capacity/RSS, catalog/current metadata or other simultaneously live
outputs. Borrowed UTF-8 text decoding is tested for the checked normal quick-xml
profile; attribute normalization may still own a bounded SDK string. No proposed
16 MiB live-metadata target is reinterpreted as cumulative copying, and no context
return refunds a new ledger because this unit creates none.

The original 1,205-copy regression now passes, with nine focused compatibility,
allocation, first-cause and real proposal tests. Workspace Clippy has no new
changed-code diagnostics. Broader unit status is recorded in
`output/standard-fields-implementation-gates.json`. Global owned metadata/model
capacity, producer admission and publication remain separate open gates.

## JSON checkpoint `490d2a1` (2026-10-20)

Project metadata is parsed by a private streaming visitor after an allocation-free
scan of its already retained input. The existing 8 MiB project role bounds wire
and decoded token spans; a fixed 127-frame container stack preserves the default
Serde recursion boundary. Malformed/deep optional metadata still falls back.
Decoded duplicate keys remain last-wins, wrong-type replacements clear earlier
values, invalid colors retain their positional holes, and all ignored/tail values
and trailing document bytes undergo normal semantic validation. `IgnoredAny` is
not used because its number/depth behavior differs from the former DOM.

Each of four field prefixes has at most 256 positions. Color slots store seven
ASCII bytes; auxiliary labels retain at most 256 Unicode scalars / 1,024 UTF-8
payload bytes. Prefix/result vectors reserve at most 256 entries; controlled
strings allocate once for their clipped payload. Final labels move where possible;
joined names preserve trim-before-clipping without a full concatenation. Known
controlled reserve failures latch the same observer. The actual caller propagates
terminal failures before cached-root fallback and does not recharge input or
inflation for this in-memory parse. The DOM and full auxiliary arrays remain only
in the test oracle.

The test-first prefix regression and all 223 core tests pass (one existing ignored
fixture), including 18 new JSON controls. Actual public package tests cover Stored
and Deflate, UTF-8 failure, duplicate keys, recursion, positional boundaries and
exact accounting across two passes. Verification status beyond core is recorded
in `output/project-json-implementation-gates.json`; it must not be inferred from
these results.

Pinned serde_json 1.0.151 uses one reused escaped-string scratch buffer. Under the
reviewed default-number features and versioned Vec growth rule, admitted 8 MiB
input gives logical scratch <=8 MiB, requested steady capacity <16 MiB and possible
moving old/new backing overlap <24 MiB. Those conditional profile estimates exclude
retained input, outputs, stack and allocator overhead; they are not hard RAM/RSS
limits or a recoverable SDK allocation promise. Actual normal-unit features must
be correlated for each checked profile. Whole-owned metadata, mesh/map storage,
producer allowance and publication remain open.

## Earlier STL/runtime checkpoints (2026-10-20)

The STL correction follows shared-runtime checkpoint `743c863`. Both binary and
ASCII parsing now enforce the existing 2,000,000-triangle policy before output
growth. Exact-size binary detection still precedes ASCII fallback; malformed and
nonfinite ASCII facet errors keep their previous order, and stored nonfinite
normals remain supported. Binary reservations are checked/fallible; ASCII stages
one complete facet on the stack and reserves both capped output vectors before
appending either. Known allocation failures use the same operation observer.
Legacy `TooManyTriangles` count/limit and native error shape remain unchanged.

Three real RED regressions failed before the fix (binary accepted two under a
private one-triangle profile; ASCII appended 18/6 floats before refusing). The
corrected core suite passes 205 tests plus one existing ignored fixture, including
15 new admission/allocation controls. Full workspace regressions pass. Evidence:
`output/stl-before-growth-implementation-gates.json`; exact hosted verification
is tracked in [PR #4](https://github.com/cubiclejuice/gogglelab/pull/4).

The current runtime integration follows core checkpoint `128da5e`. Shared mutable
package contexts now span archived-source validation and later geometry, metadata,
preview and viewer/bundle loading. Runtime model ZIP reads use the actual inflation
budget once; ordinary file/readback collection uses an exact source envelope with
no fictional inflation charge. Scan member limits remain clamped to the narrower
configured policy and the fixed 100 MiB model cap. Actual policy excess is terminal
Input; changed exact length/hash is Envelope/Stale; ordinary CRC remains corruption.

Bounded model ZIP construction/local ranges and terminal propagation were already
wired at `7e6326b`: all seven model/manual readers use the reviewed facade, while
backup restoration keeps its separate format policy. Default ZIP APIs remain
unchanged. Runtime metadata now streams root XML instead of retaining its complete
buffer; images use fixed 16 MiB binary input and 64 KiB retained relationships.

Local full-workspace regressions pass: 205 core tests plus one existing ignored,
366 runtime, 209 native, 70 server and other suites. A real reset control fails when
hidden validation receives a fresh counter with the same observer, then passes
after exact restoration. Independent plan/collector/caller reviews are closed.
See `output/runtime-context-implementation-gates.json` and
[PR #4](https://github.com/cubiclejuice/gogglelab/pull/4) for current verification.
Earlier `128da5e` actual public-server image acceptance includes 550 Cargo and 37 npm
notices with exact ZIP MIT text; the previous disposable composed Cargo inventory
contains 558 Rust components. Neither establishes fresh final unified-app packaging,
its complete source-kit/notices, Windows execution or hardware acceptance.

The opaque XML facade bounds wire/event acquisition and successful raw/recovered
values before caller normalization: 64 KiB tokens, 4 KiB values, 128 attribute
items, depth 128 and 256 KiB live raw names. Its actual SDK admission iterator is
bounded to 129 inspections (the last distinguishes overflow from None); PI/XML
declarations are token-only scope. The 16 KiB temporary attribute-workspace
estimate was re-reviewed against actual macOS Rust 1.99.1/hashbrown 0.17.1 and
quick-xml 0.41.0. It is a versioned profile estimate, not a portable hard cap,
fallible SDK-allocation guarantee, RSS bound or completed Owned permit.

## Remaining allocation and integration boundaries

- Core `threemf.rs` now charges actual returned model bytes through one mutable
  context; it no longer sums declared entry sizes. Context collection is bounded
  before growth, settings are retained once/reparsed without another inflation
  debit, and ordinary EINTR is retried. Its 16 MiB counter is cumulative collected
  **INPUT**, not every owned name/path/normalized metadata copy. Physical input mesh/object/
  component/build collections now use the per-Package limits above. Remaining derived
  containers and aggregate owned metadata remain OPEN. The JSON
  DOM/auxiliary-array correction is described above; its local bounds do not close
  whole-parser capacity or the flattened-output overlap.
- `parse.rs` binary/ASCII output admission and checked/fallible growth are active.
  Their two vectors retain the existing 9/3 floats per accepted triangle. This is
  not a whole-parser/producer memory cap or an immediate pure-decode cancellation
  guarantee; SDK float parsing, capacities, reallocation and pipeline overlap
  remain inputs to the later profile.
- Runtime `archives.rs` has bounded ZIP/typed pure-read-before-stage handling,
  regular-source/1 GiB outer admission, one actual outer budget across scan members,
  and before-growth member collection. Nested 3MF budgets are separate from outer
  ZIP work, share the terminal observer, and survive subsequent operation passes.
- Runtime images/embedded metadata now use the opaque XML facade, bounded retained
  relationships, binary image collection and streamed root-model metadata. Native,
  catalog/default jobs, geometry, bundles and cleanup proof use context adapters.
  Remaining owned metadata/candidate copies and full decode/producer overlap are OPEN.
- PNG output is already capped at 4 MiB with explicit finish and a known sink
  allocation category. Decoder/compressor workspace and full producer overlap are
  not established by that encoded-output limit.

No source/historical-cache cleanup or publication is enabled by these parser
changes. The existing Owned transfer retains the configured ledger; it does not
establish a defensible producer allowance. The two-slot/2 GiB proposal remains
unproven and disabled until the remaining bounds/profile/admission gates pass.

## Active and proposed finite limits

The table distinguishes active input policy from proposed/open allocation bounds.
Use a private test-limit override to exercise boundaries without large fixtures.

| Resource | Initial cap | Enforcement point |
| --- | --- | --- |
| ZIP central records / metadata envelope | 10,000 / 16 MiB | Active model facade: intrinsic constructor checks across retries, including local metadata |
| Aggregate referenced-part input vertices / triangles | 6,000,000 / 2,000,000 | Active across actually parsed parts, before caller fields/vector growth; cached parts count once |
| Aggregate objects / components / build items | 10,000 / 100,000 / 100,000 | Active physical admission before caller fields; duplicates/discarded/disabled records count |
| Project JSON selected fields | 256 positions per field / 256 Unicode scalars per label | Active private visitor; 8 MiB pre-SDK input guard, fixed default recursion boundary, full semantic tail validation |
| Collected INPUT text | 16 MiB cumulative | Active core/runtime context, before append; no refund on dropped/invalid UTF-8 input |
| All owned metadata copies | 16 MiB proposed | OPEN: names/paths/normalized strings/JSON are not covered by input collection |
| XML token / attribute value | 64 KiB / 4 KiB | Active four core and runtime image/metadata XML paths; 128 items, depth 128, live raw names 256 KiB |
| Model settings / project metadata / root relationships | 8 MiB / 8 MiB / 64 KiB | Active shared actual collection; existing declared-oversize project None/no-read skip retained |
| Actual inflated read work / model part | 512 MiB / 100 MiB | Active core/runtime contexts; actual bytes and EOF/CRC probes; outer/nested packages stay separate |
| Encoded PNG | 4 MiB | Active sink/finalization; does not bound compressor/decoder memory |
| Component processing work | 10,000,000 visits | Active per parse: diagnostic scans, membership and emission checked before work |

Retain existing 100 MiB model input, 2,000,000 flattened triangles, component depth
32, viewer plates 256, viewer parts 1,024, standard materials 1,024 and embedded
filaments 256. Retain embedded input 16 MiB, dimensions 4,096, pixels 4 million and
decoder allocation limit 64 MiB. None alone is a whole-producer memory bound.

The earlier application-preflight proposal was superseded by the reviewed
vendored ZIP 8.6 constructor: 64 combined footer candidates, 64 MiB metadata
read/allocation-work allowances and a finite read/seek-call budget apply across
all attempts, including mutable seekable sources. Physical records/ranges are
checked before deduplication; original wire identity, bit-3 placeholders and small
ZIP64 remain supported. Checked cached offsets do not establish payload/source
identity or verify descriptor gaps. A consumer workspace must select the same
reviewed vendor patch at its own root.

Next, enforce package-wide input storage/aggregate metadata counts and controlled
fallible growth before every remaining copy/push/insertion. Cached reuse must not
charge stored input twice; actual re-reads still spend work. Do not replace these
remaining bounds with the already active XML acquisition or collected-input caps.
Typed terminal errors must remain terminal through optional/default paths while
ordinary bounded malformed metadata retains existing fallback.

## Admission and the finite work profile

The shared native/catalog/default producer must acquire its Owned reservation
before source payload reads, ZIP construction, extraction or parser validation.
Opening/statting an admitted source may precede byte admission. For an indexed
archive member, account the outer file (currently allowed up to 1 GiB), central
metadata, selected member and expansion/validation work before
`extract_member_to_temporary`. Retain its identity/hash checks. Its validation
parse and the later thumbnail parse are separate bounded passes; reserving only
`model.bytes` misses the outer work. Do not introduce additional retained source
copies to implement budgeting.

After the above limits are enforced, derive an allowance from the maximum
simultaneous live capacities in the actual pipeline: raw source; ZIP metadata;
cached input vertices/triangles/components/maps; copied XML/JSON; flattened
positions/normals (48 used bytes per triangle, before capacity overhead); viewer
metadata; and either raster buffers or embedded decode/transform/encode buffers.
The current 512-pixel raster uses about 2 MiB for RGBA plus depth, but keeps parsed
geometry alive. Embedded selection also happens after model parsing, so its image
peak overlaps that model. Include bounded PNG output and any verification copy.
Sequential outer hashing, validation parsing and thumbnail parsing add work, but
do not blindly sum memory that has already been released.

Only then choose and test a conservative producer allowance and concurrency cap.
Charge against the existing shared Owned accounting (default 4 GiB, possibly
configured lower), as well as any narrower producer work cap. Never use an
independent ledger to expand that limit. This is a finite allocation/work profile,
not a measured or guaranteed process RSS number; decoder/parser internals and
allocator overhead need explicit conservative allowances. If a defensible bound
is still missing, keep publication wiring gated and report that gap.

## Historical review findings and foundation checkpoints

These earlier requirements/checkpoint results are retained as historical evidence.
Constructor/local-range, typed-error, PNG-finalization, component-work and core
XML/actual-inflation items are now implemented as stated above; JSON/model storage,
owned allocation and producer admission remain open. The future-tense notes
below describe their original checkpoint, not current missing ZIP protection.

- **High — constructor fallback:** ZIP 8.6 retries earlier EOCD records after a
  central decode failure. A terminal-envelope preflight followed by unrestricted
  `ZipArchive::new` is insufficient; `ArchiveOffset::Known` does not pin the chosen
  EOCD. Bind construction to the exact approved envelope or enforce allocation
  limits at every constructor attempt, potentially through a narrow reviewed
  dependency patch. Test a bounded last footer with a rejected extra field and an
  earlier forged large-count footer. Do not enable a partial guard.
- **High — mutable metadata:** a retained outer-file FD does not prevent in-place
  footer/central changes between preflight and construction. Consume a bounded
  immutable approved metadata snapshot, or enforce limits during construction;
  post-constructor size/hash checks are too late. Test a paused-after-preflight
  same-inode overwrite before any large allocation.
- Bound EOCD comments and ZIP64 `record_size`/extensible data before allocation,
  including checked integer conversions and single-volume consistency. Add raw
  read/seek/candidate work limits covering footer searches, local metadata,
  constructor retries and outer hash passes, separately from inflated work.
- Count physical central records before filename deduplication. Validate the
  checked central range and actual/declared record count; `archive.len()` can
  hide duplicate physical records. Bound decoded names/comments, Unicode-extra
  replacements, retained extra-field copies and lookup-map keys as well as raw
  metadata. A 16 MiB encoded envelope is not a 16 MiB allocation allowance.
- Preserve fatal exhaustion through locator `.ok()`, ZIP32/ZIP64 search catches,
  central retries and prior-error selection. Charge each signature candidate
  inside the search. A previously malformed candidate must never mask a later
  resource stop or cause it to become another footer fallback.
- Validate local headers and compressed ranges against approved central metadata,
  including ZIP64 fields and data-descriptor flags. Legitimate bit-3 zero local
  sizes must use descriptor-aware rules. Refuse overlaps or offsets into central
  metadata/footer before member reads; charge local variable fields too.
- Enforce JSON node/text/depth limits before a `serde_json::Value` DOM or complete
  settings/type/vendor array clones allocate. Streaming consumers must preserve
  existing first-256 palette clipping and bound unknown-field skipping.
- Introduce typed resource-stop errors through optional metadata/image paths.
  Budget exhaustion, changed envelopes and exhausted read work must not be
  swallowed by `.ok()`, `if let Ok` or catch-all raster fallback. Ordinary absent,
  unsupported or bounded malformed optional metadata should still permit valid
  geometry where the existing feature does so.
- A bounded PNG sink must be paired with explicit encoder `finish()` and propagated
  footer errors. Test exact fit and a one-byte-short IEND/finalization allowance;
  encoder Drop cannot report those failures.
- Replace declared-size debits with actual-delivered-byte charging; do not add both.
  Put lexical guards on the same streaming XML read rather than reinflating a
  preflight copy. Parsing one retained settings string twice is not two inflated
  reads. Temporary numeric coordinate strings are not permanently retained text;
  keep live storage charges distinct from bounded total processing work.
- **High — component traversal work:** declared component counts, depth 32 and the
  flattened triangle cap do not bound repeated walks through a shared component
  graph. A binary DAG with 33 objects and 64 edges ending in a printed empty mesh
  can require billions of visits while emitting no triangles. Both
  `object_has_geometry` and `emit_object` need one package-wide traversal-work
  allowance, checked on every visit across metadata validation and emission.
  Include the preliminary `component_part_metadata_paths` walk: repeated build
  roots otherwise multiply its component scans and path allocations before either
  recursive function runs. Charge before lookup/iteration/allocation there too.
  Preserve repeated-instance semantics; memoizing empty geometry alone does not
  bound all instance walks. This is an existing parser gap, not a regression from
  the unwired foundations.

The bounded encoder/finalization slice is verified at `fc23b4a`: all five new
boundary/byte-equivalence/footer tests pass on macOS and Linux; workspace checks,
Community packaging, Linux server build and Windows GNU compilation pass in
hosted PR run `39372166206`. Windows execution is separate.

The next foundation adds an opt-in bounded ZIP constructor and typed parser stops,
without migrating producers yet. Prefer intrinsic checks in the locked ZIP
implementation over an application preflight followed by unrestricted construction.
One sticky budget must cover all retries and check the actual values used at
allocation time, including mutable seekable sources. Proposed constructor policy
adds 64 footer/ZIP64 candidates, 64 MiB metadata read work and a separately bounded
allocation-work allowance to the limits above. Default ZIP APIs retain their
existing semantics. Preserve dependency license and provenance when carrying a
narrow local patch.

Constructor bounds do not validate every later local-header read or establish
source identity. Keep those gates open: checked local ranges and descriptor-aware
consistency before caching data offsets; separate metadata work from compressed
member payload; retain original source identity/hash checks. An immutable snapshot
alternative would need to cover every reachable retry/header, not just one footer.

Typed `InputLimitExceeded` and `ReadWorkExceeded` are deterministic, nonretryable
stops; `EnvelopeChanged` is a retryable stale-source stop. Temporary runtime permit
contention stays separate. Preserve these types through optional paths until the
outer presentation boundary; never infer their meaning from error strings.

Close typed propagation and constructor/local-header binding before package-wide
parser/JSON changes and producer allowance. These findings remain open until their
implementation tests pass; no unsafe producer wiring is authorized by this plan.

### Constructor and traversal foundation checkpoint

The local candidate includes a narrow ZIP 8.6 patch with an opt-in bounded
constructor; the original constructor and `Config` retain their behavior. The
constructor tracks physical records, encoded metadata, allocation work, candidates,
actual metadata reads and read/seek calls across retries. Actual allocator failure
remains a separate sticky operational error. The source records its upstream
revision and original license in `vendor/zip/GOGGLELAB-PROVENANCE.md`.

No production ZIP caller uses this API yet. Payload limits, source consistency and
complete typed fallback propagation remain gates. The local-header candidate is
recorded separately below.
Cargo patches belong to the resolving workspace root. A downstream app using the
public crates by path must supply the same reviewed ZIP patch in its own root;
adding a separate direct dependency alone can leave two ZIP copies. Before caller
migration, verify dependency edges resolve to the reviewed vendor source, reconcile
each composition's lockfile, and build/test its disposable candidate. Official
private core-pin integration remains a separate release gate.
The component work guard is active in parsing, including the preliminary metadata
walk; its exhaustion is terminal through plate resolution. It preserves normal
instance multiplicity and the existing depth/triangle limits.

Focused offline validation passes 100 parser tests, including 19 bounded-ZIP and
seven component-budget regressions, with one existing ignored fixture. Parser
Clippy and formatting pass; unchanged material/test-helper warnings remain.
Three license-inventory regressions pass and fail against the previous collector.
An actual offline notice audit includes ZIP's original MIT text and upstream URL.
Docker source-copy rules and Linux/Windows parser-test selectors include the local
dependency. Full hosted platform/build acceptance is the next gate for this
checkpoint; no publication or complete parser-memory guarantee is claimed.

### Eager local-header candidate

The bounded constructor now admits each physical local header under the same
metadata budget, preserving the original encoded name and method before Unicode
or AES metadata overrides. It checks lengths, flags, concrete CRC/sizes and ZIP64
fields, then caches the checked data offset and restores the central cursor.
Legal descriptor-mode unknown size values remain supported. Header and compressed
payload intervals are checked and sorted before filename deduplication, including
entries that would otherwise disappear from the final map.

Descriptors between intervals remain unverified gaps. This does not establish
payload identity, full-record nonoverlap or source immutability. Default ZIP
constructors retain their prior behavior; production migration remains gated.

Independent review is clear. Focused offline validation passes 109 parser tests,
including 28 ZIP regressions, with the existing ignored fixture retained. Tests
cover actual local-field mutation, cached offsets after later header mutation,
budget/OOM failures during admission or cursor restoration, physical aliases,
bit-3 placeholders, local ZIP64, Unicode and AES metadata. Parser Clippy and
formatting pass. Hosted platform/build verification follows this checkpoint.

## Compatibility and small boundary tests

Keep the existing 3MF tests for deflated cubes, `p:path` components, distinct-part
resource IDs, repeated/mirrored instances, disabled items, plate membership,
optional metadata and bounded palettes. In particular preserve
`follows_p_path_components_and_applies_item_transforms`,
`reused_resource_ids_in_distinct_model_parts_resolve_independently`,
`instance_occurrences_include_disabled_build_items_before_filtering`,
`optional_slicer_and_project_metadata_enriches_structural_parts` and
`standard_material_and_embedded_palette_entries_are_bounded`.

Add tiny exact-limit/one-over tests using private limits:

- Two referenced parts exceed a shared mesh/text budget although each fits alone;
  unused/unprinted objects count, while repeated cached references do not duplicate
  storage charges. Flattened output remains independently bounded.
- A small repeated-component DAG with empty leaves exceeds a tiny shared traversal
  allowance in both membership validation and emission before expensive expansion.
  Keep ordinary repeated-instance output unchanged; depth/count/triangle checks
  remain separate. Do not execute a billions-of-visits fixture to demonstrate it.
- Oversized token/attribute, relationships/config text and decoded attribute
  expansion fail before their destination buffer grows beyond its allowance.
- Forged EOCD counts, inconsistent central ranges and underspecified inflated
  lengths cannot bypass actual structural/read bounds. A normal small ZIP64
  fixture still parses; entry/name/extra/comment aggregate limits are tested.
- Low outer-archive admission refuses before hashing, member inflation or the
  validation parse. Keep traversal, encryption, cancellation and selected-member
  cache tests in `archives.rs`, including
  `extracts_only_selected_member_to_owned_cache_and_reuses_verified_bytes`.
- Encoder sink accepts the exact output allowance and rejects the next byte;
  malformed/large embedded images retain their current decode guards. Preserve
  `renders_3mf_geometry_and_optionally_writes_qa_artifact` and
  `proof_never_qualifies_embedded_preview_copies_even_when_pixels_match_raster`.
- Cross-producer low-budget tests cover native, catalog and default regeneration:
  one shared reservation cannot exceed the configured cap, and all failures release
  it. Ordinary fixture geometry, thumbnail selection, pixels and provenance stay
  unchanged; archive/embedded functionality is not disabled as a substitute.

Implement parser/ZIP bounds first (`stl-core`, runtime archives/images), then derive
and review the producer allowance, then wire the existing Owned permit into all
producer callers. Stage publication and catalog migration remain separate gates.
No Windows production support is implied by these parser changes.

## Terminal-error boundary for model readers

`stl_core::zip_bounds` provides the shared bounded archive/member facade. Its
observer now covers the six indexing/thumbnail ZIP readers and separate native
manual extractor. An operation creates one observer before optional opens, shares
it across those opens, and finishes the entire result before presentation or
publication. Reads and seeks check the first terminal cause before and after
delegation, including successful buffered results. Optional settings, project
metadata and embedded-image handling preserve terminal failures before cached
geometry, another candidate or raster fallback. Ordinary malformed optional
metadata and local image-size rejection retain their existing fallback behavior.

Known operational allocation failure stays distinct from deterministic input/work
limits and temporary runtime contention. Runtime errors retain their existing
codes and retry policy; native errors retain the Library/message shape. Existing
public String APIs are terminal compatibility adapters around typed bodies.
Source fingerprint/hash and exact length-envelope comparisons produce source-changed stops.

Verified archive reading finishes before cache/staging creation. Rendering stops
before thumbnail staging or catalog writes. A terminal cleanup-proof failure
returns no execution token, drops tentative receipts/permits and leaves every
file untouched; the caller starts a fresh review. Ordinary unprovable candidates
keep their existing behavior. Source holds and publication authority are unchanged.

Consumer workspace roots must resolve the same vendored ZIP package before using
this API, because a dependency's Cargo patch does not propagate to its consumer.
Backup restoration is a separate format outside this model-reader profile.
Core/runtime actual member inflation/XML acquisition and operation-context
composition are active as stated at the top. JSON/model/owned-copy allocation,
whole-producer admission, final composed acceptance, source identity and immutable
publication remain separate gates.

Historical typed-boundary validation passed 128 parser tests (one existing ignored fixture), 347 runtime
tests, 207 native tests and 70 server tests, plus workspace Clippy and formatting.
The native watcher tests require normal macOS event access: the identical binary
fails callbacks inside the workspace sandbox and passes all eight watcher tests
and the full native suite outside it. No source, timeout or test disablement was
used to resolve that environment restriction. Hosted verification follows this
checkpoint; Windows execution remains separate from compilation.
