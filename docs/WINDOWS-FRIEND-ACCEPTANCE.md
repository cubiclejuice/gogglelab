# Windows Community acceptance: Claude handoff

Give this entire file to Claude on the Windows test computer. Execute the plan below in order and produce the required report. This plan is not evidence that Windows acceptance has passed.

## Instructions for Claude

Verify the supplied GoggleLab Community (Free) Windows build using a disposable VM or separate disposable Windows account, an isolated application profile, and synthetic model files. Do not use a personal library, existing profile, Pro license, printer credentials, or provider API key. Do not scan the LAN, contact printers, make provider calls, publish builds, or send messages. Do not change source or weaken a guard to make a test pass.

Record `PASS`, `FAIL`, `BLOCKED`, or `NOT-RUN` for every case. Disabled operations, missing fixtures, unavailable UI tools, skipped native tests, and missing build identity are not passes. If UI automation is unavailable, give the human exact manual steps and wait for their observations; never claim screenshot QA without viewing the running Windows app.

Stop the affected test on unexpected original-file loss, authentication bypass, access outside configured roots, or undo overwriting a new file. Preserve logs, source hashes, and recoverable holds. Ask before installing prerequisites/installers, changing the system, executing a slicer/test executable, or permanently deleting anything. Each deletion approval must name exact synthetic paths and scope. Do not disable SmartScreen, antivirus, certificate checks, execution policy, or another security control.

## 1. Establish source and environment

The owner may have local uncommitted changes absent from GitHub. Require the exact owner-supplied installer artifact or exact source snapshot/commit containing the intended changes. Do not assume the default branch or newest release contains this work. Missing provenance is `BLOCKED`.

The candidate branch contains `.github/workflows/windows-community-test.yml`, designed for manual dispatch. It is not currently registered in the repository workflow list, so do not assume it can be dispatched or that artifacts exist. If matching artifacts are unavailable, use the approved source-build lane below or mark the installer lane `BLOCKED`; do not merge or change repository settings to enable it. When available, it produces `gogglelab-community-windows-<commit>` containing one NSIS EXE, one MSI, `BUILD-INFO.txt`, `SHA256SUMS.txt`, and this plan. Confirm the workflow ran on the intended revision. Record Windows edition/version/build, CPU architecture, VM/account isolation, PowerShell version, and WebView2 runtime version. The workflow targets `x86_64-pc-windows-msvc`; ARM emulation does not prove native x64 coverage.

## 2. Optional matching source-build lane

Run only with the intended source snapshot and human approval for prerequisites/building. Installer-only testing can continue without this lane, but native coverage remains `NOT-RUN` unless matching CI logs are supplied.

Use Bun **1.3.14**, Python **3.13**, stable Rust MSVC with Clippy, Visual Studio C++ build tools and Windows SDK, and WebView2. Install prerequisites only through approved official mechanisms. Record source commit/snapshot hash, `git status --short`, and tool versions; never reset, stash, clean, commit, or push another person's work. Use a short local NTFS checkout and a stable project-specific target directory.

Run the workflow sequence below, each command separately. Capture its full log and `$LASTEXITCODE` immediately; stop this lane at a failure. Record ignored/skipped tests by name and reason, including whether Windows-only DACL, reparse/junction, and replacement fixtures actually executed.

```powershell
bun install --frozen-lockfile
bun run check:frontend
python integrations/fusion/test_bridge.py
bun run build:web
cargo test --workspace --locked -- --test-threads=1
cargo clippy --workspace --all-targets --locked
bun run build -- --release --bundles nsis,msi --no-sign
```

Measure disk space before/after building. Inspect generated outputs after success, preserving the working build and latest known-good release. Follow `AGENTS.md`: inspect exact paths and active processes/open handles, run the exact scoped `cargo clean --dry-run`, and obtain exact approval before permanent cleanup. Never delete an entire Rust target, broad cache, source, Git history, configuration, credentials, user data, backups, or unknown files. If cleanup is not performed, report zero space recovered.

## 3. Verify artifacts and install

