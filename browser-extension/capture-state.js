import {
  basename,
  canonicalPage,
  safeCaptureSourceUrl,
  supportedCatalogUrl,
  supportedPage,
} from "./shared-module.js";

export const MAX_QUEUE = 500;
export const MAX_PAYLOAD_BYTES = 64 * 1024;
const MAX_PAGE_SNAPSHOTS = 200;
const MAX_TAB_SNAPSHOTS = 100;
const MAX_TAB_SNAPSHOT_AGE_MS = 30 * 60 * 1000;
const MAX_DOWNLOAD_ATTRIBUTIONS = 500;

export function trustedControlSender(sender, runtimeId, extensionRoot) {
  return (
    sender?.id === runtimeId &&
    !sender.tab &&
    typeof sender.url === "string" &&
    sender.url.startsWith(extensionRoot)
  );
}

export function queueableDownloadName(value) {
  return typeof value === "string" && /\.(stl|3mf|obj|zip)$/i.test(value);
}

function hasCatalogSource(item) {
  return [item?.referrer, item?.url, item?.finalUrl].some(supportedCatalogUrl);
}

function safeDownloadName(value) {
  if (typeof value !== "string" || !value.trim()) return null;
  const name = basename(value);
  if (
    !name ||
    name === "." ||
    name === ".." ||
    /[\u0000-\u001f\u007f]/.test(name) ||
    new TextEncoder().encode(name).byteLength > 255
  ) {
    return null;
  }
  return name;
}

function modelFileName(value) {
  const name = safeDownloadName(value);
  return !!name && /\.(stl|obj|3mf)$/i.test(name);
}

function recentTabSnapshot(snapshots, tabId, currentTime) {
  if (!Number.isInteger(tabId) || tabId < 0) return null;
  const snapshot = [...snapshots].reverse().find((entry) => entry.tabId === tabId);
  if (!snapshot?.page || typeof snapshot.capturedAt !== "string") return null;
  const age = Date.parse(currentTime) - Date.parse(snapshot.capturedAt);
  return Number.isFinite(age) && age >= 0 && age <= MAX_TAB_SNAPSHOT_AGE_MS ? snapshot : null;
}

export function downloadPageUrl(item) {
  const urls = [item?.referrer, item?.url, item?.finalUrl].filter(supportedPage).map(canonicalPage);
  const unique = [...new Set(urls)];
  if (unique.length > 1) throw new Error("Download attribution is ambiguous across model pages");
  return unique[0] ?? null;
}

export function downloadSourceUrl(item) {
  const modelPage = downloadPageUrl(item);
  if (modelPage) return modelPage;
  const sources = [item?.referrer, item?.url, item?.finalUrl]
    .map(safeCaptureSourceUrl)
    .filter(Boolean);
  return sources.find(supportedCatalogUrl) ?? sources[0] ?? null;
}

export function shouldCaptureDownload(item) {
  if (!safeDownloadName(item?.filename)) return false;
  const sourceUrl = downloadSourceUrl(item);
  if (!sourceUrl) return false;
  return hasCatalogSource(item) || modelFileName(item.filename);
}

