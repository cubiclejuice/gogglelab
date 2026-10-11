# Phase 6: Free headless, browser, MCP and capture
Date: 2026-10-11
Status: shared runtime and authenticated catalog/capture HTTP, MCP and browser slices locally verified; remaining scan, geometry, management and integration transports are in progress.
All implementation is public MPL-2.0 Community with no license requirement.

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

## Network boundary authorization (2026-10-11)
Automatic approval review initially rejected non-loopback binding. The owner asked
for an explanation of LAN/Docker scope, then explicitly replied "approved" to
implementing opt-in LAN/Docker with mandatory auth, localhost default, desktop
off until enabled, and synthetic-data development containers published on localhost.
This resolves that code gate. It does not authorize live-library mounting, public
deployment, provider calls, DNS/certificate-account changes or external publication.
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