1. Read `BUILD-INFO.txt` and confirm the edition is Community, the artifact is marked as an unsigned test build, and the commit is the one requested for testing.
2. In the artifact directory, verify both installers against the manifest. Keep actual hashes in the report. Checksums detect corruption against the supplied manifest; confirm the artifact's origin with the owner separately.

   ```powershell
   $ErrorActionPreference = 'Stop'
   $installers = @(Get-ChildItem -LiteralPath . -File |
     Where-Object { $_.Extension -in @('.msi', '.exe') })
   if (@($installers | Where-Object Extension -eq '.msi').Count -ne 1 -or
       @($installers | Where-Object Extension -eq '.exe').Count -ne 1) {
     throw 'Expected exactly one MSI and one NSIS EXE.'
   }
   $lines = @(Get-Content -LiteralPath './SHA256SUMS.txt')
   if ($lines.Count -ne 2) { throw 'Expected two checksum entries.' }
   $seen = @{}
   foreach ($line in $lines) {
     if ($line -notmatch '^([0-9a-fA-F]{64})  (.+)$') {
       throw 'Malformed checksum entry.'
     }
     $expected = $Matches[1]
     $name = $Matches[2]
     $files = @($installers | Where-Object Name -CEQ $name)
     if ($files.Count -ne 1 -or $seen.ContainsKey($name)) {
       throw "Unexpected or duplicate checksum filename: $name"
     }
     $actual = (Get-FileHash -LiteralPath $files[0].FullName -Algorithm SHA256).Hash
     if ($actual -ine $expected) { throw "Checksum mismatch: $name" }
     $seen[$name] = $actual.ToLowerInvariant()
     "VERIFIED $($seen[$name])  $name"
   }
   ```

3. Take a clean VM snapshot/use a new disposable account. With approval, install one package and launch GoggleLab. Test MSI and NSIS in separate clean lanes, not layered over one another. If Windows blocks the unsigned installer, record `BLOCKED`; do not weaken security.
4. Confirm executable/version/build identity and Free access without a license. Verify there are no old roots, credentials, printers, or slicer trust decisions; stop if isolation failed. Capture the initial window/version.
5. Confirm the server/LAN listener is off by default, localhost is the default bind, and AI sharing defaults to local until explicitly opted in. Do not activate cloud AI. Check actual listening addresses when testing the listener; a settings label alone is insufficient.
6. Resize the window, navigate with the keyboard, and inspect library/settings dialogs for clipped controls, focus traps, unreadable text, or poor errors. Record screenshots of failures.

## 4. Synthetic library and junction checks

Create a disposable library and an outside target containing synthetic STL files:

```powershell
$root = Join-Path $env:TEMP ('GoggleLab-Windows-QA-' + [guid]::NewGuid().ToString('N'))
$library = Join-Path $root 'library with spaces'
$outside = Join-Path $root 'outside'
$evidence = Join-Path $root 'evidence'
$junction = Join-Path $library 'outside-junction'
New-Item -ItemType Directory -Path $library, $outside, $evidence | Out-Null
@'
solid triangle
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 1 0 0
vertex 0 1 0
endloop
endfacet
endsolid triangle
'@ | Set-Content -LiteralPath (Join-Path $library 'inside.stl') -Encoding ascii
Copy-Item -LiteralPath (Join-Path $library 'inside.stl') -Destination (Join-Path $library 'café & brackets [1].stl')
@'
solid outside
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 1 0 0
vertex 0 1 0
endloop
endfacet
endsolid outside
'@ | Set-Content -LiteralPath (Join-Path $outside 'outside.stl') -Encoding ascii
$sourcePaths = @(
  (Join-Path $library 'inside.stl'),
  (Join-Path $library 'café & brackets [1].stl'),
  (Join-Path $outside 'outside.stl')
)
$sourcePaths | ForEach-Object { Get-FileHash -LiteralPath $_ -Algorithm SHA256 } |
  Export-Csv -LiteralPath (Join-Path $evidence 'original-hashes-before.csv') -NoTypeInformation
$outsideHash = (Get-FileHash -LiteralPath (Join-Path $outside 'outside.stl') -Algorithm SHA256).Hash
New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
$root
```

Record the displayed exact fixture root. If junction creation is unavailable, mark that manual case `BLOCKED`; do not infer boundary acceptance. Use `-LiteralPath` for bracketed filenames.

Add `$library` as an in-place indexed root and scan. Verify both STL entries, thumbnails, selection, search/filter, tags/categories, favorites/collections, and basic management work in Free. Restart and re-scan: state must persist without duplicate entries. Originals must stay at their source paths with unchanged hashes after scanning, thumbnail generation, metadata edits, and catalog-only removal.

