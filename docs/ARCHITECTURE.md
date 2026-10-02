# Community architecture

The public repository builds independently, with no private source, credentials or optional Pro dependency.
`src/community.ts` supplies authored-color viewing to the shared `startApp` host.
`src-tauri` registers inspection, browsing, recovery and read-only slicer-discovery commands.
The Rust workspace also contains model parsing/inspection and read-only Library recovery crates.

Pro consumes exported frontend contracts and the composable native builder from a pinned `core/` submodule.
Private source owns Library mutations, custom colorways, palette access and slicer launching.
The core defines extension contracts, but never registers a slicer-launch command.
The generation-checked original-source accessor preserves canonical source paths.

Selected-root filesystem operations validate root identity and containment. File moves reject
collisions, symlinks, moving roots or moving folders into themselves. Recovery preserves originals,
Library identities and raw saved colorway records.
