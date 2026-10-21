# Unreleased: Free library, server, and printer management

These changes are implemented locally and are not a published release. Current delivery
gates are tracked in the [Free parity plan](FREE-PRINTVENTORY-PARITY-PLAN.md). The official
unified app must be rebuilt against the updated public core before these features can be
claimed for that download.

Free includes indexed folders and thumbnails, managed library organization, ratings and
favorites, recoverable source removal, backups and recovery, multipart previews, slicer
handoff and custom slicer profiles. Printer profiles, maintenance logs and reminders,
parts stock and transactional per-copy print-event consumption are included. Reviewed
JSON import/export preserves supported printer/history/parts data and saved slicer
names/path hints; imported apps require local selection and explicit trust before launch.
Schema 11 preserves unlinked printer-history snapshots, with a backup before migration.

The authenticated headless server, browser library, MCP integration and reviewed browser
capture inbox are implemented. Localhost and read-only server defaults remain; remote
access and write capabilities require explicit configuration. Native source operations
use configured roots, version/hash checks and recoverable holds for original files.
Browser printer fleet, maintenance, and reminders now share the versioned runtime.
Optional printer LAN access requires a separate host policy and stays off by default.
Discovery requires scope review plus a separate Start; suggestions save unapproved.
Selected-printer checks require approval and current versions. Bambu multicast and
Carbon broadcast are independent choices. Clients cannot expand host permission.
Revocation/cancellation prevent stale results from being saved. Synthetic browser
acceptance is separate from hardware and current Linux/container delivery checks.

Free adds a local filament catalog and model/history assignments. Spoolman pulls retain
stable filament/spool IDs and referenced missing/archived records. AI tagging supports
configurable compatible, Anthropic and Gemini providers, session-only credentials,
explicit batch review, and append/replace/category-aware merge. Category sharing defaults
to Local; sharing categories with the configured provider is an explicit setting used
when generating suggestions. Saving settings makes no provider call, and suggestions
never apply automatically. Provider fees are separate from GoggleLab entitlement.

