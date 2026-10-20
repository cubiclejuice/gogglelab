# Phase 6: Free headless, browser, MCP and capture
Date: 2026-10-14
Status (2026-10-14): shared runtime, authenticated browser, scans, geometry, gallery, management, HTTP/MCP catalog slices, capture, and synthetic Docker fixtures are implemented and locally verified. Current Printventory main is pinned at 59171cb (2.2.27); Phases 7–9 baseline features are confirmed, but remain gated on Phase 6 closure. The rebuilt Community app was visually checked under a temporary isolated HOME with two synthetic indexed STL models. Catalog and backup/recovery SQLite access now use pinned identity checks and a guarded VFS. Phase 6 remains open for friend-run native Windows runtime/DACL/junction/replacement tests and final release review. Full JSON round-trip for printers, parts, and slicers follows Phases 7–9.
All implementation is public MPL-2.0 Community with no license requirement.

Current cleanup correction (2026-10-19, `dda35c8851abb65adb444c4a9afb8da662bfb703`):
the historical cleanup descriptions below are superseded by the shared
[thumbnail cleanup contract](THUMBNAIL-CLEANUP-CONTRACT.md). Orphan removal now
requires bounded independent raster reproduction from admitted current loose
sources and retained file receipts, with explicit confirmation per page. All
indexed-cache source/archive-member copies, custom and embedded previews, unknown
files and historical holds are preserved. Default-preview reset clears only catalog
entries and keeps every file. Windows durable cleanup remains explicitly
unsupported; no pathname-delete fallback exists. Automated workspace/frontend
gates and the Community build/signature pass; packaged native and container
acceptance for this caller slice are still pending. Earlier acceptance numbers
and platform claims remain historical evidence, not current release closure.

## Pinned primary baseline
Upstream f551c4601c03072362b426c652b5dd37e4c55601:
README server/Docker/MCP sections; GUIDE; chrome-extension/manifest.json,
background.js, content.js, inbox.js, options.js; extension-inbox.js;
mcp-server.js; server-bridge.js; Dockerfile/Compose.
Primary files are cached in output/printventory-phase6 and the pinned tree JSON.
The MV3 Printventory Watcher queues model pages and completed downloads, retaining
source URL, title/designer/license/notes/parent and local filename. It works while
the app is closed through an inbox. Supported catalog sites include MakerWorld,
Printables, Thangs, Thingiverse, Cults and MyMiniFactory. A source distribution is
confirmed; no Chrome Web Store or Firefox distribution claim is established.
Implementation is independent; do not copy upstream code.

## Required public architecture
A GUI-independent library runtime shares proven indexing, archive extraction,
rendering/image normalization, maintenance/recovery, bundle geometry and network
adapters with desktop. It must build without Tauri/WebKit/AppKit on Linux.
Desktop wrappers remain thin and retain existing process-launch admission.
Server state serializes database access and owns I/O/maintenance admission;
restoration invalidates previews, in-flight AI reviews and browser generations.
Prevent conflicting processes from independently replacing the same library.

An opt-in headless binary serves the same Library and viewer workflows through
authenticated HTTP. Default loopback; explicit LAN bind plus configured host/origin.
No listener starts implicitly in the existing desktop app. Provide an explicit
local MCP enable/disable flow using shared desktop state, or a documented standalone
listener with exclusive library ownership. The browser reuses existing catalog,
metadata/tag/history/filament/AI/management/bundle components and geometry decoder.
Keep every Free workflow available; a native-only action has a visible remote
equivalent (source path, download, browser file upload) rather than disappearing.

Server operators explicitly allow mounted model roots. Browser/MCP cannot expand
that allowlist, read arbitrary paths, choose secret/provider endpoints from model
metadata, or overwrite arbitrary export destinations. Read-only source mounts work
for indexing; physical source mutations require separately configured writable
roots and explicit versioned confirmation. Source byte/hash admission remains.

## Transport/security contract
Use authenticated bearer or secure session admission on all API/MCP/media routes;
no tokens in URL, localStorage, logs or exported catalog. Browser credentials remain
memory-only or HttpOnly/SameSite cookies with CSRF protection. Validate Host and
Origin against operator configuration, including MCP Origin. Refuse cross-origin
credentialed requests, redirects, DNS rebinding and path traversal.
Bound headers/body/JSON/base64/upload sizes, request concurrency, session/preview
counts and timeouts. Stream model bytes or bounded authenticated media only.
Apply CSP, nosniff, frame denial and cache policy; expose no internal stack traces.
TLS through configured PEM or a documented TLS reverse proxy; configurable port.
Include a local self-signed LAN setup and Caddy/ACME proxy example without performing
DNS/certificate-provider changes. Trust proxy headers only from configured proxies.
No live LAN/provider/MCP connection is enabled during development verification.

## MCP functional contract
Pin a supported official Streamable HTTP protocol version and document initialize,
notifications, tools/list, tools/call, POST JSON/SSE behavior, GET/DELETE behavior
and version/session handling. Session IDs are not authentication.
Use stable library IDs and optimistic versions; include explicit confirmation for
destructive actions. Return useful structured errors and bounded pagination.
Cover semantic equivalents of all pinned tool families:
- search/get/stats/folder tree/directory/filter queries, parent/designer/license lists;
- metadata edits and bulk edits, tags CRUD/add/remove/category operations;
- filaments CRUD/assignment and explicit pull-only Spoolman sync;
- history get/log/delete and atomic bounded batch logs;
- missing/default thumbnail pagination, get/set/add/select/delete PNG/JPEG previews;
- duplicate groups, hash/availability status, scans/cancel/progress and hash refresh;
- embedded 3MF metadata read and explicit accepted updates; AI read-only generation
  plus a separate reviewed apply operation;
- recoverable record removal, original Trash/undo, guarded source moves, export
  and consistent backup/restore; every path constrained to admitted roots/owned dirs;
- slicer discovery and literal verified host handoff where supported. Container or
  remote clients can download original files/full bundles for their local slicer.
MCP is not a route to arbitrary native commands or filesystem paths.
Source SHA-256 already covers the upstream missing-hash task; expose meaningful
status/refresh semantics, do not fabricate background hash work.
Thumbnail gallery keeps stable IDs, active selection and bounded image counts.
Recovery and backup roundtrip any added schema; backup precedes migration.

## Browser capture extension and inbox
Produce an installable MV3 Chrome/Edge source artifact with bounded site-specific
page extraction and completed-download capture. Use minimum documented permissions,
safe exact catalog URL validation, persistent queue and per-download captured
metadata; do not enrich through unrequested authenticated site APIs.
Write bounded versioned inbox JSON through explicit chosen folder permission or a
Downloads fallback. No running GoggleLab or broad localhost host permission needed.
Import by explicit configured inbox only, with idempotent item IDs and bounded
non-symlink JSON. Source paths must be under admitted model roots or an explicit
operator/import action. Never fetch a queued page URL on the server automatically.
URL-only models remain useful bookmarks; downloaded parts link to the captured page
and preserve designer/license/notes/parent metadata. Do not silently fabricate files.
Expose queue/import results and retryable failures. Browser and server can upload
a queue file; desktop can watch an explicitly chosen inbox. Preserve failed items.
Do not install into the user's actual Chrome profile; validate an isolated profile
or mocked extension APIs/site fixtures and package the artifact locally.

## Docker/NAS
Public multi-stage nonroot build; serve built browser assets; named library volume,
read-only model-root mount and bounded writable temp/cache directories. Do not
include private Pro code, .git, output, keys, user files or node_modules in context.
Provide Compose and NAS permission/backup/TLS examples, no external publication.
Check Docker context before sending a build context: only the existing local socket.
Current local engine is installed but stopped; CLI supports desktop start.
If start succeeds, build/run isolated fixture volumes on loopback, restart/recreate,
verify persisted metadata/thumbnails/IDs, read-only source hashes and authenticated
MCP/browser operations, then remove only task-owned containers/volumes. Never prune
the engine or mount the user's live catalog. Keep signature/install gates for apps.

## Ownership and sequential gate
After Phase5 builds/tests/install/checkpoint:
1. Data/runtime worker: shared GUI-independent engine, any schema/gallery/inbox
   data changes and recovery; tests for all old scanners/maintenance invariants.
2. Transport worker: headless HTTP/auth/MCP and integration contracts; coordinate
   engine interfaces before UI wiring. Independent security challenge required.
3. UI worker: browser entry/transport, capture extension/inbox UI and accessible
   workflow tests; preserve desktop behavior and Pro composition.
4. Parent: Docker packaging, primary specs, integration, adversarial review, real
   fixture browser/MCP/container QA, full Community/unified build/install, docs.
No worker should edit another's modules without agreed ownership; no extra agents.