Check fast completion without closing/reopening Library: add another synthetic STL,
scan that root, and require the visible model count, source scan time and preview
to update. Repeat a quick scan and a preview job; a completed job must not leave
the old catalog visible. Create and rename a collection through the visible
labelled in-app dialog, test Cancel/invalid names, and verify keyboard focus stays
in Library after a successful refresh. Missing native prompts or refresh only
after reopening are failures.


Check the catalog purge confirmation without deleting anything: choose an exact
synthetic backup destination, confirm it is visible, enter lowercase `purge` and
require rejection, then Cancel/Escape and verify the catalog is unchanged. Execute
exact uppercase `PURGE` only after the separate exact synthetic-operation approval
in section 6. Require a verified backup, retained previous Library and unchanged
original-source hashes.

Check the manual original-Centauri ID dialog only with networking disabled or an
owner-supplied fake backend that cannot contact a LAN. Verify the disclosure,
invalid-input rejection, Cancel/Escape, cleared input after reopening, readable
layout and keyboard focus. Do not submit an ID or test a real endpoint in this
Windows checklist. Missing isolation is `BLOCKED` for this case; live printer
validation is separate. Network profile metadata save/edit must retain its scheme
and authority; metadata persistence does not prove protocol compatibility.

Test valid owner-supplied synthetic 3MF/ZIP fixtures separately, including grouping and safe extraction. Do not rename arbitrary bytes to those extensions. Missing fixtures are `BLOCKED`. Malformed input must give a safe error without crashing or writing outside the test area.

Confirm `outside.stl` is not indexed/reachable through the junction. Use only app actions expected to reject an out-of-root path; do not manually move, trash, or edit the outside file. Recheck:

```powershell
(Get-FileHash -LiteralPath (Join-Path $outside 'outside.stl') -Algorithm SHA256).Hash -eq $outsideHash
```

Expected `True`. Retain the junction and fixtures until exact cleanup approval; never recursively traverse the target to remove the link.

## 5. Slicer profiles, trust, handoff, and DACL

Create/edit/default/remove test slicer profiles in Free. Select the executable through the local picker and explicitly trust it. JSON-imported paths/arguments must not grant launch authority automatically.

With approval, use an installed supported slicer or an owner-reviewed harmless argument-recording fixture executable. Do not download an unknown stub, pick an unrelated executable, run imported shell commands, or send a print. No suitable executable means handoff is `BLOCKED`, even if profile CRUD passes.

Hand off both STL filenames. Verify the receiving process gets each path as one literal argument with no shell expansion/extra command, and originals retain their hashes. A slicer opening a model proves opening; literal-argv proof requires the approved recording fixture or matching automated tests. Test changed executable identity only with a dedicated approved fixture, never by replacing an installed slicer. Handoff must revalidate/reselect or fail closed.

While the handoff directory exists, inspect its owner and ACL. If the convention below differs from the running build, locate the actual directory from test/app output and record it; do not inspect an unrelated directory:

```powershell
$libraryName = Split-Path $library -Leaf
$handoff = Join-Path $env:LOCALAPPDATA "GoggleLab-$libraryName-handoff-v1"
if (Test-Path -LiteralPath $handoff) {
  $acl = Get-Acl -LiteralPath $handoff
  $acl.AreAccessRulesProtected
  $acl.Owner
  $acl.Access | Format-Table IdentityReference, FileSystemRights, IsInherited
}
```

Expected: protected DACL, owner is the current user's SID, and access limited to that SID and `SYSTEM`; no broad `Users`, `Everyone`, or `Authenticated Users` entry. If the directory is already gone, manual ACL inspection is `BLOCKED`; automated native-test evidence is a separate result.

## 6. Guarded originals, holds, and ordinary concurrency

Recoverable holds cover original synthetic model sources only; rebuildable thumbnails/previews do not need them. Concurrent-change scope means ordinary other-process replacement, not a malicious same-user program bypassing advisory locks.

Before manual move/trash/purge/undo, list exact fixture paths and obtain approval for that sequence. Use supported interfaces. If Windows mutations are disabled pending acceptance, record `BLOCKED`; do not enable switches/remove guards just to pass.

