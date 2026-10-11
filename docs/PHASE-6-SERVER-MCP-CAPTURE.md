# Phase 6: Free headless, browser, MCP and capture
Date: 2026-10-11
Status: discovery/spec only; implementation must wait for the Phase 5 full gate.
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
Full transport, engine and UI plan challenges remain pending after Phase5 gate.
