# Contributing to GoggleLab Community

This repository contains the public Community app and shared core. The official
Pro app is developed in a separate private workspace that pins a public core
commit; do not add Pro implementation, private manifests, issuer keys, or
release credentials here.

## Local development

Requirements: macOS, Bun 1.3.14, stable Rust, and Xcode Command Line Tools.

```sh
bun run setup
bun run dev
```

`bun run dev` uses a development app identity and data directory. Use
`bun run dev:web` for the Vite frontend alone. Finder file-open behavior requires
a built app.

## Checks and builds

```sh
bun run check
bun run build
```

`bun run check` covers frontend formatting/lint/types/tests, the public Cargo
workspace, and the Community web build. `bun run build` packages the desktop
app and applies the STL Finder UTI patch. `bun run status` shows the current
branch and local changes.

Keep edits to the public extension contract small and supported. Core changes
needed by Pro must land in this repository first; the private workspace then
updates its `core/` submodule to the reviewed public commit. Community must keep
building without private code or credentials.
