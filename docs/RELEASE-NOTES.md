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