The original Elegoo Centauri Carbon can map reported idle, printing, paused and complete
states during an explicit read-only connection check; unsupported values stay Unknown.
This is additional Community functionality beyond the pinned Printventory baseline.
Real-device job-state validation remains pending after the latest discovery timeout.
Bambu MQTT implementation and real certificate/identity validation remain deferred in
[issue #3](https://github.com/cubiclejuice/gogglelab/issues/3). Windows acceptance is tracked
in [issue #2](https://github.com/cubiclejuice/gogglelab/issues/2). No full parity or complete
cross-platform release acceptance is claimed yet. Colorways and palette access remain Pro.

# Unreleased: in-place catalog and thumbnails

Free now supports multiple indexed folders, startup/periodic scans, cancellation, size
limits, persistent geometry thumbnails, source/status filters, and name/size/date sorting.
Indexing never moves or writes source files. Missing files retain catalog metadata; a unique
rename retains its identity. Schema-v1 catalogs are backed up consistently before the
additive schema-v2 migration. Recovery exports retain managed assets and indexed references.
Source folder backups remain necessary for indexed originals.

# Unreleased: Free library and slicer handoff

Basic managed Library import/search/browsing and original STL/3MF slicer handoff are
Community features and work without Pro activation in the unified app. Existing official-app
Library storage, model IDs and schema remain unchanged. Colorways and palette access remain Pro.
This is the first [Free parity milestone](FREE-PRINTVENTORY-PARITY-PLAN.md), not full Printventory parity.

# GoggleLab 0.1.1

GoggleLab now ships through one public download channel. The official app is built from the
private product repository with the public MPL-2.0 core pinned at an exact commit. It starts
in Free mode and unlocks the licensed Pro features after a valid purchase email and signed
schema-2 license key are entered. The app keeps the established
`com.sonicparke.stl-handoff` identifier and existing GoggleLab Pro data paths.

The earlier Community preview remains a separate installation with separate data under
`com.cubiclejuice.gogglelab.community`. This release does not move, merge, or delete that
data. Existing 0.1.0 Community and Pro release downloads are preserved.

## Downloads

- macOS: choose the Apple Silicon (`arm64`) or Intel (`x64`) ZIP, unzip it, and copy
  GoggleLab to Applications.
- Windows: choose the x64 NSIS setup executable or MSI installer. Windows 10/11 and
  WebView2 are required.
- Linux: choose the x64 Debian package or AppImage. Ubuntu 22.04/glibc 2.35 or newer and
  WebKitGTK 4.1 are required; AppImages may need FUSE 2.

Each release candidate must contain all six native downloads from four successful native
runner jobs. `SHA256SUMS.txt`, the per-runner manifests, and `SOURCE-PROVENANCE.json`
record the exact private build commit and public core commit. A missing runner, changed
asset, mismatched pin, or incomplete production trust policy blocks publication.

Apple builds are currently ad-hoc signed and not notarized. Windows installers are
currently unsigned. A draft may be used for private verification, but these limitations
must remain visible if the release is published as a preview.

## Upgrading from the 0.1.0 Pro preview

- Windows: keep the same installer family when possible: MSI to MSI or NSIS setup to NSIS
  setup. Release-candidate checks exercise the legacy MSI upgrade identity and the legacy
  NSIS uninstall registry identity. The old-MSI-to-new-NSIS display-name match is verified
  against the pinned installer template, but not installed unattended because the upstream
  migration opens the MSI uninstaller UI. Old NSIS to new MSI is not a supported migration
  path.
- Linux Debian package: installing the new `goggle-lab` package replaces the old
  `goggle-lab-pro` package while retaining the `gogglelab-pro` executable name.
- macOS ZIP: quit GoggleLab Pro, keep the existing `GoggleLab Pro.app` as a temporary backup,
  and copy the new `GoggleLab.app` into Applications manually. Open the new app and verify
  its Library and activation data before removing the old app. Both builds use the same
  application identifier, so do not run them at the same time or delete application data
  during the switch.

## Licensing

Activation is offline after installation. Enter the purchase email exactly for the part
before `@`; GoggleLab lowercases only the email domain. The signed license contains the
canonical purchase email in readable form. A wrong email, malformed key, invalid signature,
unsupported schema, or unknown trust anchor does not enable Pro and does not replace a
working activation.

Production release candidates require a trusted verification-key configuration with at
least one key allowed to verify schema-2 email-bound licenses. A trusted key without an
explicit per-key schema override defaults to schema 2. Artifact-only untrusted preview
builds stay marked as non-publishable. Development issuer keys and development entitlement
controls are excluded from the official installer.

## Platform limitations

- Autodesk Fusion/F3D bridge support is macOS-only. Windows and Linux users can export
  STEP in Fusion.
- Linux nested drag-and-drop moves and ZIP extraction remain disabled. Use the system file
  manager or archive utility for those operations.
- Slicer discovery uses supported standard executable locations. Linux AppImages use the
  documented BambuStudio/Bambu_Studio or ElegooSlicer/ELEGOOSlicer filenames under
  `~/Applications`; Flatpak slicer launch is not supported.
- STEP preview requires a WebView with WebAssembly SIMD, tail calls, and exceptions. The
  desktop startup/STL smoke test does not establish full STEP compatibility on every distro.

## Public core, notices, and CAD source

The release attaches the exact public Community core source, the complete pinned
OCCT/occt-wasm/RapidJSON source kit, the MPL-2.0 license, and third-party notices. The
LGPL CAD engine remains separately replaceable. Place a compatible rebuilt
`occt-wasm.wasm` at the application data directory's `cad/occt-wasm.wasm`, then restart.
The override takes precedence without changing the application executable or requiring a
Pro license.

Application data roots for the official app:

- macOS: `~/Library/Application Support/com.sonicparke.stl-handoff/`
- Windows: `%APPDATA%/com.sonicparke.stl-handoff/`
- Linux: `~/.local/share/com.sonicparke.stl-handoff/` or the equivalent under
  `$XDG_DATA_HOME`

## Free library organization (phase 3, local validation)

The Community and unified Free app include categorized tags, designer/license/source
metadata, explicit batch edits, print history with quantities and filament references,
parent/child links, collections and ZIP/folder bundles. ZIP indexing is opt-in per root;
archives remain in place. Schema 3 migrations preserve a prior-schema snapshot and
read-only recovery preserves those schemas. Multipart preview/handoff and backup restore
were subsequently implemented in phase 4. These changes are not a published release.

## Free library management (phase 4, local validation)

Adds duplicate visual review within current filters, recoverable removal/restore,
recoverable holds for selected original sources, source reveal, full Library backups, staged restore/purge,
cache maintenance, directory/parent/license filters and sidebar resizing. Bundle preview
shows up to 32 parts; handoff sends all admitted members using verified stable copies.
Custom PNG/JPEG and embedded 3MF preview images are supported. These local changes are
not a published release; native window capture and cross-platform runtime gates remain.
