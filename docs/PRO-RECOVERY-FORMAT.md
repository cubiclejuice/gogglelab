# Library and saved colorway recovery format, version 1

This is the public recovery contract for existing GoggleLab Library data. The recovery reader is independent of the writable Library implementation. It reads `app_data_dir/library/library.sqlite3` with SQLite's read-only flag, in a read transaction, and never initializes a missing database or runs a migration. It accepts `PRAGMA user_version = 1` only. An unknown or incomplete schema is an error; the reader leaves the source intact.

The v1 reader requires the existing `libraries`, `folders`, `blobs`, `models`, `revisions`, `tags`, `model_tags`, `manuals`, `thumbnails`, and `jobs` tables and their v1 columns. `folders` preserve `id`, `name`, and `parent_id`. Each model preserves its ID, `library:<model_id>` source identity, display name, description, folder ID, current revision ID, metadata version, deletion time, and creation/update times. Every revision is joined by `revisions.model_id` and `revisions.blob_hash = blobs.hash`; it preserves revision ID, model ID, blob hash, original name, format, creation time, byte count, and the managed blob path. Tag IDs/names and model-tag links remain explicit. Referenced manuals and thumbnails retain their paths and hashes; thumbnails also retain their IDs and `is_custom` state. Unreferenced staging, trash, jobs, and blobs are not exported.

## ZIP layout

An export is a new ZIP file, with `manifest.json` and `files/000000.bin`, `files/000001.bin`, and so on. Entries are ordered by model ID, then revision ID, then manual and thumbnail ID. Entry timestamps and permissions are fixed. Files use ZIP's stored method so their bytes match the managed source exactly. `manifest.json` is last because its paths and verified sizes are known after streaming the files. The exporter never replaces an existing destination and removes an incomplete ZIP if validation or writing fails.

The manifest has `manifest_version: 1` and `library_schema_version: 1`, plus `library_id`, `library_name`, `library_created_at`, `folders`, `tags`, `model_tags`, and `models`. A model contains `revisions`, an optional `manual`, and `thumbnails`. Each asset record has:

| Field | Meaning |
| --- | --- |
| `original_relative_path` | Path stored by the Library, relative to its root |
| `exported_path` | ZIP entry path; `null` in a catalog inspection response |
| `bytes` | Exact exported byte count |
| `sha256` | Lowercase SHA-256 of the exported bytes |

An original's `sha256` must match its blob hash; manuals and thumbnails must match their `content_hash` column. A revision also contains `blob_hash`, making its original relationship explicit. An export of selected models includes their ancestor folders and linked tags. An empty model selection exports all models. The generated test fixture in `crates/library-recovery/tests/recovery.rs` exercises a deleted model, folder, tag, original, manual, and thumbnail and verifies the archive bytes and manifest links.

Managed paths must use forward slashes, start with their designated `originals/`, `manuals/`, or `thumbnails/` directory, contain no empty, `.` or `..` components, and remain within the canonical Library root. Symlinked managed path components are rejected. The destination must be an absolute path whose parent resolves outside the Library root. The exporter limits each asset to 512 MiB, total assets to 4 GiB, each catalog query to 50,000 rows, models to 10,000, catalog metadata and manifest JSON to 16 MiB each, and raw colorway input to 8 MiB. These are recovery safety bounds, not Library storage limits.

## Saved colorways

Saved colorways live under the webview localStorage key `gogglelab.colorways.v1`. The Free recovery interface reads that raw value in the same app/webview identity and passes it to the native `recovery_export_colorways` command. The command writes the received UTF-8 string bytes to a new user-chosen file, with an 8 MiB limit. It neither parses nor normalizes the string, so malformed JSON remains exportable. It never calls `setItem` or otherwise changes localStorage. A missing key has no raw value to export.

Future Pro schema changes must retain v1 recovery compatibility or ship a public, compatible reader adapter in the same release. Older readers reject unknown versions without writing to the source.