- Record source hash, planned destination, confirmation, hold, and final hash for each operation. Check configured-root restrictions, one-use confirmation, and stale hash/version rejection. Reusing confirmation must not mutate again.
- Exercise ordinary source replacement after planning but before execution using an available deterministic fixture/hook. Expect failure closed or recoverable preservation of changed bytes. Clicking quickly is not race evidence. Without a deterministic fixture, mark the manual case `BLOCKED` and cite matching Windows automated results separately.
- Verify a successful guarded mutation retains expected original bytes recoverably. Undo must restore the expected hash and refuse to overwrite a new destination collision.
- Exercise supported restart recovery with pending holds. Verify holds remain visible/recoverable and junction/reparse paths cannot escape configured roots. Do not force-kill during an arbitrary write and call it the supported fixture.
- Keep purge/permanent deletion `NOT-RUN` without exact approval for the synthetic hold. Do not delete an unreviewed orphan path.

For a supported source-build fixture, record the staging entry set before and
after default thumbnail indexing. It must not gain retained model-source copies;
original bytes and an unrelated staging sentinel must remain unchanged. Generated
purge/restore control metadata has a separate bounded receipt/recovery path.
Platform-disabled atomic exchange or directory synchronization must stay disabled
on Windows; record that coverage as `BLOCKED`/unsupported, never enable it just to
pass. A Unix test result does not establish Windows mutation safety. Do not delete
historical holds based on a generated-looking name.

Recheck outside/unrelated original hashes. Preserve holds and logs for review.

## 7. Offline printer history, stock, JSON, and recovery

Use synthetic data only. No discovery, printer connections, credentials, or provider calls.

1. Create/edit/delete synthetic printer profiles, maintenance logs, and reminders. Check timestamp/nickname snapshots, restart persistence, and unlinked imported history. Imported endpoints must remain unapproved; connection/job status stays unknown. CRUD does not prove hardware compatibility.
2. Create stock 10, record a local event consuming 2 (stock 8), edit usage to 3 (stock 7), then delete that explicitly stock-adjusted event (stock 10). Insufficient-stock rejection must leave all quantities/rows unchanged. Record observed values.
3. Import historical events with usage, then replay. Neither import may deduct stock; replay must not duplicate rows. History edit/delete must not invent refunds. Legacy pre-v10 events have unknown adjustment state: verify warning/no inferred refund. Missing part IDs must not silently link a same-name replacement part.
4. Export/import JSON into another disposable profile using the supported mechanism, then export again. Compare semantic fields/links, optional/null printer refs, timestamps, nickname snapshots (including empty legacy ones), stock, and event usage. IDs/order may legitimately differ; preserve both exports and source fixture.
5. **Slicer round-trip gate:** the pinned Printventory slicer shape is `{name, path}`. Verify saved custom profile names and exported path hints round-trip through the supported import flow, with each imported executable explicitly reselected and trusted locally. JSON must not grant executable trust/identity or supply launch arguments. Existing local profiles and the selected local default must remain unchanged; the format carries no imported arguments or default selection. If an older/broken build still exports `slicers: []` despite populated custom profiles, ignores imported slicers, or only warns of unsupported data, mark this case `BLOCKED` and full parity incomplete. Intentional local reselection/trust is not lost metadata.
6. Use supplied synthetic v9/v10 databases to test migration to schema **11**, normal automatic backup, retained history/versions/FKs, unknown stock-adjustment state, and empty nickname recovery. Without fixtures, migration is `BLOCKED`; do not fabricate a legacy database or alter a personal one.
7. Back up/restore synthetic data to a second isolated profile. Compare source hashes, catalog metadata, history, stock, and links. Malformed/stale input must fail safely. Retain backups/logs; never restore over personal data.

## 8. Optional loopback server

Deliberately enable the supported listener only in the disposable profile and retain loopback binding. Record actual address/port, verify protected unauthenticated requests fail and an authorized local read works, and perform the browser checks below before stopping the listener. Do not record tokens in logs/screenshots.

While that synthetic listener is running, connect the browser client with its temporary key. Confirm Printer fleet is advertised and available without a license. Create/edit a synthetic profile, select it, add maintenance and a reminder, and check persistence after a controlled fixture-server restart. A notes-only maintenance edit must preserve the exact saved timestamp, including seconds/milliseconds. Pause the reminder: Complete must be absent until Resume; completing it afterward must create one maintenance record and update the reminder atomically. Record the observed values. Keep printer network policy disabled and do not click discovery/status actions. Disconnect and verify the browser reports its key cleared from memory, then disable the listener and verify the socket closes. A missing fleet route/control is a failure, not a skipped success.

