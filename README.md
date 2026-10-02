# GoggleLab Community

A macOS desktop viewer for checking 3D models before printing. Built with Tauri, Rust, TypeScript and Three.js.

## Community features

- STL and 3MF viewing, dimensions, volume where supported, mesh integrity and printer bed fit.
- STEP/STP tessellated previews; F3D viewing through the optional local Autodesk Fusion bridge.
- Multi-plate selection and overview, embedded/authored 3MF colors, and appearance controls.
- Folder browsing, search, watching, ZIP extraction, drag/drop moves and confirmed Trash actions.
- Installed slicer discovery and printer-profile detection for inspection.
- Read-only recovery of existing Library data and raw saved colorway records.

Community does not contain managed Library, editable/saved colorway or slicer-launch implementations.
The official app composes this core with private Pro source and unlocks paid functionality through signed entitlements.
Custom preview colors are not exported into models. STEP/F3D previews do not add slicer handoff.

## Build and develop

Requires macOS 13+, stable Rust, Bun 1.3.14 and Xcode Command Line Tools.
STEP preview requires a WebKit version supporting the bundled CAD engine (Safari 17.2+).

```sh
bun run setup
bun run check
bun run build
```

The app is generated at `target/release/bundle/macos/GoggleLab.app`.
`bun run dev` uses isolated Community development data; `bun run dev:web` runs only Vite.
`bun run build:web` builds only the frontend. Desktop bundles are locally signed, not notarized releases.

Finder file associations apply to built registered app bundles. Production editions retain
`com.sonicparke.stl-handoff` and existing storage formats for compatibility.
See [CONTRIBUTING.md](CONTRIBUTING.md), [architecture](docs/ARCHITECTURE.md),
[Fusion previews](docs/F3D-PREVIEW.md), and [recovery](docs/PRO-RECOVERY-FORMAT.md).

## License

Community source is [MPL-2.0](LICENSE). Dependencies retain their own licenses;
see [third-party notices](THIRD-PARTY-NOTICES.md). Pro source lives separately in
the private `cubiclejuice/gogglelab-pro` repository. Commercial terms, production
key custody and approved binary releases are managed separately.

## Limitations

macOS only; no mesh repair, slicing, supports, CAD editing, OBJ or G-code rendering.
CAD previews preserve the original source and derive measurements from tessellated geometry.
The Fusion bridge requires an installed, running Autodesk Fusion with its local add-in enabled.
ZIP extraction rejects unsafe paths, encrypted/unsupported entries and collisions.
Slicer discovery checks `/Applications` and `~/Applications`.
