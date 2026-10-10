# Community architecture

The public repository builds independently, with no private source, credentials or optional Pro dependency.
`src/community.ts` supplies authored-color viewing to the shared `startApp` host.
`src-tauri` registers inspection, browsing, managed Library, recovery, slicer-discovery and generation-checked slicer-handoff commands.
The Rust workspace also contains model parsing/inspection, managed Library storage and read-only Library recovery crates.

Pro consumes exported frontend contracts and the composable native builder from a pinned `core/` submodule.
Private source owns custom colorways, palette access and licensing.
The core registers Library and slicer launch independently of Pro entitlements.
Library uses the existing app-data library directory and schema-v1 database; initialization is lazy.
The Pro plugin must not register duplicate Library state or commands.
The generation-checked original-source accessor preserves canonical source paths.

Selected-root filesystem operations validate root identity and containment. File moves reject
collisions, symlinks, moving roots or moving folders into themselves. Recovery preserves originals,
Library identities and raw saved colorway records.

The selected Library and handoff implementations moved from private Pro to public MPL-2.0
with explicit owner approval on 2026-10-10. See [Free parity plan](FREE-PRINTVENTORY-PARITY-PLAN.md).