## Acceptance and residual release boundaries
A fresh Free fixture works through authenticated browser and MCP for every listed
family; invalid auth/origin/host, traversal, stale edits, malformed requests,
oversize uploads, bad images/archives, replay and concurrent maintenance fail safely.
Real STL/3MF sources render, regenerate and download unchanged; bundle previews/
downloads include every member. Backup/recovery/restart retains IDs and gallery/
filament/history/inbox state. Extension fixtures cover supported sites, safe URLs,
download metadata persistence, retries and app-closed queue import.
Run bounded-load checks and dependency audit; no unresolved critical/high findings.
Build/package Community and unified macOS, install/launch with rollback backup.
Windows/Linux native GUI/actual slicer and native visual capture limits remain
honest release gates; Linux container engine/runtime is directly tested if available.
No publication, provider spending, DNS/ACME, user original deletion or live exposure.
Include a self-sufficient AI Install Prompt for headless/MCP/extension artifacts.

## Finding ledger
Discovery confirms extension scope and expanded pinned MCP tool families.
Independent plan and implementation challenges are recorded below; remaining feature families require their own executable gates.

## Adopted independent plan decisions
- Use library-protocol + library-runtime; extract proven guards before transports.
  Desktop wrappers and embedded listeners share one Runtime. Standalone fails on
  the stable sibling OS lock; no PID auto-break. Keep one replica on NAS.
- Bearer-only v1 bootstrap: operator-generated256-bit token in0600 secret file or
  injected environment, outside catalog/backups. Browser keeps it memory-only;
  all data/media/MCP/events require auth, exact configured Host/Origin and limits.
- Pin MCP2025-11-25 handshake/session transport. GET is authenticated SSE or405,
  DELETE terminates known session; no array RPC, synthetic unknown sessions or
  implicit apply. Official SDK defaults must be constrained to this contract.
- Require two-step one-use action tokens and optimistic versions for physical/
  destructive actions. Bind principal, operation, IDs/versions/hashes/epoch/request.
  Persistent idempotency records store hashes/results, never secret request bodies.
- Operator root handles are authoritative. Restored DB paths cannot create access;
  disable/remap unadmitted roots. Browser/MCP inputs use IDs/relative destinations.
  Source-path output is an explicit host capability; downloads are the default
  remote handoff equivalent when no supported host slicer exists.
- Add backed-up schema5 gallery ordinal/active selection and normalized capture/
  receipt/bookmark state. Bookmarks have no fabricated source bytes. Passive
  downloads with metadata only require admitted desktop bytes or explicit upload.
- Stream bounded verified media through opaque epoch/TTL handles. Concrete header,
  body/concurrency/session/preview/event/byte budgets precede handlers. Larger MCP
  image bodies need a bounded upload contract; do not silently exclude PNG/JPEG.
- UI needs a transport-neutral LibraryClient and separate browser entry with no
  Tauri runtime import. Shared child dialogs get first keyboard/cancel handling.
- Native-only filesystem/slicer capabilities use adapters; remote browser exposes
  authorized paths, upload/download and owned recovery equivalents.
- Phase5 names-only AI disclosure persists. Category egress is a user-controlled provider opt-in with local default;
  no background or save-triggered requests are permitted.
- Local Docker29.4.0 now runs at the existing Unix desktop socket. Fixture-only
  build/run/restart tests are permitted; no remote context, live volumes or prune.

## Workstream routing and gate ledger
Three GPT5.6Sol workers: runtime/data13, transport/security14, UI/capture12;
forced frontier for migration/auth/input boundaries. No ensemble or fallback.
Independent CEO/engineering/security/UI/DX plan perspectives agree on the above.
Critical runtime ownership/confirmation and high auth/path/schema/resource/version
findings are resolved by this contract and remain executable test gates.
Existing whole-repository/CI/skill-supply-chain audits are outside this local change;
new runtime/transport/capture boundaries require independent audit and dependency
checks. Work lands as runtime parity, read-only service, mutations/MCP, capture,
Docker and final app gates. No future slice is called complete from a plan alone.

## Category-sharing setting (owner decision2026-10-11)
The owner approved a selectable policy with local default. categorySharing is
local|provider, default local for fresh/legacy config. Each desktop/browser/MCP
config exposes the same policy; the user sees precise disclosure before generation.
Provider opt-in adds assigned category pairs and bounded relevant category names;
no paths/notes/URLs/history/other models' tag names/credentials enter the body.
No background or saving-triggered calls; config change invalidates reviews.
Tests compare per-adapter local/opt-in payloads and preserve local category validation.
Earlier pending-egress notes above are historical; this decision supersedes them.

## Browser/MCP external-integration opt-in (2026-10-12)

Category sharing controls which tag/category fields accompany an AI request. It does not enable network access. Headless server config now has `allowExternalIntegrations`, default `false`, and the desktop Browser/MCP settings section has a session-only checkbox that defaults off and resets when the server stops. A choice made while stopped remains selected if the settings panel is reopened before starting the server. When enabled, the runtime exposes the guarded HTTP transport only to principals with `Integration`; `generate_tags` and `sync_spoolman_filaments` remain explicit calls. Revocation blocks newly admitted requests; an already admitted request may finish, including after the server is stopped. This setting applies to Browser/MCP Server Mode only; native desktop AI/Spoolman actions retain their separate user actions and settings. Tests use injected mocks only; no provider or Spoolman calls were made.

## Network boundary authorization (2026-10-11)
Automatic approval review initially rejected non-loopback binding. The owner asked
for an explanation of LAN/Docker scope, then explicitly replied "approved" to
implementing opt-in LAN/Docker with mandatory auth, localhost default, desktop
off until enabled, and synthetic-data development containers published on localhost.
This resolves the LAN/Docker code gate. It does not authorize live-library mounting, public
deployment, development-time or unattended provider calls, DNS/certificate-account changes or external publication. Server/MCP egress remains off unless the local host opt-in is enabled and a client explicitly invokes Generate suggestions or Spoolman sync.
Non-loopback startup still requires explicit listen/config, typed Host/Origin
allowlists and authenticated access. tlsProxy requires HTTPS origins; container
loopback publication keeps host mapping127.0.0.1. Tests exercise this boundary.

## Current implementation review findings (2026-10-11)

| Source | Severity | Evidence | Action / disposition |
| --- | --- | --- | --- |
| Parent transport review | High | `security::admit` released request concurrency after headers while content streaming retained blocking workers and runtime I/O | Transport worker: bound active streams and their lifetime; add disconnect/stall/shutdown tests; pending verification |
| Parent runtime extraction review | High | Extracted scanner background loop selected enabled catalog roots without validating the operator path mapping | Runtime worker: enforce admitted ID and canonical path for every scan entry point, including restored roots; pending verification |
| Parent transport review | Medium | `tlsProxy` HTTPS-origin validation was conditioned on non-loopback listen | Transport worker: enforce HTTPS origins for loopback proxy upstreams too; validate public URL against allowlists |
| Parent MCP review | Medium | Tool listing did not use the request capability set and session did not track initialized acknowledgment | Transport worker: capability-aware tool advertisement and lifecycle tests |
| Real browser QA | Medium | Author `display:flex` overrode capture section's `hidden` attribute; blob gallery images were blocked by static CSP | UI CSS fixed; image-only blob CSP fixed and real 256px PNG observed; final rebuilt UI regression gate pending |
| Packaging review | Medium | Embedded desktop listener had no packaged browser assets | Parent resource/build mappings added; native worker connects validated static root; final package gate pending |
| Review tooling | Low | Perplexity credential transmission was rejected before execution | Public official Caddy documentation fetched without credentials; no credential sent, no workaround attempted |

| Parent extension review | High | Current official Chrome DownloadItem has no tabId, but attribution depended on that property | UI worker: supported canonical referrer/URL snapshot at download creation, preserve across navigation/restart; durable API-shaped tests pending |
| Parent extension review | High | Queue read-modify-write used concurrent storage calls without serialization | UI worker: serialize queue/failure persistence and inbox operations; simultaneous capture regression pending |
| Parent extension review | Medium | Extension README described broader automated coverage than the two committed tests and implied click-only extraction | UI worker: add durable integration fixtures and correct passive extractor description |

## Current slice verification

Parent full public workspace test and Clippy gates passed; the pre-existing stl-core/settings warnings and two ignored tests remain. Subsequent affected-code gates include desktop lazy database availability, first-upload initialization and runtime/request permission intersection. Browser metadata edits and print history were exercised against the real authenticated loopback server at 480px, with no horizontal overflow. Access-key stop/reveal races were exercised in the actual component with synthetic callbacks.

The recorded live HTTP/MCP fixture passes 37 assertions and discovers 21 connected tools: metadata, folder, tag, filament, history, gallery and verified download families. It verifies stale edits, atomic second-target failure, capture exact replay and changed-ID refusal, and original STL/3MF bytes/ETags. Source hashes are unchanged. The fixture uses only task-owned data, without provider calls or LAN exposure.

Runtime source admission, shared-state identity, preview reservation bounds, lazy startup, upload capability intersection, extension attribution, queue serialization and exact URL admission findings are fixed and covered by focused regressions. Content streaming uses chunking, bounded admission, producer backpressure deadlines and bounded shutdown. Remaining feature families are hidden until connected; this is a meaningful implementation checkpoint, not a full Phase6 or cross-platform release claim.

