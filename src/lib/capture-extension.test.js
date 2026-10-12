import { expect, test } from "bun:test";
import {
  basename,
  canonicalPage,
  stableId,
  supportedPage,
} from "../../browser-extension/shared-module.js";
import {
  createCaptureStore,
  downloadPageUrl,
  MAX_QUEUE,
  queueableDownloadName,
  trustedControlSender,
} from "../../browser-extension/capture-state.js";
import { createInboxWriter } from "../../browser-extension/inbox.js";

function storageFixture(initial = {}) {
  const values = structuredClone(initial);
  return {
    values,
    async get(defaults) {
      await Promise.resolve();
      const result = {};
      for (const [key, fallback] of Object.entries(defaults)) result[key] = values[key] ?? fallback;
      return structuredClone(result);
    },
    async set(update) {
      await Promise.resolve();
      Object.assign(values, structuredClone(update));
    },
  };
}

function page(url, title = "Detailed model") {
  return {
    url,
    title,
    designer: "Ada",
    license: "CC0",
    notes: "Rich notes",
    parentExternalId: null,
  };
}

function envelope(itemId, createdAt = "2026-10-11T00:00:00Z") {
  return {
    version: 1,
    itemId,
    createdAt,
    page: page(`https://www.printables.com/model/${itemId}`),
    downloads: [],
  };
}

test("capture extension admits only HTTPS model pages on exact supported hosts", () => {
  for (const url of [
    "https://makerworld.com/en/models/123-part",
    "https://www.thangs.com/designer/ada/3d-model/widget-123",
    "https://www.printables.com/model/123-part",
    "https://thingiverse.com/thing:123",
    "https://cults3d.com/en/3d-model/tool/widget",
    "https://www.myminifactory.com/object/3d-print-widget-123",
  ])
    expect(supportedPage(url)).toBe(true);
  expect(supportedPage("http://www.printables.com/model/123-part")).toBe(false);
  expect(supportedPage("https://user:secret@www.printables.com/model/123-part")).toBe(false);
  expect(supportedPage("https://www.printables.com:444/model/123-part")).toBe(false);
  expect(supportedPage("https://printables.com.attacker.test/model/123")).toBe(false);
  expect(supportedPage("https://evil.printables.com/model/123")).toBe(false);
  expect(supportedPage("https://www.printables.com/not-a-model/123")).toBe(false);
  expect(supportedPage(`https://www.printables.com/model/123-${"x".repeat(2048)}`)).toBe(false);
  expect(supportedPage("https://www.printables.com/")).toBe(false);
});

test("classic content-script and module validators enforce the same boundary", async () => {
  delete globalThis.GoggleCapture;
  await import(`../../browser-extension/shared.js?validator=${Date.now()}`);
  for (const url of [
    "https://makerworld.com/en/models/123-part",
    "https://evil.makerworld.com/en/models/123-part",
    "https://user@printables.com/model/123-part",
    "https://thingiverse.com/thing:123",
    "https://thingiverse.com/prefix/thing:123",
  ])
    expect(globalThis.GoggleCapture.supportedPage(url)).toBe(supportedPage(url));
  delete globalThis.GoggleCapture;
});

test("capture identities are deterministic and canonical pages drop tracking", async () => {
  expect(await stableId("page\0download-1")).toBe(await stableId("page\0download-1"));
  expect(canonicalPage("https://www.printables.com/model/1?utm_source=x&keep=y#part")).toBe(
    "https://www.printables.com/model/1",
  );
  expect(basename("C:\\Downloads\\part.stl")).toBe("part.stl");
});

test("simultaneous queue writes are serialized without losing captures", async () => {
  const storage = storageFixture();
  const inbox = [];
  const store = createCaptureStore(storage, async (item) => inbox.push(item.itemId));
  await Promise.all([store.enqueue(envelope("one")), store.enqueue(envelope("two"))]);
  expect(storage.values.captureQueue.map((item) => item.itemId)).toEqual(["one", "two"]);
  expect(inbox).toEqual(["one", "two"]);
});

test("download attribution keeps rich referrer metadata across navigation and worker restart", async () => {
  const storage = storageFixture();
  const original = page("https://www.printables.com/model/123?tracking=1", "Original rich title");
  let store = createCaptureStore(storage, async () => {});
  await store.savePage(original);
  await store.rememberDownload({
    id: 7,
    referrer: original.url,
    url: "https://files.printables.com/part.stl",
    filename: "/Downloads/part.stl",
  });
  await store.savePage(page("https://www.printables.com/model/999", "Navigated elsewhere"));
  store = createCaptureStore(storage, async () => {});
  expect((await store.downloadPage({ id: 7, filename: "/Downloads/part.stl" })).title).toBe(
    "Original rich title",
  );
});

