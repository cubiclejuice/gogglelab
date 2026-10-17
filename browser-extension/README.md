# GoggleLab Capture extension

This Manifest V3 source extension captures supported model-page metadata and completed download records into a bounded local queue. It does not connect to GoggleLab or receive localhost access. Catalog API enrichment is optional, off by default, and never runs until the user enables it and clicks Enrich.

## Install for development

1. Open `chrome://extensions` in Chrome or `edge://extensions` in Edge.
2. Enable **Developer mode**.
3. Choose **Load unpacked** and select this `browser-extension` directory.
4. Open a supported model page and use the GoggleLab Capture toolbar button.

Do not point development checks at a personal browser profile. Use a disposable profile with synthetic pages and downloads. This source artifact has not been published to an extension store.

## Supported sites

- MakerWorld
- Printables
- Thangs
- Thingiverse
- Cults3D
- MyMiniFactory

Only HTTPS model-page routes on the six exact catalog roots or their subdomains are accepted. Host matching requires the root or a dot-boundary suffix, so lookalikes such as `makerworld.com.attacker.test` are rejected. Supported routes include localized MakerWorld `/models/{id}`, Printables `/prints/{id}` and `/model/{id}`, Thingiverse `/thing:{id}` and `/thing/{id}`, Thangs `/model|design|3d-model/{slug}` plus designer model pages, Cults `/3d-model|design/{slug}`, and MyMiniFactory `/object/{slug}`. Credentials and non-default ports are rejected. Tracking query parameters and fragments are removed from captured source URLs. The extension does not fetch a queued page later.

The popup first loads metadata locally from the open page, then lets the user edit the title, designer, license, source URL, and notes before queueing. The edited source URL must still be a supported HTTPS model page. Local extraction makes no catalog API requests. If the user enables enrichment in Inbox options, the popup shows a separate Enrich button; results fill the editable fields and still require review before queueing. Queue import remains a separate explicit action in GoggleLab.

Page extraction uses bounded JSON-LD and `__NEXT_DATA__`, Open Graph/Twitter metadata, and local DOM/slug fallbacks. The service worker keeps one recent page snapshot per tab, capped at 100 tabs and 30 minutes, and clears it on navigation or tab close. When a completed download points to a CDN, its `tabId` can attach the queued record to that recent source-page snapshot.

## Permissions

- `activeTab`: lets the popup request a fresh extraction when the user explicitly captures the current page. The declared content script also performs passive, local metadata extraction on supported pages so a later completed download can retain its source context.
- `storage`: keeps the bounded queue and folder preference across service-worker restarts.
- `downloads`: associates completed catalog-site downloads and STL/OBJ/3MF files from other safe HTTPS sources with their referrer when available.
- `offscreen`: writes an inbox file after an explicit folder choice when the service worker has no document context.
- Site host permissions: run the data-only extractor on the six supported catalog roots and their subdomains. There is no localhost or all-sites permission.
- Optional API hosts: only `api.printables.com` and `api.bambulab.com`, requested after the user enables enrichment. No sign-in cookies are sent, and API responses are bounded.

## Inbox and fallback

The options page can request a user-selected inbox folder. Permission can be revoked by clearing that selection. When folder access is unavailable or revoked, export falls back to `Downloads/GoggleLabInbox`. Each immutable version-1 JSON envelope contains a stable item ID, page metadata, and bounded download records. The extension captures completed downloads from supported catalog hosts and STL/OBJ/3MF downloads from other safe HTTPS sources. Unattributed model-file captures use the safe referrer URL (or the download URL when needed) and the filename as a minimal title; query strings, fragments, and local paths are not included. Unsafe or ambiguous source URLs are skipped, as are non-model downloads from other sites. Model bytes stay in the browser download location until the user explicitly selects them for import.

GoggleLab imports a queue file explicitly. Matching model files are admitted as uploads; a page without a file becomes a bookmark and never fabricates model bytes. Replaying the same item ID and payload is idempotent. Reusing an item ID with changed content is a conflict.

## Limits and retries

- At most 500 queue items.
- Each stored envelope is at most 64 KiB.
- Every download from a supported catalog host is queued as metadata; STL, 3MF, and OBJ downloads from other safe HTTPS sources are also queued. The browser importer currently offers STL and 3MF files. Other download types and OBJ remain metadata-only until explicit importer support is available; the extension never fabricates model bytes.
- Stable item and capture IDs make service-worker retries idempotent.
- Failed downloads remain visible and retryable; successful records are immutable.
- Queue-full, malformed, unsupported, and conflicting captures fail visibly instead of evicting an unrelated item.

## Verification evidence

Repository tests exercise exact HTTPS host/path admission, canonical URL handling, deterministic identities, serialized concurrent queue writes, capacity/conflict/replay/retry behavior, persisted referrer attribution across navigation and worker restart, control-message sender admission, and folder/offscreen fallback with mocked browser APIs. JavaScript syntax and the MV3 manifest are checked locally. No extension was installed in a user Chrome or Edge profile, and no authenticated catalog API or live GoggleLab server was contacted by those tests.

Run the source checks from the repository root:

```sh
bun test src/lib/capture-extension.test.js
node --check browser-extension/background.js
node --check browser-extension/content.js
node --check browser-extension/inbox.js
node --check browser-extension/offscreen.js
node --check browser-extension/options.js
node --check browser-extension/popup.js
```