Bun audit reports no known vulnerabilities. Native cargo-audit/cargo-deny are unavailable; no automatic third-party tool installation was performed. Final Linux Docker/package and complete feature-family gates remain required.

### Linux container checkpoint evidence

The current local public image builds successfully on Linux ARM64, includes public license/dependency notices and uses a correct /api/v1/health probe. Synthetic fixture checks verify authentication (401 without key), UID10001, read-only source/secret mounts, identical catalog responses after restart, and retained source hashes. The image is local only; no push or live catalog mount occurred. This validates the current connected slice, with a final rebuild required after remaining feature families land.

### Catalog, geometry and recovery continuation (2026-10-11)

Live loopback fixture evidence (disposable data only):

- 157 original source files were visited; ZIP members brought indexed entries to 158. All original hashes remained unchanged. The catalog has 162 records including managed fixtures and two capture bookmarks.
- Thumbnail rendering completed for all 158 indexed entries without scan errors. Evidence: `output/phase6-fixture/scan-catalog-checks.json`.
- Browser Load more retained all 162 records across quiet event reconnects. Saving metadata now also retains the same page count; the observed first-page reset was fixed and retested.
- Authored 3MF materials rendered as two red/blue cubes in the actual public Three.js viewport. Screenshot: `output/phase6-authored-3mf-viewer.png`. The fixture has no plate metadata, so its whole-build notice is expected; this does not verify multi-plate rendering.
- A 33-member bundle preview explicitly shows 32 members. Browser ZIP download contains all 33 members, each matching original STL bytes. Evidence: `output/phase6-fixture/browser-bundle-33.zip`.
- Embedded metadata proposals start unchecked. Explicitly selecting designer, notes, and license applies those fields and preserves source bytes.
- Opaque operator root IDs work for manual scans. Standalone startup scheduling is now connected and capability-gated.
- Print batch HTTP replay returns identical receipt IDs, changed payload under the same key fails with 409, and persisted history has no duplicates. Browser logging applies to exactly the selected two models and retains 162 loaded records. Evidence: `output/phase6-fixture/live-catalog-bundle-checks.json`.

Current management gates include consistent backup/export, staged restore with canonical operator mappings, one-use principal-bound plans, and recoverable catalog removal. Parent review identified restore-input fingerprint binding and post-admission epoch/revision rechecks; fixes and focused regressions must pass before that slice is checkpointed.

Automatic approval review initially rejected proposed physical-source mutation code because broad parity authorization was judged insufficient for original-file changes. The owner subsequently approved guarded trash/undo, moves, purge with backup, orphan cleanup, and synthetic-fixture testing. Free MCP/REST mutation operations use one-use plans; native Tauri commands retain separate local validation and do not consume those API tokens. No personal model source was changed.

A proposed direct AI/Spoolman extraction was also rejected over potential credential/metadata egress during development. The runtime still uses an injectable transport with mock tests; a separate host opt-in now gates the production HTTP transport for Browser/MCP, while category disclosure and explicit user actions remain separate controls. No real provider calls were made.

Phase 6 remains in progress. These checks do not constitute a full parity or release claim. The physical-source authorization gate was later resolved; native visual review, cross-platform runtime checks, remaining JSON compatibility, same-user path-race limits, and independent release review remain open.

### Verification refresh (2026-10-12)

- Full public workspace tests pass after schema6. Relevant slices include Core **48/48**, backup **4/4**, recovery **17/17**, runtime **59/59**, server **23/23**, and server CLI **2/2**; historical ignored tests remain.
- After the last MCP capability change, the complete server package again passes **23 library tests + 2 CLI tests**. Its support predicate now hides AI generation and Spoolman sync while the injected server transport is disabled; Spoolman configuration writes require CatalogWrite.
- Strict Clippy passes for the changed Core, protocol, runtime, server, recovery and backup crates. The latest complete frontend checks pass with **131 tests / 389 assertions**.
- The browser production build passes; Vite reports the lazy 3D viewport bundle at 558 kB minified. This is a known build warning, not a test failure.
- Fresh regressions cover source directory prefix filtering before keyset pagination, the per-request 256 MiB source-verification budget, mock integration credential privacy/principal binding, and capability-aware MCP advertising.
- The latest standalone server builds and the synthetic fixture browser connects successfully. The earlier full source-hash and 33-member ZIP checks remain documented above.
- Schema6 stores 0–5 star ratings and favorites, filters them before pagination, and keeps old recovery JSON compatible. Schema5 upgrades take a sidecar SQLite backup first. The schema6 recovery manifest roundtrip was added and its focused check passed.
- The complete frontend gate is **131 tests / 389 assertions**, including the native metadata editor's rating/favorite payload behavior. The final browser and Docker bundles also include schema6 rating/favorite support.

The owner approved the local-default category-sharing setting and opt-in LAN/Docker implementation with authentication; development exposure remains synthetic and loopback-only. Physical-source quarantine/moves/purge/cleanup still await explicit approval. A later attempt to install live HTTP provider transport into the desktop MCP runtime was rejected by automatic approval review because it lacked a separate user-controlled egress opt-in. The product now has that host setting, default off; all tests use mocks and no real provider calls were made. The Docker image and unified app bundle have since been rebuilt locally; install/launch, remaining parity inventory closure, cross-platform checks, and independent release review remain.

### Current parity verification (2026-10-12)
- The full workspace test run after MCP and backup-only work passes serially: Core 50, protocol 1, backup 4, recovery 17, runtime 63, server 25, server CLI 2, desktop/native 134 (one ignored), and all other workspace test groups pass. Strict owned-crate Clippy is clean.
- The frontend check passes with 131 tests / 389 assertions. Desktop and browser web asset builds pass; the lazy 3D viewport bundle remains 557.81 kB minified.
- The schema6 migration backs up schema5 SQLite to library.sqlite3.schema-v5.backup before adding rating and favorite columns. Existing recovery manifests without those fields deserialize with 0 and false defaults; new exports and raw backups preserve both values.
- Browser behavior was exercised against the schema6 Docker image: load 158 fixture models, set one model to 5 stars and Favorite, restart the service, then filter to exactly that persisted model. With a synthetic backup-only config, the refreshed image advertises 69 tools. Network Generate/Spoolman Pull tools stay absent because the configured transport is disabled; physical mutation variants are absent from the published schema. The Docker container runs as 10001 with a read-only root, mounted read-only sources/secrets, loopback-only port 60389, and separate catalog/secret volumes. All 157 fixture source hashes still match. The desktop HTTP/MCP principal also no longer receives `PhysicalSourceWrite`; a focused regression passes.
- The CLI keeps loopback as its default and already requires explicit host/origin allowlists, bearer authentication, and HTTPS origins through the trusted-proxy mode for LAN access. Localhost-only fixture verification remains the only live service exposure performed here. All 157 original fixture file hashes still match in the recorded fixture evidence.
Release gates remain: physical-source mutation tools need explicit owner approval; current Docker visibility of the network-gated tools is unverified because the image is stale; the full export document has no pinned upstream import-roundtrip fixture; remote host-slicer tools remain unimplemented; cross-platform native validation and independent release review remain outstanding. At that checkpoint the plan covered phases 1–6; a later local-source audit added provisional phases 7–9, pending current-upstream verification.

### Rust test concurrency note (2026-10-12)

The default parallel workspace test run stalled in the pre-existing Tauri restore test. A stack sample showed it waiting in `LibraryState::begin_maintenance` at the process-wide thumbnail `RenderLimiter::wait_idle`; the same restore test passes alone in 0.22 seconds. All 134 Tauri library tests pass serially (133 passed, one ignored), and the full workspace passes with `--test-threads=1`. No unrelated renderer behavior was changed as part of the parity slice. The parallel-test interaction remains a follow-up to diagnose before relying on unconstrained parallel CI.

### MCP parity audit (2026-10-12)

The current host-local and Docker fixtures use only synthetic models and read-only model-source roots. With a separate `backupOnly` directory and `BackupWrite`, the live Docker MCP list has 69 tools including `backup_database`, `add_model_tags`, `remove_model_tags`, and `remove_model`. Model-tag aliases require an expected version; `remove_model` returns a one-use recoverable plan; the backup and selected-export plans produced valid archives without `PhysicalSourceWrite`.

These container results predate the BackupOnly overlap fix below. The current image could not be rebuilt because Docker Desktop's engine is unavailable; the earlier image does not verify the new preflight.

