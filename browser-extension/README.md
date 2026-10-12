# GoggleLab Capture extension

This Manifest V3 source extension captures supported model-page metadata and completed STL/3MF downloads into a bounded local queue. It does not connect to GoggleLab, call catalog-site APIs, or receive localhost access.

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

Only HTTPS model-page routes on the exact listed hosts are accepted. Tracking query parameters and fragments are removed from the captured page identity. The extension does not fetch a queued page later.

## Permissions

- `activeTab`: lets the popup request a fresh extraction when the user explicitly captures the current page. The declared content script also performs passive, local metadata extraction on supported pages so a later completed download can retain its source context.
- `storage`: keeps the bounded queue and folder preference across service-worker restarts.
- `downloads`: associates completed STL/3MF downloads with the page captured in that tab.
- `offscreen`: writes an inbox file after an explicit folder choice when the service worker has no document context.
- Site host permissions: run the data-only extractor on the six supported catalog families. There is no localhost or arbitrary-host permission.

## Inbox and fallback

The options page can request a user-selected inbox folder. Permission can be revoked by clearing that selection. When folder access is unavailable or revoked, export falls back to `Downloads/GoggleLabInbox`. Each immutable version-1 JSON envelope contains a stable item ID, page metadata, and bounded download records. Model bytes stay in the browser download location until the user explicitly selects them for import.

GoggleLab imports a queue file explicitly. Matching model files are admitted as uploads; a page without a file becomes a bookmark and never fabricates model bytes. Replaying the same item ID and payload is idempotent. Reusing an item ID with changed content is a conflict.

## Limits and retries

- At most 500 queue items.
- Each stored envelope is at most 64 KiB.
- STL, 3MF, OBJ, and ZIP downloads from an attributed supported model page are queued. OBJ and ZIP remain metadata-only attachments until a supported explicit import is available; the extension never fabricates model bytes or enables an OBJ parser.
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
