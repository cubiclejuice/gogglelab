# Guarded browser and MCP printer networking

Status: local implementation, package, and synthetic visual acceptance passed. This is
required by the browser parity promise in the Free parity plan. Everything remains
Community/Free. Bambu MQTT and real-device validation remain deferred in issue #3.

## Authority and defaults

- Keep the desktop listener stopped by default. Printer LAN permission is separate
  from inbound browser exposure and from AI/Spoolman external integration access.
- Add a distinct `printerNetwork` capability and an operator-owned policy containing
  admitted private IPv4 scopes, `allowMulticastDiscovery`, and
  `allowCarbonBroadcast`. Both datagram permissions default to false. No policy
  means no network access. Browser/MCP requests cannot enable or expand this policy.
- Intersect principal capability, runtime capability, operator scope, and requested
  scope before any transport. Retain the existing 1,022-address, worker, request,
  response, suggestion, and total-deadline bounds; report actual limits to the UI.
- No live LAN, hardware, or provider call is authorized for implementation testing.
  Use synthetic transports and loopback fixtures. Existing one-shot approvals are
  consumed. Never send a new request to a real printer under consumed approvals.

## Interface and acceptance requirements

Use existing authenticated API/MCP conventions, strict unknown-field rejection,
sanitized errors, and decimal strings for versions. Protocol, OpenAPI, client, and
contract tests must agree on the operations below.

1. Read policy availability and limits without networking. Disabled hosts explain
   that the operator must configure access; clients cannot turn it on.
2. Create a discovery plan from a private IPv4 range. Return the normalized scope,
   address count, actual operations, multicast/broadcast reach disclosure, and an
   expiring principal-bound one-use token. Planning performs no network I/O.
3. Start the reviewed plan through a separate explicit action. Return an opaque
   owner-bound asynchronous job, since the transport deadline can consume the API
   request timeout. Bound active jobs, plans, results, and retention. Support owner
   polling and cancellation. Editing a range invalidates its browser review.
4. Return credential-free suggestions. Saving selected suggestions creates ordinary
   unapproved profiles; it does not start checks or approve endpoints.
5. Review/approve an exact saved endpoint using its expected version. Check that
   the endpoint is admitted by host policy. An endpoint edit invalidates approval.
6. Explicitly test one approved saved printer using its expected version. Require
   catalog-write permission for the saved result. Carbon MainboardID discovery
   requires a separate visible choice and explicit `allowCarbonBroadcast` permission.
   Keep MainboardID transient and server-side; do not accept arbitrary remote
   MainboardIDs, paths, URLs, credentials, or control commands.

Carbon discovery sends UDP broadcast to `255.255.255.255:3000` on the routed
interface. Multicast permission does not grant that broadcast permission. Show the
specific broadcast reach before the explicit Carbon action.

For discovery, expose only the actual implemented protocol choices. Unicast-only
policy must never silently send multicast. If a protocol cannot operate under the
chosen policy, explain that limitation before starting it. Multicast may reach
devices outside the selected/operator address scope; only in-scope replies may
produce candidates or subsequent connections.

## Runtime and persistence

| Operation | Required current capabilities |
| --- | --- |
| Availability summary | `catalogRead`; report unavailable without network authority |
| Plan, start, poll, cancel discovery | `printerNetwork` and `catalogRead`, plus job ownership |
| Approve/revoke a saved endpoint | `printerNetwork`, `catalogRead`, and `catalogWrite` |
| Test and save selected status | `printerNetwork`, `catalogRead`, and `catalogWrite` |

Recheck these on every request; an owner-bound ID never substitutes for current
authorization. Operator policy disable automatically cancels affected work.

- Reuse the shared transport; do not duplicate it in REST handlers. Require a live
  guard at construction and network boundaries. Revoke permission and cancel jobs
  on policy disable/shutdown. Bind every plan/job/check to a monotonic policy
  generation; disable/re-enable cannot revive old work. Before returning results
  or committing status, recheck generation, current permission, and cancellation.
  Linearize that final check and commit with revocation/cancellation so revoked
  or cancelled work cannot publish results or write status afterward.
- Acquire runtime I/O admission before transport. Read the saved version and
  approval, release the database mutex during I/O, and conditionally persist only
  if the endpoint, approval, and version remain current. Maintenance admission and
  catalog identity checks must remain effective. Stale inputs perform no I/O;
  concurrent edits cannot receive a stale result.
- Keep Carbon's correlated acknowledgement requirement and observed-event wording.
  No raw payload, MainboardID, serial, or credential reaches responses/logs.
- Advertise only installed, authorized operations in capabilities/OpenAPI/MCP.
  The browser controls must match the actual advertised interface.

## Build and acceptance order

1. Shared transport extraction: native compile, all moved tests, guard tests,
   independent comparison, and current app build. Preserve certificate bytes and
   dependency versions. Correct stale native QA-test references before proceeding.
2. Protocol/runtime policy and jobs: synthetic tests for scope intersection,
   disabled access, disable/re-enable generations, bounds, ownership, token
   expiry/replay, cancellation and revocation during final commit,
   no redirect/proxy/arbitrary URL, stale approval/version, and maintenance races.
3. Authenticated REST/MCP, operator configuration, and OpenAPI contracts. Test read
   versus write permissions, missing policy, strict fields, version precision, and
   absence of identity/credential/raw-status material from wire output.
4. Browser review/start/cancel/save/approve/test flow. Verify no automatic traffic
   after connect, save, refresh, or settings changes, and explicit Carbon disclosure.
5. Independent security review, production browser/native builds, and synthetic
   browser acceptance. Only then move to the next delivery gate in the main plan.

The unified app's production core-pin update also requires publication on public
`main` under its supported update script. Preserve unrelated private-repo work;
test composition separately and do not bypass that release integration gate.

## Verification checkpoint, 2026-10-19

The complete serial Rust workspace suite, workspace Clippy, formatting/diff checks,
and 326 frontend tests / 1,287 assertions pass. Runtime coverage includes ten API
tests and 73 shared transport tests; the server library suite passes 62 tests.
The isolated debug app and standalone Community production app both build. The
Community identifier and deep/strict ad-hoc signature are verified.

Independent review found and fixed scope-narrowing blocking endpoint revocation,
and a native settings refresh racing revocation and later AI-setting updates.
Both have regressions that failed before the fix. Browser inspection also caught
hidden buttons being displayed by CSS and cramped checkbox/consent layout; scoped
styles now keep inactive controls hidden and consent on its own full-width row.

The fake-only loopback example in
`crates/library-server/examples/printer_network_fixture.rs` proved interactive
review/start, suggestion review/unapproved save, separate approval/check, fake
Moonraker and Carbon online/printing results, cancellation, and disconnect/key
clearing. The owner completed native browser confirmations that automation could
not reach. Fixture operation counts were discovery=2/status=2. A separate initial
connect-only fixture recorded zero/zero. A fresh post-layout fixture was seeded
through guarded APIs for visual inspection (zero discoveries/one fake status).
The runtime asserts that the fake backend is installed before policy admission;
none of these fixtures can send printer traffic. All fixtures were stopped and
their evidence retained. Native isolated Settings separately shows the listener,
AI access, printer LAN access, multicast, and Carbon broadcast off; no permission
was enabled during that native inspection. Hardware acceptance remains open.
