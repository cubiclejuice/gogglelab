# Community architecture

The public repository builds independently, with no private source, credentials or optional Pro dependency.
`src/community.ts` supplies authored-color viewing to the shared `startApp` host.
`src-tauri` registers inspection, browsing, managed Library, recovery, slicer-discovery and generation-checked slicer-handoff commands.
The Rust workspace also contains model parsing/inspection, managed Library storage and read-only Library recovery crates.

Pro consumes exported frontend contracts and the composable native builder from a pinned `core/` submodule.
Private source owns custom colorways, palette access and licensing.
The core registers Library and slicer launch independently of Pro entitlements.
Library uses the existing app-data library directory; schema upgrades are backed up and initialization is lazy.
The Pro plugin must not register duplicate Library state or commands.
The generation-checked original-source accessor preserves canonical source paths.

Selected-root filesystem operations validate root identity and containment. File moves reject
collisions, symlinks, moving roots or moving folders into themselves. Recovery preserves originals,
Library identities and raw saved colorway records.

The selected Library and handoff implementations moved from private Pro to public MPL-2.0
with explicit owner approval on 2026-10-10. See [Free parity plan](FREE-PRINTVENTORY-PARITY-PLAN.md).

Phase 2 adds additive schema-v2 indexed references with a pre-migration SQLite snapshot
backup. Originals remain in place. A bounded background scanner reconciles complete scans;
failed/cancelled scans do not infer missing data. Indexed sources are validated against
immutable canonical roots at use. Thumbnail rendering runs in bounded blocking tasks and
stores deterministic PNGs keyed by source hash and renderer version. The read-only recovery
reader accepts schemas 1 and 2 and preserves indexed references without reading originals.

## Free organization and archive indexing (phase 3)

Library schema 3 adds metadata, categorized tags, print attempts/filament references,
parent relations and stable bundles without changing original files or library identities.
Metadata batches require expected versions and commit atomically. Print counts and last
successful dates derive from history, independently of the current queue/status selection.
Before schema upgrades, SQLite creates a consistent snapshot with VACUUM INTO.

ZIP scans are opt-in per indexed root. Compound outer-path/member identities retain
metadata. Bounded streaming scans verify an archive payload fingerprint; on-demand
viewer cache copies are hash checked, while thumbnail-only extraction is temporary.
Disabling ZIP scanning pauses those members instead of falsely marking them missing.
Read-only recovery exports organization/history and indexed references for schemas 1–3;
it does not copy external indexed originals. UI metadata is always rendered as text.

## Free management and multipart workflows (phase 4)

Versioned removal preserves tombstones/history and an owned recovery snapshot. Source
changes advance metadata versions so stale cleanup selections cannot trash changed files.
Explicit physical Trash is limited to loose indexed files; shared managed blobs and ZIP
contents survive catalog removal. Delayed restore uses verified owned copies and refuses
destination collisions. Full backup is separate from the read-only recovery export:
it contains a consistent SQLite snapshot and hashed owned namespaces. Restore verifies
the archive/catalog, stages replacement, preserves the previous directory, advances
metadata versions, and invalidates viewer state before swapping.

Library I/O admission prevents replacement during scanning, thumbnails, source copying
or viewing. Independent bundle preview state caps geometry at 32 parts with a notice,
while handoff admits all members and preserves originals. Single and bundle sends verify
content and launch fresh supported slicers with literal arguments. Handoff copies live
outside the replaceable Library root, and macOS launches use a new instance.

Preview images normalize bounded PNG/JPEG inputs to small owned PNGs. Safe package
thumbnail relationships support embedded 3MF images; custom images retain priority over
regeneration. No new capability depends on license state.

## Free filaments and integrations (phase 5)
Schema4 adds stable manual/Spoolman catalog rows and ordered max-five model assignments.
Referenced manual records archive; missing/archived remote records retain identity.
Filament types and physical spools use separate source-scoped IDs. Complete bounded
GET-only snapshots reconcile atomically; partial failures do not mark entries missing.
Recovery schemas1–4 preserve unknown historical IDs and selected dependency rows.
Heterogeneous batch assignments prevalidate every target before one transaction.

library-integrations implements bounded provider/Spoolman HTTP without GUI dependencies.
Remote endpoints require HTTPS and admitted pinned DNS/IP destinations; local/private
endpoints require explicit scope. Redirects and environment proxies are disabled.
Native IntegrationState keeps Zeroizing session secrets out of persisted config/catalog
exports. Single-flight admission bounds jobs; config/library generations and model versions
reject stale results. Reviewed decisions apply atomically and tokens expire/consume once.
Known tag identities normalize locally so displayed merge results match stored categories.
Category sharing defaults to names-only/local; the owner approved an opt-in provider setting for relevant category labels on explicit generation.