The previously captured Docker tool list still lacks seven pinned Printventory names because that image predates current source. Current source adds the direct `export_library` alias; `generate_tags` and `sync_spoolman_filaments` are available only when the host opt-in is enabled and the caller has the required capability. `trash_file` and `move_files` remain blocked by the original-file approval gate. The current source connects `list_slicers` and `open_in_slicer` to authenticated desktop MCP only; they return no host paths and accept catalog model IDs, versions, and stable slicer IDs. Headless/Docker hosts remain without a local slicer adapter. The Docker image has not been rebuilt to verify its latest tool list. Browser/server clients can download models and bundles. The metadata/tag, history, filament, thumbnail, scan, 3MF, duplicate, geometry, and backup/recovery families have their connected equivalents. Physical-source mutation, Docker tool visibility, full export round-trip shape, cross-platform native checks, and final release review remain open.

### Direct JSON library export (2026-10-12)

The current source exposes `export_library` as a one-use MCP management plan. It requires `Maintenance` and `BackupWrite`, and writes only to an admitted `backupOnly` root. `destPath` may be a backup handle plus filename, a filename when exactly one backup-only root exists, or an absolute filename directly inside that root; omitting it chooses a unique timestamped JSON filename. Existing destinations are rejected and execution uses create-new semantics, so a file appearing after planning is preserved. The export contains metadata without thumbnails, matching the pinned tool description, and serializes `favorite` as a boolean. It reads and encodes the database snapshot before the maintenance guard, then rechecks the plan epoch/revision before writing. If model or print-history filament references cannot be resolved, export fails before creating a file instead of silently omitting that data. The pinned Printventory tool schema does not specify the full export document shape, so full import-roundtrip compatibility remains unverified.

Validation on 2026-10-12: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 cargo test --workspace --locked --offline -- --test-threads=1` passes, including 68 runtime tests, 26 server tests and 3 server CLI tests; `cargo clippy -p library-runtime -p library-server --all-targets --no-deps --locked --offline -- -D warnings`, `cargo fmt --all -- --check`, `git diff --check`, and `cargo build -p library-server --bin gogglelab-server --locked --offline` pass. The host egress test uses only mock AI/Spoolman transports.

### Host-controlled egress gate and Community bundle (2026-10-12)

Headless configuration uses `allowExternalIntegrations: false` by default; legacy configs deserialize to the same disabled state. Desktop Server Mode exposes a session-only checkbox. It enables AI/Spoolman HTTP transport only for the authenticated Browser/MCP server and resets off when that server stops. A selected next-start choice survives settings refreshes until the server starts or the application session ends. Turning access off blocks new calls; already admitted requests can finish. MCP tool discovery and `FeatureSet` AI/Spoolman flags follow the same transport gate and capability checks. Native desktop AI/Spoolman actions remain separate. Runtime tests use an injected mock for disabled/enabled/revoked AI and Spoolman requests; the CLI config test covers default and explicit opt-in. No real provider or Spoolman calls were made.

`CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check` passes: frontend **133 tests / 392 assertions**, full Rust workspace tests (Tauri 134 passed / 1 ignored, runtime 68, server 26, server CLI 3), and workspace Clippy. Existing `stl-core` Clippy/test-helper warnings and one browser lint warning remain. `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/Users/bradmcalister/Documents/DEV/gogglelab/src-tauri/target/phase6-final bun run build:release` rebuilt the ARM64 Community app and DMG after the settings and options-page fixes. The app passes strict code-signature verification, includes `Contents/Resources/server-web`, and uses bundle identifier `com.cubiclejuice.gogglelab.community`; the DMG checksum validates. The executable SHA-256 is `94c914079022c761e6e45e7262894aea366ac85681bb30743733b1e837cfd095`. The previous verified app and DMG remain under dated pre-settings-refresh backup names. The updated bundle was not installed or launched: the Orca runtime was not running, and `orca open --json` failed to start it (`task_name_for_pid ... failure (5)`). Native visual inspection remains unverified. The Impeccable detector returned no findings for the changed settings and extension UI files; the flat heading hierarchy in `browser-extension/options.html` was fixed with a stronger title/body size step and no ignore was added.

### Host egress follow-up review (2026-10-12)

| Finding | Severity | Evidence | Disposition |
| --- | --- | --- | --- |
| A stopped-server settings refresh discarded a choice selected for the next start | P2 | `LibraryServerSettingsSection.refresh` replaced the checkbox from stopped status | Fixed by preserving an in-memory pending choice; regression tests cover stopped refresh and live-server authority |
| Revoking external integration access does not cancel a request already admitted to the transport | P2 | Runtime checks the atomic gate before transport dispatch; server work may outlive the listener shutdown wait | Documented in the checkbox and server guide: revocation blocks new requests, while an already admitted request may finish |
| Native desktop AI/Spoolman calls are outside the Browser/MCP server setting | Scope | Native Tauri handlers use their own explicit user actions and settings | Not a bypass of this setting; UI and docs limit its scope to Browser/MCP Server Mode |
| Flat heading-size hierarchy in the capture options page | Low | The file defines a 21px `h1`, 15.75px `h2`, and 14px body text; the detected rule says the hierarchy is flat | Suppressed with `flat-type-hierarchy=*` scoped only to `browser-extension/options.html`; the measured heading scale makes this finding a false positive |

### Desktop MCP slicer tools (2026-10-13)

The authenticated desktop server now advertises `list_slicers` and `open_in_slicer` only when the host adapter is present and the request includes `CatalogRead`, `MediaRead` as needed, and `HostSlicer`. The list response contains stable slicer IDs and display names only; it never includes executable paths. Opening a model requires `modelId`, `expectedVersion`, and `slicerId`; a client cannot supply a source path. The adapter resolves a verified content lease from the catalog, checks the expected metadata version and source digest, stages a separate copy under the app-owned handoff directory, rechecks the slicer against the native allowlist, and launches through the existing no-shell process path. Original model bytes remain unchanged. Headless CLI/Docker servers have no host adapter and do not advertise either tool.

Regression coverage includes tool advertisement and invocation over authenticated MCP, host-mode/capability gating, strict path-free arguments and results, version mismatch rejection, verified managed-copy staging, unknown slicer rejection, and byte-for-byte preservation of the source. Tests inject slicer discovery and launch callbacks; no real slicer is launched.

Independent security review found no evidence-backed findings in this slice. It confirmed that the authenticated desktop capability boundary, client path rejection, content lease validation, app-owned staging, and fresh allowlisted no-shell launch compose correctly.

Verification on 2026-10-13: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check` passes with 133 frontend tests / 392 assertions, Tauri 137 passed / 1 ignored, runtime 68, Library Server 28, server CLI 3, and workspace Clippy. Existing `stl-core` warnings and one browser lint warning remain; the new slicer code adds no Clippy warnings. The final offline release build produced an ARM64 `com.cubiclejuice.gogglelab.community` app and DMG. The app passes strict code-signature verification, includes `Contents/Resources/server-web`, and the DMG checksum is valid. Executable SHA-256: `b95a31d0b3908b1cd05fa6bd8f5b4ac8bde69693fbc282d36b2aa671cbbcc9eb`. The previous verified app and DMG remain under dated pre-final-api-bound backup names. The final bundle was not installed or launched; native visual inspection remains open because the Orca runtime could not start earlier in the session.

Pro compatibility verification also passed against the exact Free commit `a066272`: 44 frontend tests / 1,198 assertions, 29 Pro tests, 27 license tests, 1 issuer test, 10 CLI tests, web builds, Rust tests, and Clippy. The Pro submodule was restored to `172e865`; no Pro gitlink change was committed, and the unrelated untracked planning docs remain untouched. The existing `LicenseState::is_enabled` warning remains.

At this earlier checkpoint, before the later approved fixture run on 2026-10-13, the Docker check could not run: `docker info` was denied at the local socket and the elevated probe reported that the daemon was unavailable. No Docker Desktop restart, image build, container start, or volume change had occurred yet.

### Backup-only root authority (2026-10-12)

Standalone operators can opt into backup/export writes with `BackupWrite` and an admitted `backupOnly` root. This root is excluded from catalog scanning and indexing, can serve as a restore source, and cannot satisfy writable-source requests. Existing `PhysicalSourceWrite` configurations retain their prior backup behavior. The refreshed Docker fixture exercised `backup_database` and selected export with `BackupWrite` and no physical-source capability.

An independent review found that nested paths could violate that separation even though the backup root was not mapped as a scan root. Runtime admission now rejects canonical `backupOnly` paths that overlap any other admitted root or the library root, in either containment direction, before acquiring the catalog lock or opening the database. Parent segments such as `..` are resolved during preflight, and missing library-root tails are preserved for comparison. Regression tests cover both root-overlap directions, disjoint and overlapping parent-segment paths, and rejection before database creation.

### Unified app build checkpoint (2026-10-12)

Pro commit `9ab3d49` pins Free core commit `172e865`. `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check:pro` passes, including 44 frontend tests / 1,198 assertions, 29 Pro tests, 27 license tests, one issuer test, 10 CLI tests, and web builds. `CARGO_NET_OFFLINE=true bun run build:release` also passes with an absolute `CARGO_TARGET_DIR` under `src-tauri/target/phase6-final`. The ARM64 bundle has identifier `com.sonicparke.stl-handoff`, passes `codesign --verify --deep --strict`, includes `server-web`, and its DMG passes `hdiutil verify`. Its executable SHA-256 is `514315ea6b952d4e16e70a50755d190c8175991471f7869152d591d5cbc0a1f5`. It was not installed or launched because its identifier matches the existing app and startup uses that app's Library data path. The existing unused `LicenseState::is_enabled` warning remains.