test("download creation snapshots supported referrer before Chrome supplies the final filename", async () => {
  const storage = storageFixture();
  const original = page("https://www.printables.com/model/321-part", "Captured before download");
  let store = createCaptureStore(storage, async () => {});
  await store.savePage(original);
  await store.rememberDownload({
    id: 12,
    referrer: original.url,
    url: "https://files.example/transfer",
    filename: "",
  });
  await store.savePage(page("https://www.printables.com/model/999-other", "Later page"));
  store = createCaptureStore(storage, async () => {});
  expect((await store.downloadPage({ id: 12, filename: "/Downloads/final-part.stl" })).title).toBe(
    "Captured before download",
  );
});

test("supported-page OBJ and ZIP attachments retain metadata while filenames alone authorize nothing", async () => {
  expect(["part.stl", "part.3mf", "source.obj", "bundle.zip"].every(queueableDownloadName)).toBe(
    true,
  );
  expect(queueableDownloadName("notes.pdf")).toBe(false);
  const storage = storageFixture();
  const store = createCaptureStore(storage, async () => {});
  const source = page("https://www.printables.com/model/654-attachments", "Attachment model");
  await store.savePage(source);
  await store.rememberDownload({ id: 20, referrer: source.url, filename: "" });
  expect((await store.downloadPage({ id: 20, filename: "/Downloads/source.zip" })).title).toBe(
    "Attachment model",
  );
  expect(await store.downloadPage({ id: 21, filename: "/private/arbitrary.obj" })).toBeNull();
});

test("ambiguous supported download attribution fails instead of inventing a page", () => {
  expect(() =>
    downloadPageUrl({
      referrer: "https://www.printables.com/model/1",
      url: "https://makerworld.com/en/models/2",
    }),
  ).toThrow("ambiguous");
});

test("queue exact replay is idempotent, conflicts fail, and retry rewrites the same item", async () => {
  const storage = storageFixture();
  const inbox = [];
  const store = createCaptureStore(storage, async (item) => {
    inbox.push(item.itemId);
    return { ok: true };
  });
  const first = envelope("same");
  await store.enqueue(first);
  await store.enqueue({ ...structuredClone(first), createdAt: "later" });
  expect(storage.values.captureQueue).toHaveLength(1);
  await expect(
    store.enqueue({
      ...structuredClone(first),
      page: page("https://www.printables.com/model/other"),
    }),
  ).rejects.toThrow("different metadata");
  await store.retry("same");
  expect(inbox).toEqual(["same", "same"]);
});

test("queue capacity and Windows path input stay bounded", async () => {
  const full = Array.from({ length: MAX_QUEUE }, (_, index) => envelope(String(index)));
  const store = createCaptureStore(storageFixture({ captureQueue: full }), async () => {});
  await expect(store.enqueue(envelope("overflow"))).rejects.toThrow("queue is full");
  expect(basename("C:\\Users\\person\\Downloads\\part.stl")).toBe("part.stl");
});

test("control messages require the extension identity and extension page sender", () => {
  const root = "chrome-extension://good/";
  expect(trustedControlSender({ id: "good", url: `${root}popup.html` }, "good", root)).toBe(true);
  expect(trustedControlSender({ id: "evil", url: `${root}popup.html` }, "good", root)).toBe(false);
  expect(
    trustedControlSender({ id: "good", url: `${root}popup.html`, tab: { id: 4 } }, "good", root),
  ).toBe(false);
  expect(trustedControlSender({ id: "good", url: "https://attacker.test/" }, "good", root)).toBe(
    false,
  );
});

test("inbox writer uses the chosen folder and falls back to Downloads after revocation", async () => {
  const folderWriter = createInboxWriter({
    offscreen: { hasDocument: async () => true, createDocument: async () => {} },
    runtime: { sendMessage: async () => ({ ok: true, via: "folder" }) },
    downloads: {
      download: async () => {
        throw new Error("unexpected fallback");
      },
    },
  });
  expect(await folderWriter(envelope("folder"))).toEqual({ ok: true, via: "folder" });
  let downloaded = null;
  const fallbackWriter = createInboxWriter({
    offscreen: {
      hasDocument: async () => false,
      createDocument: async () => {
        throw new Error("revoked");
      },
    },
    runtime: { sendMessage: async () => ({ ok: false }) },
    downloads: {
      download: async (request) => {
        downloaded = request;
        return 17;
      },
    },
  });
  expect((await fallbackWriter(envelope("fallback"))).via).toBe("downloads");
  expect(downloaded.filename).toBe("GoggleLabInbox/fallback.gogglelab-capture.json");
  expect(downloaded.url.startsWith("data:application/json;base64,")).toBe(true);
  downloaded = null;
  await fallbackWriter({ ...envelope("unsafe"), itemId: "../../escape" });
  expect(downloaded.filename.startsWith("GoggleLabInbox/")).toBe(true);
  expect(downloaded.filename).not.toContain("..");
});
