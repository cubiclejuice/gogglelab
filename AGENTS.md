# Agent instructions

## Project workflow

- Use the model-router skill.
- Use sub-agents whenever parallelization is likely to improve speed, coverage, or quality.
- Use Build New Thing whenever applicable. For coding tasks, use the build-new-thing skill by default.
- Before reading source files or scanning the codebase, use Tokensave MCP tools (`tokensave_context`, `tokensave_search`, `tokensave_callers`, `tokensave_callees`, `tokensave_impact`, `tokensave_node`, `tokensave_files`, `tokensave_affected`). If those tools cannot answer the code-analysis question, query `.tokensave/tokensave.db` directly. If a Tokensave gap is found, propose an issue at <https://github.com/aovestdipaperino/tokensave> and remind the user to remove sensitive or proprietary code from the report.
- Always use `mcp__semble__search` before Grep, Glob, or Read to explore the codebase. Use `--content docs` for documentation, `--content config` for configuration, and `--content all` for all content. Use exact-path reads or exhaustive literal searches only when needed; use `mcp__semble__find_related` for related implementations.

## Build storage and cleanup policy

- Keep GoggleLab build storage bounded. After successful builds, inspect generated artifacts and duplicate build directories; do not automatically clear all caches after each build.
- Preserve the current working build, the latest known-good release, source archives needed for testing, and license/audit artifacts. Prefer one stable `CARGO_TARGET_DIR` for repeated work instead of creating duplicate targets.
- Before any Cargo cleanup, run `cargo clean --dry-run` with the exact target directory, profile, and package scope being considered. Read its full summary; if it includes the active build, a protected release, or anything outside the approved scope, do not execute cleanup.
- Cargo may count package-generated dependency files in its dry-run summary without printing their individual paths. Inspect that exact target’s package-named `debug/deps` artifacts and hard-link aliases alongside the listed fingerprint/incremental outputs. Links covered by the same approved package/profile command are generated cleanup scope; links to source, user data, another target, or a protected release require separate review and must never broaden approval.
- Before proposing removal, verify each exact path is reproducible build output. Never delete source, Git history, uncommitted work, configuration, credentials, user data, backups, Docker data, release/runtime assets, or application data.
- Prefer a build tool's narrow supported cleanup command. Never remove an entire Rust `target` tree; only consider identified inactive debug intermediates such as `target/debug/deps`, `build`, `incremental`, `.fingerprint`, or `examples`. Preserve release output and any active cache.
- Never clean files while a build, package tool, updater, MCP operation, or app may be using them. Check active processes and open files immediately before cleanup. If process/open-file inspection is denied, do not infer inactivity.
- Ask the user for explicit approval naming the exact paths and cleanup scope before any permanent deletion. Approval for one target does not authorize neighboring or newly discovered targets.
- Measure `df -h /System/Volumes/Data` before cleanup and after each cleanup phase. Use `du` only to estimate candidates; report the measured free-space delta. Re-inventory candidates after every build rather than reusing old sizes.