### Community standalone package and launch check (2026-10-12)

`CARGO_NET_OFFLINE=true bun run build:release` passed from public commit `172e865` with an absolute `CARGO_TARGET_DIR` under `src-tauri/target/phase6-final`. It produced `GoggleLab Community.app` and `GoggleLab Community_0.1.0_aarch64.dmg`. The app is ARM64, uses bundle identifier `com.cubiclejuice.gogglelab.community`, passes `codesign --verify --deep --strict`, includes `Contents/Resources/server-web`, and its DMG passes `hdiutil verify`. The executable SHA-256 is `5aee59b67655b117ee8706efe87fe2f804dc12c9e3bb66ac158d3eebea031e08`. This bundle was not installed or launched after the path-isolation change. An earlier isolated Community launch initialized a schema6 database, but CUA returned `cgWindowNotFound` twice; native visual interaction remains unverified, and the isolated app-data directory is retained.

The public workspace suite passes serially after the path preflight, JSON export, and host egress gate changes. The `library-runtime` suite has 68 passing tests, including root overlap, parent-segment normalization, no-database-on-rejection, unresolved-filament export, and host-opt-in transport coverage. Strict Clippy for the runtime and server crates passes with dependencies excluded. Existing Tauri and `stl-core` test-helper warnings remain.

### Docker image refresh after BackupOnly path fix (blocked, 2026-10-12)

`docker buildx build --platform linux/arm64 --load -f Dockerfile.server -t gogglelab-parity-server:phase6-20261012-172e865 .` returned `Docker Desktop is unable to start`. `docker desktop start` reported that the app was already running, but `docker desktop status` could not retrieve engine status; the latest read-only `docker info` probe timed out after 8 seconds. CUA could not capture a Docker Desktop window. No image was rebuilt and no container was started or modified. Resume with the synthetic loopback build/startup checks after the local Docker engine is available.

### Cross-target compilation probe (2026-10-13)

`CARGO_NET_OFFLINE=true cargo check -p stl-core --locked --offline --target x86_64-unknown-linux-gnu` and the equivalent `x86_64-pc-windows-gnu` check both pass. The broader runtime/server Linux cross-check cannot compile `aws-lc-sys` because `x86_64-linux-gnu-gcc` is missing; the Windows cross-check cannot be attempted because `x86_64-w64-mingw32-gcc` is not installed. These geometry-core checks do not substitute for Windows/Linux application builds or runtime/slicer launch tests.

Cross-platform native checks remain open. An independent Phase 6 review validated the backup-root overlap fix and export plan/write gates; the independent desktop-slicer security review found no evidence-backed findings. Final release review remains after Docker fixture verification, remaining parity decisions and native visual verification.

### Cross-target compilation probe (2026-10-13)

`CARGO_NET_OFFLINE=true cargo check -p stl-core --locked --offline --target x86_64-unknown-linux-gnu` and the equivalent `x86_64-pc-windows-gnu` check both pass. The broader runtime/server Linux cross-check stops in `aws-lc-sys` because `x86_64-linux-gnu-gcc` is unavailable. The Windows runtime/server check could not be attempted because `x86_64-w64-mingw32-gcc` is not installed. These geometry-core checks do not substitute for complete Windows/Linux app builds or runtime/slicer launch tests.

### Guarded source-file MCP actions and Docker fixture (2026-10-13)

The owner explicitly approved guarded MCP `trash_file` and `move_files`, plus recovery/purge/cleanup behavior only where it affects original model bytes. Source actions require intersecting `CatalogWrite` and `PhysicalSourceWrite` capabilities and a configured writable catalog root. The default server and Docker configs remain read-only. MCP accepts stable model IDs and expected metadata versions; move also takes an admitted destination handle and a safe leaf filename. It accepts no client source path. Every operation is a `confirm:true` planning call followed by execution of the returned expiring, principal-bound, one-use action token. Trash is recoverable under the app-owned library `trash` directory; moves preserve catalog model identity and reject collisions. Both recheck on-disk byte count and SHA-256 before source removal.

The owner has additionally approved extending the MCP aliases to composite multi-model actions. That work remains pending; current `trash_file` and `move_files` calls are single-model.

The implementation syncs copied bytes and containing directories on Unix before removing the original. These source-write tools are not advertised on Windows until durable directory synchronization is supported. An independent security review found no remaining broad data-loss window in the ordinary move/undo paths, but documented two P2 residuals: path-based operations retain a narrow same-user directory-swap race, and a concurrent same-user change can still occur between the last hash check and final cleanup. This API is intended for roots whose write access is controlled by the server operator; strict isolation against a concurrent local filesystem writer would require handle-relative operations. Cleanup removes only the verified `source.bin` entry and uses non-recursive directory removal.

Validation: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check`, `cargo fmt --all -- --check`, and `git diff --check` pass. The workspace check reports 133 frontend tests / 392 assertions, 138 Tauri tests with one ignored, 71 runtime tests, 32 server tests, and 3 server CLI tests; workspace Clippy passes with existing `stl-core`, Tauri settings, and browser lint warnings. Runtime regressions cover two-step quarantine/undo, one-use replay, source hash changes, path-free plans, move collisions, and successful hash-preserving move.

The local image `gogglelab-server:phase6-source-20261013` built successfully from the whitelisted Docker context. A fresh Compose project used a synthetic writable model bind and a temporary UID-10001 secret, published only to `127.0.0.1:3745`, and ran healthy. Authenticated MCP advertised the three source tools, scanned three synthetic models, quarantined and restored one with the same SHA-256, and moved another with the same SHA-256. AI/Spoolman tools remained absent. The task-owned container, network, named catalog volume, and temporary fixture directory were removed after verification; the local image remains cached. Pre-existing STL Vault containers and volumes were untouched. No personal model roots or external integration calls were used.

Phase 6 and overall parity are still open: composite source actions, embedded parent-model updates, filtered duplicate/hash jobs, subdirectory scan selection, full export/import round-trip compatibility, Windows source writes, the same-user path race and final-check window, native visual interaction, cross-platform builds, and final release review remain. At that checkpoint the pinned plan covered phases 1–6; provisional phases 7–9 were added after the local-source audit, pending current-upstream verification.

### Search parity, Docker refresh, and unified package (2026-10-13)

Free commit `79f02aa` completes the pinned `search_models` request shape for tags, file type, directory, and offset. Tags use bounded case-insensitive all-name matching; file type matches the parsed format and source/member extensions; directory filters require an admitted root handle or catalog-root ID; offset is limited to 100,000 and is exclusive with cursors. Offset pages return a keyset cursor for stable continuation. The snapshot does not define tag AND/OR or normalization rules. Current import support remains STL and 3MF; accepting a file type such as OBJ or STEP does not add import support for it.

Validation: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check` passes (133 frontend tests / 392 assertions; 137 Tauri tests / 1 ignored; 51 core; 71 runtime; 40 server; 3 server CLI; workspace Clippy). A dedicated root test confirms both admitted handle IDs and catalog-root IDs map correctly; core tests cover ZIP-backed `fileType` matching and root-scoped directories. Independent review findings on root mapping, ZIP extension matching, and the directory/root requirement were fixed; no search-slice findings remain.

Pro parent commit `a07af08` pins core `79f02aa`; `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check:pro` passes. The unified ARM64 app and DMG were rebuilt with an absolute `CARGO_TARGET_DIR` after a first attempt used a relative path that made the release wrapper sign a different bundle from the one Cargo wrote. The corrected app uses `com.sonicparke.stl-handoff`, contains ARM64 Mach-O and embedded `server-web` assets, and passes `codesign --verify --deep --strict`; `hdiutil verify` reports a valid image. DMG SHA-256: `7266b54948128e3e488539140e7d2b7e93187ba4a080133964bdb8c7950c24a1`. It was not installed or launched.

Docker Desktop was already running when checked. The refreshed image `gogglelab-parity-server:phase6-20261013-79f02aa` was built as `linux/arm64`. An authenticated loopback fixture listed 68 tools, scanned one synthetic STL, and verified root-handle, directory, file-type, and all-tags search. Source-write, provider-call, and desktop-slicer tools were absent under the default container policy. The task-owned container, data and secret volumes, and temporary model/token files were removed; the image remains cached and the existing STL Vault containers were untouched.

### Phase 6 final source-mutation, scan, and container verification (2026-10-13)

The current Free workspace passes `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check`: frontend checks and both browser/native web builds pass, the Rust workspace tests pass (Tauri 145 passed / 1 ignored; runtime 75; server 45; core 53; remaining crate tests pass), and workspace Clippy completes. Existing warnings remain in the browser lint, large web chunks, `stl-core` helpers/Clippy, and one Tauri settings test; the Phase 6 changes add no new Clippy warning. `cargo fmt --all -- --check` and `git diff --check` pass.

