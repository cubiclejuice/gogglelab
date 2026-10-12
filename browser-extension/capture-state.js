import { basename, canonicalPage, supportedPage } from "./shared-module.js";

export const MAX_QUEUE = 500;
export const MAX_PAYLOAD_BYTES = 64 * 1024;
const MAX_PAGE_SNAPSHOTS = 200;
const MAX_DOWNLOAD_ATTRIBUTIONS = 500;

export function trustedControlSender(sender, runtimeId, extensionRoot) {
  return sender?.id === runtimeId && !sender.tab &&
    typeof sender.url === "string" && sender.url.startsWith(extensionRoot);
}

export function queueableDownloadName(value) {
  return typeof value === "string" && /\.(stl|3mf|obj|zip)$/i.test(value);
}

export function downloadPageUrl(item) {
  const urls = [item?.referrer, item?.url, item?.finalUrl]
    .filter(supportedPage)
    .map(canonicalPage);
  const unique = [...new Set(urls)];
  if (unique.length > 1) throw new Error("Download attribution is ambiguous across model pages");
  return unique[0] ?? null;
}

export function createCaptureStore(storage, writeInbox, now = () => new Date().toISOString()) {
  let tail = Promise.resolve();
  const serialize = (operation) => {
    const run = tail.then(operation, operation);
    tail = run.catch(() => {});
    return run;
  };

  const savePage = (page) => serialize(async () => {
    const { capturePages = [] } = await storage.get({ capturePages: [] });
    const url = canonicalPage(page.url);
    const next = [...capturePages.filter((entry) => entry.url !== url), { ...page, url, capturedAt: now() }]
      .slice(-MAX_PAGE_SNAPSHOTS);
    await storage.set({ capturePages: next });
  });

  const rememberDownload = (item) => serialize(async () => {
    const url = downloadPageUrl(item);
    if (!url) return null;
    const { capturePages = [], captureDownloads = [] } = await storage.get({ capturePages: [], captureDownloads: [] });
    const page = [...capturePages].reverse().find((entry) => entry.url === url) ?? null;
    const record = { downloadId: item.id, url, page, recordedAt: now() };
    await storage.set({ captureDownloads: [...captureDownloads.filter((entry) => entry.downloadId !== item.id), record].slice(-MAX_DOWNLOAD_ATTRIBUTIONS) });
    return record;
  });

  const downloadPage = (item) => serialize(async () => {
    const { captureDownloads = [], capturePages = [] } = await storage.get({ captureDownloads: [], capturePages: [] });
    let record = captureDownloads.find((entry) => entry.downloadId === item.id) ?? null;
    if (!record) {
      const url = downloadPageUrl(item);
      if (!url) return null;
      record = { downloadId: item.id, url, page: [...capturePages].reverse().find((entry) => entry.url === url) ?? null, recordedAt: now() };
    }
    if (record.page) return record.page;
    return {
      url: record.url,
      title: basename(item.filename),
      designer: null,
      license: null,
      notes: null,
      parentExternalId: null,
    };
  });

  const enqueue = (envelope) => serialize(async () => {
    const encoded = new TextEncoder().encode(JSON.stringify(envelope));
    if (encoded.byteLength > MAX_PAYLOAD_BYTES) throw new Error("Capture metadata exceeds 64 KiB");
    const { captureQueue = [] } = await storage.get({ captureQueue: [] });
    const existing = captureQueue.find((item) => item.itemId === envelope.itemId);
    if (existing) {
      envelope.createdAt = existing.createdAt;
      if (JSON.stringify(existing) !== JSON.stringify(envelope)) throw new Error("This capture identity already exists with different metadata");
      return existing;
    }
    if (captureQueue.length >= MAX_QUEUE) throw new Error("Capture queue is full; import or clear completed items before adding more");
    await storage.set({ captureQueue: [...captureQueue, envelope] });
    await writeInbox(envelope);
    return envelope;
  });

  const recordFailure = (downloadId, error) => serialize(async () => {
    const { captureFailures = [] } = await storage.get({ captureFailures: [] });
    await storage.set({ captureFailures: [...captureFailures, { downloadId, failedAt: now(), error: String(error?.message || error).slice(0, 512) }].slice(-100) });
  });

  const retry = (itemId) => serialize(async () => {
    const { captureQueue = [] } = await storage.get({ captureQueue: [] });
    const item = captureQueue.find((entry) => entry.itemId === itemId);
    if (!item) throw new Error("Capture is no longer queued");
    return writeInbox(item);
  });

  return { savePage, rememberDownload, downloadPage, enqueue, recordFailure, retry };
}
