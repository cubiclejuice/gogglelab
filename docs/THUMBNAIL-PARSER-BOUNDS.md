# Thumbnail parser resource prerequisite

Proposed implementation gate, not an enabled producer or publication change.
The unwired `OwnedTempReservation` foundation preserves the existing configured
Owned limit; it does not establish a parser memory allowance. Implement and verify
the bounds below before assigning a shared producer work allowance or wiring the
publication contract. Keep ordinary STL, 3MF, embedded-preview and archive-member
generation available within explicit limits. No source or historical cache cleanup
is part of this prerequisite.

## Allocation boundaries to change

- `crates/stl-core/src/threemf.rs`: `Package::open` constructs `ZipArchive` before
  an application metadata limit; `Package::reader` charges declared entry sizes,
  while `parse_model_part` retains input meshes without a package-wide count cap.
  `Out::push_triangle` limits flattened output only. `optional_text`,
  `root_model_path`, `attr_str` and `read_event_into` can allocate large text before
  downstream truncation. Share one budget across every distinct referenced part,
  including unused, unprinted and disabled objects; a per-part reset is unsafe.
- `crates/stl-core/src/parse.rs`: enforce the triangle cap before binary
  `with_capacity` in every dispatch branch. For ASCII, check before appending and
  bound capacity growth rather than treating used length as allocation size.
- `crates/library-runtime/src/archives.rs`: `preflight` runs after `ZipArchive::new`.
  `extract_member_to_temporary` first calls `read_verified_archive_member`, which
  hashes the outer file, inflates the member and validates it with `parse_model`.
  Admission after extraction is too late.
- `crates/library-runtime/src/images.rs`: `embedded_3mf_thumbnail` also constructs
  ZIP metadata before its entry check. `normalize_image` has decoder limits, but
  input, decoded/oriented image, resized/RGBA image and encoded output can overlap.
  Both its PNG encoder and `thumbnails.rs::render_png` now use a shared bounded
  output sink and explicitly finish the PNG stream. The sink refuses a write
  before it would exceed 4 MiB and retains errors through footer retries. This
  bounds encoded output, not compressor workspace or the complete producer.

The locked ZIP implementation has no application entry-budget option and may
reserve from the EOCD count during construction. A subsequent `archive.len()` check
does not prevent that allocation.

## Initial finite limits and enforcement

These are conservative proposed defaults, subject to the fixture gate below.
Use a private test-limit override to exercise boundaries without large fixtures.

| Resource | Initial cap | Enforcement point |
| --- | --- | --- |
| ZIP central records / metadata envelope | 10,000 / 16 MiB | Before every ZIP constructor; include names, comments and extras |
| Aggregate referenced-part input vertices / triangles | 6,000,000 / 2,000,000 | Before vector growth, across the package |
| Aggregate objects / components / build items | 10,000 / 100,000 / 100,000 | Before insertion or push, across the package |
| Retained copied text | 16 MiB per package | Before allocating owned names, attributes, paths and metadata text |
| XML token / attribute value | 64 KiB / 4 KiB | Before parser token-buffer growth / owned attribute copy |
| Model settings / project metadata / root relationships | 8 MiB / 8 MiB / 64 KiB | Declared check plus actual bounded read |
| Actual inflated read work | 512 MiB per package parse | Shared counter on bytes delivered, including repeated real reads |
| Encoded PNG | 4 MiB | Sink refuses the next write before exceeding the limit |

Retain existing 100 MiB model input, 2,000,000 flattened triangles, component depth
32, viewer plates 256, viewer parts 1,024, standard materials 1,024 and embedded
filaments 256. Retain embedded input 16 MiB, dimensions 4,096, pixels 4 million and
decoder allocation limit 64 MiB. None alone is a whole-producer memory bound.

Introduce a small shared ZIP-envelope preflight in `stl-core` usable by both
in-memory 3MF readers and the runtime's seekable outer-archive reader. Read only a
bounded bootstrap window under an admitted permit. Validate EOCD/ZIP64 locator and
end record, checked offsets/ranges, central-record structure and actual record
count/aggregate variable fields before constructing ZIP metadata. Preserve small
valid ZIP64 archives; do not reject ZIP64 simply because its format is supported.
Reject malformed, unsupported multi-volume or ambiguous envelopes before metadata
allocation. A declared count, declared inflated length or compressed length is
only an early check, never sufficient authority for allocation or read work.

Use one checked package budget for mesh counts, retained text and inflated reads.
Cached repeated references charge stored mesh once; every actual read charges
work. Bound XML tokens before `quick_xml` grows its event buffer, for example with
a bounded streaming lexical preflight that handles quoted attributes, comments,
CDATA and declarations correctly. A post-`read_event_into` length check is too
late. Attribute limits apply to raw and decoded/copied forms; normalize labels
only after admission. Bound JSON/config collection counts and retained text too.
Use checked arithmetic and controlled `try_reserve` growth; account capacity and
map overhead, not just the number of live elements. Limit errors must propagate;
they must not become optional-metadata fallbacks that continue unbounded work.

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

## Review findings that block wiring

The plan review found the following concrete requirements. They describe existing
allocation/fallback behavior, not regressions introduced by the unwired foundation.

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

Smallest next implementation slice: bounded encoder sink plus explicit finalization
in both PNG producers, preserving normal bytes. Then close typed propagation and
constructor binding before the package-wide parser/JSON changes and producer
allowance. These plan findings remain open requirements until their corresponding
implementation tests pass; no unsafe producer wiring is authorized by this plan.

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