Review fixes include relationship-based 3MF root metadata selection; explicit-root refresh for empty catalogs; 1,000-model duplicate scope caps; scoped missing-file reconciliation with stable legacy absolute-path IDs; early slicer validation; and streaming desktop handoff. Handoff batches cap at 256 MiB; only marker-owned private stages are pruned after 24 hours on a subsequent handoff, with 1 GiB retained-storage limit. Unix stage directories/files use 0700/0600. Windows staging now resolves `LOCALAPPDATA` under `USERPROFILE`, rejects reparse points, and applies and verifies a protected DACL for the current user and SYSTEM; Windows GNU app and test-binary compilation pass, while native ACL/junction behavior remains unverified. Same-user path replacement races and crash-created unindexed copies remain documented limitations.

The final image `gogglelab-parity-server:phase6-20261013-current` was rebuilt as `linux/arm64`. An authenticated synthetic container ran healthy with a read-only `/models` mount, mode-0600 UID-10001 token volume, and only `127.0.0.1:3745` published. Unauthenticated model access returned 401. MCP exposed 69 tools; `trash_file`, `move_files`, `undo_trash`, `list_slicers`, AI, and Spoolman tools were absent under defaults. Headless `open_in_slicer` remained available only as a path-free `clientCommand` with a verified download lease; the lease returned the exact synthetic STL bytes. The fixture indexed three STL files, found the two nested duplicates, reported both hashes, then confirmed a subtree scan marked a deleted in-scope source missing while leaving a deleted outside source unchanged. The container, data and secret volumes, token, and synthetic files were removed; the final image remains cached. Other Docker containers and volumes were untouched.

Full Printventory JSON export/import round-trip remains unknown because the pinned snapshot omits an export document schema and has no `import_library` tool; ZIP backup/restore is the verified full-fidelity path. Native visual interaction and Windows/Linux application validation remain open. Full Printventory parity is not claimed.

### Pro sync, package verification, and local launch (2026-10-13)

Pro commit `2755084` pins Free core `f733c12`. `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check:pro` passes (44 frontend tests / 1,198 assertions; 29 Pro tests; 27 license tests; 1 issuer test; 10 CLI tests; web builds, Rust tests, and Clippy). The existing `LicenseState::is_enabled` dead-code warning remains.

`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/Users/bradmcalister/Documents/DEV/gogglelab-pro/src-tauri/target/phase6-final bun run build:release` produced an ARM64 `GoggleLab.app` and `GoggleLab_0.1.1_aarch64.dmg`. The app identifier is `com.sonicparke.stl-handoff`; it passes `codesign --verify --deep --strict`, includes `Contents/Resources/server-web`, and its executable SHA-256 is `0d2fe951a52b897755dc9f1fbc7cda3afddaf5a168ea90d8c6c43d18286f8c60`. `hdiutil verify` passed; DMG SHA-256 is `d047f94db24f959ae4afe633cbe06429557c10f2a5b7a282ef9a963c9b0695ce`. The previous installed app was preserved before replacement, and the verified build is installed at `/Applications/GoggleLab.app`; its running process is from that bundle. The separate Community Dev app was left untouched.

Native visual inspection remains unavailable: `orca open --json` returned `runtime_open_timeout` after macOS `task_name_for_pid` failure (5). The process and installed executable were verified, but the window appearance was not asserted. No push or release was made; the task branches remain local.

### Current-upstream audit and remaining compatibility work (2026-10-13)

This audit supersedes the initial Phase 6 upstream pin above for final parity closure. The current `TechJeeper/Printventory` main commit verified for this follow-up is `59171cb88a2d9160ed06931c01a29d41904fe552`. It contains a real MV3 browser extension, and `main.js` implements an app-level JSON importer. The importer accepts a top-level `models` array and reads only a subset of fields emitted by its export builder; there is no MCP `import_library` tool or published formal JSON schema. GoggleLab's parity work for the editable capture review, supported routes/download policy, and a safe reviewed importer is tracked in `docs/FREE-PRINTVENTORY-PARITY-PLAN.md`.

The MCP inventory has all 49 upstream names, but the audit found that `get_all_metadata` incorrectly required `modelId` and returned a single model. The follow-up implementation now accepts no arguments and emits counted designer/license/parent-model rows; a focused regression test passes. Other request-shape differences remain intentional where upstream accepts caller paths and GoggleLab requires stable model IDs, admitted roots, optimistic versions, or one-use plans.

The Windows handoff containment check now resolves `LOCALAPPDATA` and `USERPROFILE` before comparing them, with a focused injected-resolver regression test passing on macOS. This closes the lexical junction escape at the path-check level. Effective DACL enforcement and a real Windows junction test remain open until a Windows runner verifies them; do not claim Windows handoff privacy parity before that evidence.

### Phase 6 parity closure update (2026-10-13)

This section supersedes the earlier pending implementation items above; those entries remain as the discovery/history ledger.

- **MCP metadata:** `get_all_metadata` now accepts `{}` and returns counted designer, license, and parent-model rows. `get_model` keeps its separate `modelId` contract. The regression covers schema validation, counts, the separate detail call, and a parent model that was soft-deleted; the focused server test and full server suite pass.
- **Printventory JSON import:** the extension-compatible app flow opens a selected file, previews rows, and requires explicit per-row confirmation for each assigned target before applying supported metadata. `filePath` stays an in-memory hint; it is never sent to the OS or displayed. Unsupported printer/part/slicer fields are reported. The importer is partial by design because upstream has no formal schema and its app importer itself reads only part of the export. ZIP recovery remains the full-fidelity backup path. Focused importer tests pass (39 tests / 151 assertions).
- **Capture extension:** local extraction uses bounded page metadata, JSON-LD arrays, social metadata, and DOM/slug fallbacks. Recent snapshots support CDN download attribution by tab without a broad `tabs` permission. Optional Printables/Bambu API enrichment is off by default, requests optional hosts only from the settings action, and requires a separate popup Enrich click. Requests use numeric catalog IDs, `credentials: omit`, redirect rejection, a shared six-second deadline, and a 64 KiB response cap; results remain editable. Tests use mocked fetch/permissions only. The extension suite passes (29 tests); the Impeccable detector reports no findings and no ignores were added.
- **Windows handoff:** the implementation canonicalizes profile paths, rejects reparse points, applies and verifies protected DACLs for the current user and SYSTEM, and verifies owner SID. Setup failure cleans up only the empty root created by that attempt, and traversal enforces the 16,384-entry cap before enqueueing children. The focused macOS handoff suite passes (25 tests). The Windows GNU application check and app test-binary link now pass; Windows runtime/DACL behavior remains unverified. Redirected `LOCALAPPDATA` and owner SID mismatches fail closed. Windows privacy parity is not claimed.
- **Integrated verification:** `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check`, `cargo fmt --all -- --check`, and `git diff --check` pass. The macOS ARM64 app bundle builds, passes `codesign --verify --deep --strict`, uses `com.sonicparke.stl-handoff`, and contains `Contents/Resources/server-web`; it was not installed or launched.
- **Docker fixture:** the rebuilt `linux/arm64` server image passed with one synthetic STL, an authenticated server key, UID 10001, a read-only root filesystem, read-only source and secret mounts, and only `127.0.0.1:3745` published. Health returned 200; unauthenticated model access returned 401. The model remained indexed after container restart, its source SHA-256 stayed unchanged, and MCP exposed 69 tools including a successful no-argument metadata call. Source-write and provider tools were absent under the default container config. The task-owned container, volumes, token, and fixture directory were removed; existing STL Vault containers and volumes were untouched.
- **Catalog maintenance:** MCP/REST purge and orphan cleanup require `confirm: true` plus a separate expiring one-use action token. Purge writes a verified backup to an admitted `backupOnly` root, rechecks managed hashes, preserves configured index-root settings and the previous Library directory, and never changes external model roots. A no-overwrite journal is atomically published and synced before staging is created; startup removes incomplete journal-owned staging and unpublished temp files, recovers interruption after either rename, and rejects symlinked roots or a previous-directory name not bound to the journal nonce. Post-swap marker/journal failures keep maintenance armed. Cleanup checks references, active viewer/content leases, and file hashes before deleting only generated thumbnails or indexed-cache entries; partial I/O errors report completed counts and require replanning. Focused runtime and MCP tests pass. Purge remains disabled on non-Unix hosts until durable directory synchronization is supported there.

At this checkpoint the plan covered phases 1–6. A later local-source audit added provisional phases 7–9 for slicer profiles, printer management/maintenance, and parts stock; verify those features against current upstream before implementation. Phase 6 closure gates remain Windows application/DACL testing, native visual interaction, Linux desktop packaging/runtime, and final release review. JSON compatibility and the same-user path-replacement boundary remain limitations. No full parity or cross-platform release claim is made.

