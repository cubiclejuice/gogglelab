import { basename, canonicalPage, stableId, supportedPage } from "./shared-module.js";
import { writeInbox } from "./inbox.js";
import { createCaptureStore, queueableDownloadName, trustedControlSender } from "./capture-state.js";

const captureStore = createCaptureStore(chrome.storage.local, writeInbox);

function controlSender(sender) {
  return trustedControlSender(sender, chrome.runtime.id, chrome.runtime.getURL(""));
}

function cleanText(value, max) {
  if (value == null) return null;
  if (typeof value !== "string") throw new Error("Capture metadata has an invalid field type");
  const text = value.trim();
  return text ? text.slice(0, max) : null;
}

function sanitizePage(page, expectedUrl) {
  if (!page || typeof page !== "object" || !supportedPage(page.url)) {
    throw new Error("Capture metadata is not a supported model page");
  }
  const url = canonicalPage(page.url);
  if (expectedUrl && url !== canonicalPage(expectedUrl)) {
    throw new Error("Capture metadata does not match its sender tab");
  }
  const title = cleanText(page.title, 512);
  if (!title) throw new Error("Capture metadata has no title");
  return {
    url,
    title,
    designer: cleanText(page.designer, 256),
    license: cleanText(page.license, 256),
    notes: cleanText(page.notes, 4000),
    parentExternalId: cleanText(page.parentExternalId, 256)
  };
}

async function recordFailure(downloadId, error) {
  await captureStore.recordFailure(downloadId, error);
}

async function enqueue(envelope) {
  return captureStore.enqueue(envelope);
}

async function envelope(page, downloads = []) {
  const identity = `${page.url}\0${downloads.map((download) => download.captureId).sort().join("\0") || "page"}`;
  return { version: 1, itemId: await stableId(identity), createdAt: new Date().toISOString(), page, downloads };
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender?.id !== chrome.runtime.id) return;
  if (message?.type === "page.metadata" && sender.tab?.id != null) {
    if (!message.page) {
      return;
    }
    try {
      const page = sanitizePage(message.page, sender.url || sender.tab.url);
      void captureStore.savePage(page).catch((error) => captureStore.recordFailure("page-metadata", error));
    } catch {
    }
    return;
  }
  if (message?.type === "capture.activePage" && controlSender(sender)) {
    void (async () => {
      const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
      if (!tab?.id || !supportedPage(tab.url)) throw new Error("Open a supported model page first");
      const page = sanitizePage(
        await chrome.tabs.sendMessage(tab.id, { type: "page.extract" }),
        tab.url
      );
      if (!page) throw new Error("This page is not a capturable model page");
      respond({ ok: true, envelope: await enqueue(await envelope(page)) });
    })().catch((error) => respond({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === "queue.list" && controlSender(sender)) {
    chrome.storage.local.get({ captureQueue: [], captureFailures: [] }).then(respond);
    return true;
  }
  if (message?.type === "queue.retry" && controlSender(sender)) {
    void (async () => {
      respond({ ok: true, result: await captureStore.retry(message.itemId) });
    })().catch((error) => respond({ ok: false, error: error.message }));
    return true;
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  chrome.storage.local.remove(`page:${tabId}`);
});

chrome.downloads.onCreated.addListener((item) => {
  void captureStore.rememberDownload(item).catch((error) => void recordFailure(item.id, error));
});

chrome.downloads.onChanged.addListener((delta) => {
  if (delta.state?.current !== "complete") return;
  void (async () => {
    const [item] = await chrome.downloads.search({ id: delta.id });
    if (!item || !queueableDownloadName(item.filename)) return;
    const attributed = await captureStore.downloadPage(item);
    if (!attributed) return;
    const page = sanitizePage(attributed, attributed.url);
    await enqueue(await envelope(page, [{
      captureId: `download-${item.id}`,
      fileName: basename(item.filename),
      relativeInboxPath: null,
      sha256: null,
      bytes: item.fileSize ?? item.totalBytes ?? null
    }]));
  })().catch((error) => void recordFailure(delta.id, error));
});
