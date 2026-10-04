# GoggleLab 0.1.0 previews

Initial multi-platform preview builds for macOS Apple Silicon and Intel, Windows x64,
and Linux x64. Community is MPL-2.0; Pro source and downloads remain in the private
repository. The Pro preview includes the private implementation but has activation
disabled when no trusted verification key is configured. It starts in Free mode,
retains recovery access, and never uses unsigned development entitlements in release builds.

## Downloads

- macOS: unzip the architecture-matching app bundle and copy it to Applications.
- Windows: choose the NSIS setup executable or MSI installer. Windows 10/11 and WebView2 are required.
- Linux: Debian package or AppImage; Ubuntu 22.04/glibc 2.35 or newer. WebKitGTK 4.1 is required; AppImages may need FUSE 2.

Apple builds are ad-hoc signed and not notarized. Windows installers are unsigned.
These are preview builds, not a signed commercial release. SHA256SUMS.txt records every asset.

## Platform limitations

- Autodesk Fusion/F3D bridge is macOS-only; Windows/Linux users can export STEP in Fusion.
- Linux nested drag/drop moves and ZIP extraction are disabled to preserve selected-root
  safety. Use the system file manager/archive utility for those operations. Viewing/search
  still works in nested folders.
- Slicer discovery uses supported standard executable locations; Linux AppImages use the
  documented BambuStudio/Bambu_Studio or ElegooSlicer/ELEGOOSlicer filenames under ~/Applications.
  Flatpak slicer launch is not supported in this preview.
- STEP preview requires a WebView with WebAssembly SIMD, tail calls and exceptions.
  The desktop startup/STL smoke check does not establish full STEP compatibility on every distro.

Community uses its own application identity and storage, allowing installation alongside
Pro. Pro retains the established application identifier and existing Library/license data.
For old colorway recovery, use Pro's free recovery tool in the original WebView identity.

## CAD component and source

Complete license texts and dependency notices are bundled under resources/licenses.
Matching Community source and the complete pinned OCCT/occt-wasm/RapidJSON source kit
are attached to each release.

The LGPL CAD engine is separately loaded. Place a compatible modified `occt-wasm.wasm`
at the application data directory's `cad/occt-wasm.wasm`, then restart. The override
takes precedence without modifying or resigning the app and without any entitlement.
Only the WASM header/size limits are checked, not a vendor signature or approved digest.

Application data roots:

- macOS: ~/Library/Application Support/<application identifier>/
- Windows: %APPDATA%/<application identifier>/
- Linux: ~/.local/share/<application identifier>/ (or XDG_DATA_HOME)

Community identifier: com.cubiclejuice.gogglelab.community.
Pro identifier: com.sonicparke.stl-handoff.
The source kit includes exact revisions, patches, build instructions and upstream licenses.
Third-party license rights, modification/replacement and debugging rights remain intact.