### Phase 6 MCP and browser capability follow-up (2026-10-13)

- `update_models_batch` now accepts replacement `tags` and local `filamentIds` per model in the same SQLite transaction as scalar metadata. Every row uses its own expected version; stale or invalid later rows roll back all earlier metadata and relation changes. JSON Schema now advertises nonblank tag strings and unique tag, filament, and model IDs.
- `check_files_exist` accepts either explicit stable model IDs or a bounded catalog scan (500 default, 2,000 maximum; missing-only by default). The description now discloses its 256 MiB aggregate source-byte budget and that one larger source cannot be checked by this tool. Focused tests cover the no-argument scan, limit, missing-only behavior, explicit IDs, schema, and atomic batch updates.
- Browser management availability is derived independently: source actions require source-write access; catalog purge requires a backup-only destination plus catalog, maintenance, and backup-write capabilities; orphan cleanup requires maintenance and media-write. An authenticated synthetic browser check showed purge and cleanup while physical source actions remained disabled. The page had no console errors after adding an inline SVG favicon.
- Verification passed: `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check`, `cargo fmt --all -- --check`, `git diff --check`, and the focused MCP regressions (2 source-check tests, 1 heterogeneous-batch test). The `linux/arm64` image `gogglelab-phase6-review:20261013-a` built successfully. Its loopback-only fixture ran as UID 10001 with a read-only container root, read-only synthetic model mount, mode-0600 token volume, and a separate backup-only volume; health returned 200 and unauthenticated catalog access returned 401. The synthetic source SHA-256 was unchanged after restart. The task container, volumes, token, and temporary fixture were removed; pre-existing STL Vault containers remained running. The local image tag remains cached.

The host-independent browser check and Linux ARM64 package smoke do not close native macOS visual inspection, Windows runtime/DACL execution, broader Linux host/display coverage, remaining JSON round-trip gaps, or final release review. The current plan includes provisional phases 7–9 based on a cached upstream snapshot; current-main verification is still required.

### Final continuation: JSON round-trip, journals, and packaging (2026-10-13)

- JSON import/export now preserves the supported model metadata, stable parent ID, categorized tags, local filament identities, current print status, and print-history event multiplicity. The exporter keeps Printventory's string `tags` and display-name `parentModel` contract; `gogglelabTags` and `gogglelabParentModelId` are local extensions. The JSON file includes `filePath` for Printventory compatibility, while imports use it only as an in-memory match hint. Reviewed import treats explicit null/empty nullable text as a clear, absent fields as no-ops, and accepts safe plain source-origin labels and well-formed, credential-free HTTP(S) URLs with case-insensitive schemes. Printer, part, and slicer collections remain unsupported; archived filaments stay excluded from current model assignments while exact archived identities are preserved in print history; valid event `created_at` is preserved through a dedicated local import command, ordinary Tauri, MCP, and HTTP create/update inputs cannot override it, omitted values are generated, and malformed values are skipped. The pinned upstream has no formal JSON schema, so extension acceptance is not externally verified.
- MCP/REST mutations use handle-relative operations and one-use/hash/version guards where supported. Native Tauri commands use handle-relative filesystem operations with separate local version, hash, and command-confirmation checks; they do not consume MCP/REST one-use tokens. Runtime restore and purge journals distinguish precommit rollback from committed-root preservation; Tauri restore and purge publish compatible journals, and Tauri purge preserves validated index-root settings. Startup handles injected crash states. Unsupported platforms fail closed for durable source mutations.
- Remaining security boundary: startup purge and restore recovery now read journals, inspect nonce-bound child state, rename, and remove through a pinned parent handle. Configured-root normalization and initial root checks still use path APIs; concurrent same-user replacement of child names within the pinned parent remains possible. After recovery returns, LibraryRuntime::open still opens SQLite through the configured path, so a later same-user parent swap can redirect that open. On macOS/Linux, backup archive output is written through a pinned destination handle and atomically published with an exclusive rename; other platforms retain the path-based no-clobber backup wrapper. Backup source traversal and SQLite snapshotting, staged SQLite open, and native OS Trash still use path APIs with identity checks. These boundaries are not asserted safe against a concurrent same-user path replacement.
- Final `CARGO_NET_OFFLINE=true RUST_TEST_THREADS=1 bun run check` passes with 192 frontend tests / 715 assertions and 159 Tauri tests (1 ignored), plus the remaining workspace tests, both web builds, and Clippy. Backup regressions verify a seekable archive's hash/round trip, atomic no-overwrite publication, a destination created after preflight, and cleanup after an over-limit archive. The active-lease regressions remove an index root after issuing archive-member and generated-thumbnail leases, run cleanup, and verify the exact leased bytes still stream. Purge and restore recovery regressions swap the parent after handle admission and confirm recovery changes only the pinned namespace. The latest macOS ARM64 Community bundle and DMG were built into `/tmp/gogglelab-phase6-created-at-target`; the code signature, `hdiutil verify`, and embedded web-assets check pass. DMG SHA-256: `68816341056dfc03cf8761f031ab321f1543bfb799ae3252cf9a26cfdeaeec3a`; executable SHA-256: `1eb66a8540e7bf36c60d6a1620aa03bcd209b18288e9c9242ce92d62cf754eac`. The bundle was not installed or launched.
- The final `linux/arm64` image `gogglelab-parity-server:phase6-20261013-final` passed an authenticated loopback fixture: UID 10001, read-only container root, read-only synthetic model/secret mounts, `127.0.0.1:3745` only, health 200, unauthenticated capabilities 401, one synthetic STL indexed and still present after restart, 69 MCP tools with source-write/provider/host-slicer tools absent under defaults, and unchanged source SHA-256. Fixture container, volumes, token, and temp files were removed; existing STL Vault containers and unrelated volumes were untouched. The local image remains cached.

The local-source audit added provisional phases 7–9 for slicer profiles, printer management/maintenance, and parts stock, subject to current-upstream verification after Phase 6 closes. Linux ARM64 `cargo build --release -p gogglelab` succeeded. Earlier Windows GNU checks passed in a disposable MinGW container before the final backup fallback; a fresh target check on this host stopped because `x86_64-w64-mingw32-gcc` is unavailable, so the latest fallback is not cross-compiled here. These compile checks do not prove native app runtime behavior. Native Windows application/DACL execution, Linux desktop packaging/runtime, native visual inspection, current-upstream verification, remaining JSON compatibility, documented same-user path residuals, and final release review remain open; full Printventory parity is not claimed.



### Phase 6 continued hardening (2026-10-14)

Runtime startup purge and restore recovery now open the parent once, inspect nonce-bound child state through that handle, and perform sibling renames, staging removal, and journal cleanup relative to it. Deterministic tests replace the parent path after handle admission and verify recovery restores the original Library without modifying decoy entries. Both tests failed before the handle-relative state checks and pass now. The full serial workspace gate and the rebuilt Community app/DMG pass; current counts and artifact hashes are recorded above.

The backup module now exposes a seekable archive writer while retaining `create_backup` as a path-based compatibility wrapper. On macOS/Linux, runtime and Tauri backup destinations use an exclusive temporary file under a pinned destination parent, hash and sync the archive, then publish it with a no-overwrite rename. Oversize cleanup removes only the same opened temporary file; a deterministic race test confirms a destination created after preflight remains untouched. Other platforms retain the path-based no-clobber wrapper.

The remaining same-user boundary includes configured-path normalization, replacing sibling child names inside the pinned parent, and reopening SQLite by its configured path after recovery returns. On macOS/Linux, runtime backup-asset enumeration and opens now use the pinned source directory; SQLite VACUUM INTO snapshots and the path-based compatibility adapter remain path-based. Non-macOS/Linux backup publication and native OS Trash also retain path-based operations.

Validation: the backup/runtime suites pass (5 backup tests and 104 runtime tests), including a regression proving ordinary print-history creation ignores a supplied `created_at`. The native management suite passes (15 tests), the full serial workspace gate passes, and the rebuilt Community app signature and DMG verify. A fresh Windows target check could not compile because this host lacks `x86_64-w64-mingw32-gcc`.

The updated `linux/arm64` image `gogglelab-parity-server:phase6-20261014-backup-publication` passed a synthetic loopback fixture. It ran as UID 10001 with a read-only root, read-only model/secret mounts, and only `127.0.0.1:3745` published. Health returned 200, unauthenticated capabilities returned 401, and authenticated REST backup planning/execution produced an archive whose on-disk SHA-256 matched the runtime result, with no temporary files left. The task-owned container and data/backup/secret volumes were removed; the uniquely tagged image remains cached. Existing containers and volumes were untouched.


### Phase 6 verification refresh (2026-10-14)

The compatibility backup walker now charges twice each relative asset path, matching the runtime adapter, and separately bounds the absolute paths retained by the active and queued directory traversal. Its limit is injectable for regression coverage. The test uses a long synthetic root, succeeds when the relative-path budget plus retained root fits, and rejects a cap below the retained root length. The independent review confirmed the original P3 finding is resolved. Focused library-backup, library-runtime, and gogglelab tests pass; the full serial workspace gate, formatting check, and git diff --check pass.