export function createCaptureStore(storage, writeInbox, now = () => new Date().toISOString()) {
  let tail = Promise.resolve();
  const serialize = (operation) => {
    const run = tail.then(operation, operation);
    tail = run.catch(() => {});
    return run;
  };

  const savePage = (page, tabId = null) =>
    serialize(async () => {
      const { capturePages = [], captureTabPages = [] } = await storage.get({
        capturePages: [],
        captureTabPages: [],
      });
      const url = canonicalPage(page.url);
      const capturedAt = now();
      const next = [
        ...capturePages.filter((entry) => entry.url !== url),
        { ...page, url, capturedAt },
      ].slice(-MAX_PAGE_SNAPSHOTS);
      const update = { capturePages: next };
      if (Number.isInteger(tabId) && tabId >= 0) {
        update.captureTabPages = [
          ...captureTabPages.filter((entry) => entry.tabId !== tabId),
          { tabId, page: { ...page, url }, capturedAt },
        ].slice(-MAX_TAB_SNAPSHOTS);
      }
      await storage.set(update);
    });

  const rememberDownload = (item) =>
    serialize(async () => {
      const {
        capturePages = [],
        captureDownloads = [],
        captureTabPages = [],
      } = await storage.get({
        capturePages: [],
        captureDownloads: [],
        captureTabPages: [],
      });
      const recordedAt = now();
      const snapshot = recentTabSnapshot(captureTabPages, item?.tabId, recordedAt);
      const modelPageUrl = downloadPageUrl(item);
      const directSourceUrl = modelPageUrl ?? downloadSourceUrl(item);
      const catalogSourceUrl = [directSourceUrl].find(supportedCatalogUrl);
      const url = modelPageUrl ?? catalogSourceUrl ?? snapshot?.page.url ?? directSourceUrl;
      if (!url || (!hasCatalogSource(item) && !modelFileName(item?.filename) && !snapshot))
        return null;
      const page =
        snapshot?.page.url === url
          ? snapshot.page
          : ([...capturePages].reverse().find((entry) => entry.url === url) ?? null);
      const record = { downloadId: item.id, tabId: item.tabId ?? null, url, page, recordedAt };
      await storage.set({
        captureDownloads: [
          ...captureDownloads.filter((entry) => entry.downloadId !== item.id),
          record,
        ].slice(-MAX_DOWNLOAD_ATTRIBUTIONS),
      });
      return record;
    });

  const downloadPage = (item) =>
    serialize(async () => {
      if (!safeDownloadName(item?.filename)) return null;
      const {
        captureDownloads = [],
        capturePages = [],
        captureTabPages = [],
      } = await storage.get({
        captureDownloads: [],
        capturePages: [],
        captureTabPages: [],
      });
      let record = captureDownloads.find((entry) => entry.downloadId === item.id) ?? null;
      const recordedAt = now();
      const snapshot = recentTabSnapshot(captureTabPages, item?.tabId, recordedAt);
      const modelPageUrl = downloadPageUrl(item);
      const directSourceUrl = modelPageUrl ?? downloadSourceUrl(item);
      const catalogSourceUrl = [directSourceUrl, record?.url].find(supportedCatalogUrl);
      const url =
        modelPageUrl ??
        catalogSourceUrl ??
        snapshot?.page.url ??
        directSourceUrl ??
        record?.url ??
        null;
      if (
        !url ||
        (!record && !hasCatalogSource(item) && !modelFileName(item.filename) && !snapshot)
      )
        return null;
      if (!record) {
        record = {
          downloadId: item.id,
          tabId: item.tabId ?? null,
          url,
          page:
            snapshot?.page.url === url
              ? snapshot.page
              : ([...capturePages].reverse().find((entry) => entry.url === url) ?? null),
          recordedAt,
        };
      }
      if (record.page) return record.page;
      const page =
        [...capturePages].reverse().find((entry) => entry.url === record.url) ??
        (snapshot?.page.url === url ? snapshot.page : null);
      if (page) return page;
      const sourceUrl = safeCaptureSourceUrl(url);
      if (!sourceUrl) return null;
      return {
        url: sourceUrl,
        title: basename(item.filename),
        designer: null,
        license: null,
        notes: null,
        parentExternalId: null,
      };
    });

  const removeTab = (tabId) =>
    serialize(async () => {
      const { captureTabPages = [] } = await storage.get({ captureTabPages: [] });
      await storage.set({
        captureTabPages: captureTabPages.filter((entry) => entry.tabId !== tabId),
      });
    });

  const enqueue = (envelope) =>
    serialize(async () => {
      const encoded = new TextEncoder().encode(JSON.stringify(envelope));
      if (encoded.byteLength > MAX_PAYLOAD_BYTES)
        throw new Error("Capture metadata exceeds 64 KiB");
      const { captureQueue = [] } = await storage.get({ captureQueue: [] });
      const existing = captureQueue.find((item) => item.itemId === envelope.itemId);
      if (existing) {
        envelope.createdAt = existing.createdAt;
        if (JSON.stringify(existing) !== JSON.stringify(envelope))
          throw new Error("This capture identity already exists with different metadata");
        return existing;
      }
      if (captureQueue.length >= MAX_QUEUE)
        throw new Error(
          "Capture queue is full; import or clear completed items before adding more",
        );
      await storage.set({ captureQueue: [...captureQueue, envelope] });
      await writeInbox(envelope);
      return envelope;
    });

  const recordFailure = (downloadId, error) =>
    serialize(async () => {
      const { captureFailures = [] } = await storage.get({ captureFailures: [] });
      await storage.set({
        captureFailures: [
          ...captureFailures,
          { downloadId, failedAt: now(), error: String(error?.message || error).slice(0, 512) },
        ].slice(-100),
      });
    });

  const retry = (itemId) =>
    serialize(async () => {
      const { captureQueue = [] } = await storage.get({ captureQueue: [] });
      const item = captureQueue.find((entry) => entry.itemId === itemId);
      if (!item) throw new Error("Capture is no longer queued");
      return writeInbox(item);
    });

  return { savePage, rememberDownload, downloadPage, removeTab, enqueue, recordFailure, retry };
}
