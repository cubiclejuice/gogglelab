import {
  basename,
  canonicalPage,
  reviewPage,
  safeCaptureSourceUrl,
  stableId,
  supportedPage,
} from "./shared-module.js";
import { writeInbox } from "./inbox.js";
import { createCatalogEnricher, createEnrichmentSettings } from "./api-enrichment.js";
import { createCaptureStore, trustedControlSender } from "./capture-state.js";

const captureStore = createCaptureStore(chrome.storage.local, writeInbox);
const enrichmentSettings = createEnrichmentSettings(chrome.storage.local, chrome.permissions);
const catalogEnricher = createCatalogEnricher({
  isEnabled: enrichmentSettings.isEnabled,
  hasPermission: enrichmentSettings.hasPermission,
});

function controlSender(sender) {
  return trustedControlSender(sender, chrome.runtime.id, chrome.runtime.getURL(""));
}

function cleanText(value, max) {
  if (value == null) return null;
  if (typeof value !== "string") throw new Error("Capture metadata has an invalid field type");
  const text = value.trim();
  return text ? text.slice(0, max) : null;
}

function sanitizePage(page, expectedUrl, allowSafeSourceUrl = false) {
  const safeSource = allowSafeSourceUrl ? safeCaptureSourceUrl(page?.url) : null;
  if (!page || typeof page !== "object" || (!supportedPage(page.url) && !safeSource)) {
    throw new Error("Capture metadata is not a supported model page or safe HTTPS source");
  }
  const url = safeSource || canonicalPage(page.url);
  const expectedCanonical = expectedUrl
    ? safeCaptureSourceUrl(expectedUrl) || canonicalPage(expectedUrl)
    : null;
  if (expectedCanonical && url !== expectedCanonical) {
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
    parentExternalId: cleanText(page.parentExternalId, 256),
  };
}

async function activePage() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  if (!tab?.id || !supportedPage(tab.url)) throw new Error("Open a supported model page first");
  const page = sanitizePage(
    await chrome.tabs.sendMessage(tab.id, { type: "page.extract" }),
    tab.url,
  );
  if (!page) throw new Error("This page is not a capturable model page");
  await captureStore.savePage(page, tab.id);
  return page;
}

async function recordFailure(downloadId, error) {
  await captureStore.recordFailure(downloadId, error);
}

async function enqueue(envelope) {
  return captureStore.enqueue(envelope);
}

async function envelope(page, downloads = []) {
  const identity = `${page.url}\0${
    downloads
      .map((download) => download.captureId)
      .sort()
      .join("\0") || "page"
  }`;
  return {
    version: 1,
    itemId: await stableId(identity),
    createdAt: new Date().toISOString(),
    page,
    downloads,
  };
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender?.id !== chrome.runtime.id) return;
  if (message?.type === "page.metadata" && sender.tab?.id != null) {
    if (!message.page) {
      return;
    }
    try {
      const page = sanitizePage(message.page, sender.url || sender.tab.url);
      void captureStore
        .savePage(page, sender.tab.id)
        .catch((error) => captureStore.recordFailure("page-metadata", error));
    } catch {}
    return;
  }
  if (message?.type === "capture.preview" && controlSender(sender)) {
    void (async () => {
      const page = await activePage();
      respond({ ok: true, page, enrichmentAvailable: await catalogEnricher.canEnrich(page.url) });
    })().catch((error) => respond({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === "capture.enrichPage" && controlSender(sender)) {
    void (async () => {
      const page = await activePage();
      const suggestions = await catalogEnricher.enrich(page.url);
      if (!suggestions) throw new Error("No public API metadata was available for this model");
      respond({ ok: true, suggestions });
    })().catch((error) => respond({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === "capture.activePage" && controlSender(sender)) {
    void (async () => {
      if (!message.review || typeof message.review !== "object" || Array.isArray(message.review)) {
        throw new Error("Review the model details before queueing");
      }
      const page = sanitizePage(reviewPage(await activePage(), message.review));
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
  void captureStore.removeTab(tabId);
  chrome.storage.local.remove(`page:${tabId}`);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "loading") void captureStore.removeTab(tabId);
});

chrome.downloads.onCreated.addListener((item) => {
  void captureStore.rememberDownload(item).catch((error) => void recordFailure(item.id, error));
});

chrome.downloads.onChanged.addListener((delta) => {
  if (delta.state?.current !== "complete") return;
  void (async () => {
    const [item] = await chrome.downloads.search({ id: delta.id });
    if (!item) return;
    const attributed = await captureStore.downloadPage(item);
    if (!attributed) return;
    const page = sanitizePage(attributed, attributed.url, true);
    await enqueue(
      await envelope(page, [
        {
          captureId: `download-${item.id}`,
          fileName: basename(item.filename),
          relativeInboxPath: null,
          sha256: null,
          bytes: item.fileSize ?? item.totalBytes ?? null,
        },
      ]),
    );
  })().catch((error) => void recordFailure(delta.id, error));
});