LAN/Docker networking, live printers, MCP source-mutation clients, and provider calls need separate exact authorization and are otherwise `NOT-RUN`. Loopback success does not prove them.

With the matching approved source-build lane, optional printer-network browser QA
can use the deliberately fake example below. Its output directory must not exist.
It binds only `127.0.0.1:3847`, installs a static synthetic backend before admitting
policy, and cannot contact printers. The printed fixture key is public test data.

```powershell
$fixture = Join-Path $env:TEMP ('GoggleLab-Fake-Network-' + [guid]::NewGuid().ToString('N'))
cargo run -p library-server --example printer_network_fixture --locked -- $fixture
```

Open the printed localhost URL and use its fixture-only key. Review scope
`10.250.0.10/32`, then separately Start. Save the returned Synthetic Moonraker as an
unapproved profile; approve its displayed endpoint, then explicitly check status.
For the fake Carbon consent flow, create make `Elegoo`, model `Centauri Carbon`,
host `10.250.0.11`; approve it, verify status stays disabled until its separate
broadcast checkbox is selected, then confirm one fake check. Expect synthetic
online/printing. Test a second discovery cancellation and browser disconnect/key
clearing. Stop with Ctrl+C and retain the printed synthetic operation counts.
These results prove guarded UI/API behavior only, never Windows printer hardware
compatibility. A fixture failure is `BLOCKED`; do not remove a guard to proceed.

## 9. Uninstall, second installer, and cleanup

With approval, uninstall through Windows' supported interface. Verify application/shortcuts are removed and synthetic source files remain unchanged. Record whether synthetic app data persists; do not manually erase it. Test reinstall/data retention within that lane if desired, then use a separate clean snapshot/account for the other installer.

Do not remove fixtures, junctions, holds, evidence, installers, checkout, or build outputs without exact approval. Recheck active processes/open handles before cleanup; inability to inspect is not evidence of inactivity. Measure free disk space before/after and report the measured delta, or `0 bytes (no cleanup performed)`. Preserve working/known-good builds and never clean broad targets/caches/user data.

## 10. Required report

Write `WINDOWS-ACCEPTANCE-REPORT.md` and an `evidence/` folder in the approved test workspace. Redact personal paths/usernames, tokens, serials, and private network details before sharing; retain exact local records if needed. Do not upload or message automatically.

```markdown
# GoggleLab Windows acceptance report
Date / tester / Claude session:
Windows edition/version/build / CPU / VM or account isolation:
WebView2 / PowerShell / Bun / Python / Rust / MSVC versions:
Source commit or snapshot identity / dirty-state manifest:
Artifact origin / workflow run / BUILD-INFO:
MSI filename + SHA-256:
NSIS filename + SHA-256:
Synthetic root (redacted in shared copy):

| Check | MSI lane | NSIS lane | Evidence / reason |
| --- | --- | --- | --- |
| Provenance / checksums / install / launch | | | |
| Native suite / Windows-only and ignored fixtures | | | |
| Free defaults / UI / keyboard | | | |
| In-place library / thumbnails / persistence | | | |
| Original hashes / valid 3MF and ZIP / malformed input | | | |
| Junction / out-of-root denial | | | |
| Slicer CRUD / trust / argv / DACL / replacement | | | |
| Confirmation / race / original hold / undo / restart | | | |
| Offline printer history / maintenance / reminders | | | |
| Part stock / historical import / replay | | | |
| JSON round-trip including slicers | | | |
| Schema 11 / backup / recovery | | | |
| Optional loopback authentication | | | |
| Browser fleet / exact dates / pause / completion / persistence | | | |
| Uninstall / source preservation | | | |

## Commands and exit codes
## Findings and exact reproduction steps
## Screenshots and logs viewed
## Original hashes before and after
## Blocked, not-run, skipped, and ignored tests
## Cleanup approvals / exact scope / disk before and after / space recovered
## Verdict
PASS / FAIL / BLOCKED; list every remaining acceptance gate.
```

Do not declare full Windows acceptance while required cases remain blocked/not run. A build, a launch, or cross-platform tests alone cannot establish it.