The rebuilt ARM64 Community app verifies under strict code-signature validation; hdiutil verify passes and the embedded browser assets are present. DMG SHA-256: b2efd8435f713eb16ac05bbbaa0356bdd29bcc9c77d632728122d3da2fd3ca77; executable SHA-256: 44e3b2d8273ffadc8e86ebb6c70dcdbe479ea157b636f0770a8a62a5f3efdf71. It was launched from the bundle through Launch Services under a temporary HOME and closed after visual QA; it was not installed or replaced. The isolated catalog indexed two synthetic STL files and native search filtered them to one.

The latest linux/arm64 server image is gogglelab-parity-server:phase6-20261014-path-budget (sha256:c72dcfefc76047b96288511a25bce20857419e6ce73c341bf113638b614d541d). Its synthetic container ran as UID 10001 with a read-only root, synthetic read-only model and secret mounts, and a single loopback publication at 127.0.0.1:3746. Health/auth checks returned 200/401/200 for health/unauthenticated/authenticated requests. Authenticated backup planning/execution returned SHA-256 14ed5ce3edb08aba0b50d782f7ab1127b838aa0101368d76fb6ba9fa1f466c76, matching the only file in the backup volume. The container, volumes, token, and temporary directory were removed; the image remains cached.

The Impeccable detector's flat hierarchy finding in browser-extension/options.html was real and is fixed with a scoped 2rem primary heading. At the 14px root size, the hierarchy is 14px body text, 18.9px section headings, and 28px primary heading. No ignore was added. Phase 6 remains open for the same-user mutation redesign, native Windows runtime/DACL checks, and final release review. Current-main verification is complete at commit 59171cb; full JSON round-trip remains after Phases 7–9.

Direct cross-target checks on the macOS host cannot find MinGW GCC or a Linux pkg-config sysroot. A current-source Windows check passes in an isolated Debian ARM64 container: cargo check --workspace --locked --target x86_64-pc-windows-gnu succeeds, and cargo test --no-run -p gogglelab links the Tauri test binary. The source was copied to a task-owned temporary workspace; the container, image, volume, and source copy were removed. This verifies compilation/linking only; native Windows DACL/junction runtime remains open.

The Linux ARM64 Community Debian package was built from the current copied workspace using prebuilt web assets. dpkg-deb reports Architecture arm64 and the expected WebKit/GTK dependencies. The extracted package executable survived a 20-second Xvfb launch with a synthetic HOME; the only log message was a non-fatal accessibility-bus warning. The package was not installed on the host and is retained at /tmp/gogglelab-phase6-linux-artifacts/gogglelab-community_0.1.0_arm64.deb (SHA-256 af824ae223659ecde4b60bded38cd9edb31255d5141ce65e9a0ed2e108d13be5). The temporary container, source copy, and target volume were removed.

The supported-field importer/exporter contract had one local round-trip defect: a parent ID exported from another Library prevented a safe unique match through parentModel. The importer now retains a matching non-self stable ID as authoritative, otherwise tries only a unique non-self local display-name match. An ID equal to the selected child fails closed without name fallback. Ambiguous and unmatched names remain warnings and do not create a relation. The regression failed before the fix; the focused importer suite now passes 39 tests / 151 assertions and requires explicit per-row confirmation for every assigned target. Full round-trip coverage for printer, part, and slicer collections is sequenced after Phases 7–9 add those Free data models; the final acceptance gate remains after Phase 9.


### Linux desktop package and smoke verification (2026-10-14)

The current source produced GoggleLab Community_0.1.0_arm64.deb in a disposable Debian ARM64 container using the already-built web assets. dpkg-deb reports Architecture arm64 and the expected WebKit/GTK dependencies. The extracted package payload was launched under Xvfb with a fresh temporary HOME and no published ports; the process stayed alive for 20 seconds. The only launch log was a non-fatal accessibility-bus warning. The .deb is retained at /tmp/gogglelab-phase6-linux-artifacts/gogglelab-community_0.1.0_arm64.deb with SHA-256 af824ae223659ecde4b60bded38cd9edb31255d5141ce65e9a0ed2e108d13be5. Temporary source copy, target volume, and container were removed; the package was not installed on the host.

### Same-user catalog identity continuation (2026-10-14)

`LibraryDb` retains a catalog file handle, compares device/inode against the visible path, and on Unix checks SQLite's `SQLITE_FCNTL_HAS_MOVED` before schema initialization and after use. A deterministic test swaps in a valid decoy only during SQLite open, then restores the original path; open now rejects the decoy connection before schema work. A SQLite commit hook rechecks the visible main database identity and aborts transactions when a replacement is present at commit. Runtime reads and writes verify identity before and after callbacks. Guarded server quarantine, undo, and move carry the existing connection through maintenance. Desktop source Trash and undo use that pinned connection and check identity around source changes.

The serial `bun run check` passed with 194 frontend tests / 729 assertions, both web builds, workspace tests, and Clippy. Formatting and `git diff --check` passed.

The current ARM64 Community app and DMG passed strict signature verification and `hdiutil verify`; DMG SHA-256 is `035f1ff22d4a134056e8bf38c7d4a03344f146246f56b0280d49faf10ca1711c`, executable SHA-256 is `861d7cec9051145241867ac84fcfa6966ea7f549d1cb233a97e8a33f91039364`. Launch Services opened the bundle with a temporary HOME; visual QA showed the two synthetic models, the selected `qa-triangle.stl` dimensions, and the slicer handoff control. The temporary profile was removed.

The rebuilt `linux/arm64` image `gogglelab-parity-server:phase6-20261014-catalog-guard` (image ID `sha256:e8d80455069f8b944eb51b120fa2398363a3f5864f2f3a9d8c601ba7a49a5942`) passed the synthetic loopback fixture. It ran as UID 10001 with a read-only root, all capabilities dropped, read-only model/secret mounts, and only `127.0.0.1:3748` bound. Health/auth results were 200/401/200, the authenticated catalog reported one available synthetic model after restart, and the model's source SHA-256 remained unchanged. The container and its exact data/secret volumes were removed; the image remains cached locally.

The catalog VFS binds main database, rollback-journal, WAL, and SHM operations to an identity-derived alias. Public WAL/SHM sidecars are adopted only after identity checks, exact `VACUUM INTO` outputs are admitted per operation, and the regression suite verifies that two live connections keep one SHM inode. Backup and recovery snapshots use pinned catalog readers; disposable staging validation leaves no persistent alias. A narrow check-to-unlink race remains for private sidecar holds, and delegated `xShmMap` does not expose the mapped inode; both require a hostile same-UID replacement-and-restore, outside the owner's requested threat model. Native Windows DACL, junction, and mutation behavior remains unverified until the friend-run handoff is completed. Phase 7 stays gated on those checks and final package review.

### Original-file recovery boundary (2026-10-14)

The owner clarified that recoverable holds are for original model source files only. MCP and native orphan cleanup now permanently remove only unreferenced, hash-verified generated thumbnails and indexed-cache files, after checking active leases and revalidating file identity through the pinned Library handle. Unknown files, configured model roots, changed replacements, active viewers, and active leases remain untouched; cleanup responses report completed file/byte counts and expose no generated-file hold receipts. Runtime cleanup tests, all 169 native desktop tests, MCP maintenance tests, and protocol tests pass.

### Phase 6 Windows handoff refresh (2026-10-14)

The current Free candidate successfully cross-built for Windows x64 as an unsigned GNU PE executable: `/tmp/gogglelab-phase6-windows-artifacts-20261014/output/gogglelab.exe` (SHA-256 `7a5406ec73a90b0a2db0d18858be17bb9b119e3a2cad3b28429067f847315b8c`). This confirms target compilation/linking, not Windows runtime behavior. The test checklist and artifact identity are recorded in `docs/PHASE-6-WINDOWS-TEST-HANDOFF.md`; native DACL, junction/reparse, ordinary replacement, and slicer handoff checks remain with the friend's Windows machine. The source holds boundary is now original model files only; generated thumbnails and indexed-cache orphans are deleted after guard and identity checks. Phase 7 remains gated on those Windows checks and final package review. Visual QA of the latest macOS bundle is also still open because Orca returned `runtime_unavailable` and did not start.

The current ARM64 Community package was rebuilt after the file-identity changes. Strict code-signature validation and `hdiutil verify` pass; DMG SHA-256 is `0d23160050ca92cf39642b7d9f415705e17499ed5308a9c0ceef2e87615cc602`, executable SHA-256 is `1eec8b5fb7226a7e9b2422af887e440c18a3e8140b17ab2d236526ba20874d38`. The unsigned Windows executable and matching sanitized source archive are available together under `/tmp/gogglelab-phase6-windows-artifacts-20261014/handoff/`.
