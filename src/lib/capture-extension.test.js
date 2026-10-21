import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import {
  basename,
  canonicalPage,
  stableId,
  supportedPage,
} from "../../browser-extension/shared-module.js";
import * as captureShared from "../../browser-extension/shared-module.js";
import * as captureState from "../../browser-extension/capture-state.js";
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

function fakeElement() {
  const listeners = new Map();
  return {
    value: "",
    disabled: false,
    hidden: false,
    textContent: "",
    addEventListener(type, listener) {
      listeners.set(type, listener);
    },
    dispatch(type, event = {}) {
      return listeners.get(type)?.({ preventDefault() {}, ...event });
    },
  };
}

function popupFixture() {
  const elements = new Map([
    ["#capture-form", fakeElement()],
    ["#capture", fakeElement()],
    ["#enrich", fakeElement()],
    ["#status", fakeElement()],
    ["#title", fakeElement()],
    ["#designer", fakeElement()],
    ["#license", fakeElement()],
    ["#source-url", fakeElement()],
    ["#notes", fakeElement()],
  ]);
  elements.get("#enrich").hidden = true;
  return {
    elements,
    document: {
      querySelector(selector) {
        return elements.get(selector);
      },
    },
  };
}

let contentFixtureSequence = 0;

function contentDocument({
  title = "",
  metas = {},
  h1 = "",
  jsonLd = [],
  nextData = null,
  elements = {},
} = {}) {
  return {
    title,
    querySelector(selector) {
      if (selector.includes("meta[")) {
        const names = [...selector.matchAll(/(?:property|name)="([^"]+)"/g)].map(
          (match) => match[1],
        );
        const name = names.find((entry) => metas[entry]);
        return name ? { content: metas[name] } : null;
      }
      if (selector.includes("script#__NEXT_DATA__")) return nextData;
      if (selector.includes("h1") && h1) return { textContent: h1 };
      if (selector.includes('a[href*="/designer/"]')) return elements.designer ?? null;
      if (selector.includes('[rel="author"]')) return elements.author ?? null;
      if (selector.includes('[itemprop="license"]')) return elements.license ?? null;
      return null;
    },
    querySelectorAll(selector) {
      return selector.includes('type="application/ld+json"') ? jsonLd : [];
    },
  };
}

async function extractContentPage(url, document) {
  const messages = [];
  const listeners = [];
  const previous = {
    document: globalThis.document,
    location: globalThis.location,
    chrome: globalThis.chrome,
    setInterval: globalThis.setInterval,
    GoggleCapture: globalThis.GoggleCapture,
  };
  globalThis.document = document;
  globalThis.location = { href: url };
  globalThis.chrome = {
    runtime: {
      sendMessage(message) {
        messages.push(message);
        return Promise.resolve();
      },
      onMessage: {
        addListener(listener) {
          listeners.push(listener);
        },
      },
    },
  };
  globalThis.setInterval = () => 1;
  try {
    const fixtureId = `${Date.now()}-${++contentFixtureSequence}`;
    await import(`../../browser-extension/shared.js?content=${fixtureId}`);
    await import(`../../browser-extension/content.js?content=${fixtureId}`);
    return { page: messages.find((message) => message.type === "page.metadata")?.page, listeners };
  } finally {
    if (previous.document === undefined) delete globalThis.document;
    else globalThis.document = previous.document;
    if (previous.location === undefined) delete globalThis.location;
    else globalThis.location = previous.location;
    if (previous.chrome === undefined) delete globalThis.chrome;
    else globalThis.chrome = previous.chrome;
    if (previous.setInterval === undefined) delete globalThis.setInterval;
    else globalThis.setInterval = previous.setInterval;
    if (previous.GoggleCapture === undefined) delete globalThis.GoggleCapture;
    else globalThis.GoggleCapture = previous.GoggleCapture;
  }
}

test("capture extension admits HTTPS model pages on catalog roots and their subdomains", () => {
  for (const url of [
    "https://makerworld.com/en/models/123-part",
    "https://www.makerworld.com/en-us/models/123-part",
    "https://cdn.makerworld.com/en/models/123-part",
    "https://www.thangs.com/designer/ada/3d-model/widget-123",
    "https://thangs.com/model/widget-123",
    "https://www.thangs.com/design/widget-123",
    "https://thangs.com/3d-model/widget-123",
    "https://www.printables.com/model/123-part",
    "https://printables.com/prints/123-part",
    "https://thingiverse.com/thing:123",
    "https://www.thingiverse.com/thing/123",
    "https://cults3d.com/en/3d-model/tool/widget",
    "https://www.cults3d.com/design/widget",
    "https://cults3d.com/3d-model/widget",
    "https://www.myminifactory.com/object/3d-print-widget-123",
    "https://myminifactory.com/object/widget-123",
  ])
    expect(supportedPage(url)).toBe(true);
  expect(supportedPage("http://www.printables.com/model/123-part")).toBe(false);
  expect(supportedPage("https://user:secret@www.printables.com/model/123-part")).toBe(false);
  expect(supportedPage("https://www.printables.com:444/model/123-part")).toBe(false);
  expect(supportedPage("https://printables.com.attacker.test/model/123")).toBe(false);
  expect(supportedPage("https://nested.files.printables.com/prints/123-part")).toBe(true);
  expect(supportedPage("https://makerworld.com.attacker.test/en/models/123")).toBe(false);
  expect(supportedPage("https://evilmakerworld.com/en/models/123")).toBe(false);
  expect(supportedPage("https://printables.com.evil/prints/123")).toBe(false);
  expect(supportedPage("https://www.printables.com/not-a-model/123")).toBe(false);
  expect(supportedPage("https://www.printables.com/prints/")).toBe(false);
  expect(supportedPage("https://www.thingiverse.com/thing/")).toBe(false);
  expect(supportedPage("https://thangs.com/model/")).toBe(false);
  expect(supportedPage("https://cults3d.com/design/")).toBe(false);
  expect(supportedPage("https://myminifactory.com/object/")).toBe(false);
  expect(supportedPage(`https://www.printables.com/model/123-${"x".repeat(2048)}`)).toBe(false);
  expect(supportedPage("https://www.printables.com/")).toBe(false);
});

test("local extraction reads JSON-LD 3DModel arrays and bounded Next data", async () => {
  const jsonLd = await extractContentPage(
    "https://makerworld.com/en/models/123-part",
    contentDocument({
      jsonLd: [
        {
          textContent: JSON.stringify({
            "@graph": [
              {
                "@type": ["Thing", "3DModel"],
                name: "Structured model title",
                author: { name: "Structured designer" },
              },
            ],
          }),
        },
      ],
    }),
  );
  expect(jsonLd.page.title).toBe("Structured model title");
  expect(jsonLd.page.designer).toBe("Structured designer");

  const nextData = await extractContentPage(
    "https://www.printables.com/model/77-part",
    contentDocument({
      nextData: {
        textContent: JSON.stringify({
          props: {
            pageProps: {
              model: {
                id: 77,
                title: "Next data model",
                designer: { name: "Next designer" },
                description: "Local model description",
                license: { name: "CC-BY" },
              },
            },
          },
        }),
      },
    }),
  );
  expect(nextData.page).toMatchObject({
    title: "Next data model",
    designer: "Next designer",
    license: "CC-BY",
    notes: "Local model description",
  });
});

test("local extraction falls back to social metadata and site-aware DOM slugs", async () => {
  const social = await extractContentPage(
    "https://thingiverse.com/thing:123",
    contentDocument({
      title: "Thingiverse",
      metas: {
        "twitter:title": "Twitter model title",
        "twitter:description": "Twitter model notes",
        "twitter:creator": "@Ada",
        "og:license": "CC0",
      },
    }),
  );
  expect(social.page).toMatchObject({
    title: "Twitter model title",
    designer: "@Ada",
    license: "CC0",
    notes: "Twitter model notes",
  });

  const slug = await extractContentPage(
    "https://thangs.com/designer/ada/3d-model/variable-vase-123",
    contentDocument({
      title: "Thangs",
      elements: { designer: { textContent: "Ada Lovelace" } },
    }),
  );
  expect(slug.page.title).toBe("Variable Vase");
  expect(slug.page.designer).toBe("Ada Lovelace");
});

test("oversized Next data is ignored in favor of bounded local fallbacks", async () => {
  const result = await extractContentPage(
    "https://makerworld.com/en/models/123-part",
    contentDocument({
      nextData: { textContent: "x".repeat(64 * 1024 + 1) },
      metas: { "twitter:title": "Safe social title", "twitter:description": "Safe notes" },
    }),
  );
  expect(result.page.title).toBe("Safe social title");
  expect(result.page.notes).toBe("Safe notes");
});

test("catalog host admission covers only HTTPS root and dot-boundary subdomains", () => {
  expect(typeof captureShared.supportedCatalogUrl).toBe("function");
  for (const url of [
    "https://makerworld.com/any/path",
    "https://www.makerworld.com/any/path",
    "https://cdn.assets.makerworld.com/any/path",
    "https://printables.com/account/downloads",
    "https://models.printables.com/account/downloads",
  ]) {
    expect(captureShared.supportedCatalogUrl(url)).toBe(true);
  }
  for (const url of [
    "http://www.makerworld.com/any/path",
    "https://user:secret@www.printables.com/any/path",
    "https://printables.com:444/any/path",
    "https://makerworld.com.attacker.test/any/path",
    "https://evilmakerworld.com/any/path",
  ]) {
    expect(captureShared.supportedCatalogUrl(url)).toBe(false);
  }
});

test("capture manifest declares only the six catalog root and wildcard host families", async () => {
  const manifest = JSON.parse(
    await readFile(new URL("../../browser-extension/manifest.json", import.meta.url), "utf8"),
  );
  const roots = [
    "makerworld.com",
    "thangs.com",
    "printables.com",
    "thingiverse.com",
    "cults3d.com",
    "myminifactory.com",
  ];
  for (const root of roots) {
    expect(manifest.host_permissions).toContain(`https://${root}/*`);
    expect(manifest.host_permissions).toContain(`https://*.${root}/*`);
    expect(manifest.content_scripts[0].matches).toContain(`https://${root}/*`);
    expect(manifest.content_scripts[0].matches).toContain(`https://*.${root}/*`);
  }
  expect(manifest.optional_host_permissions).toEqual([
    "https://api.printables.com/*",
    "https://api.bambulab.com/*",
  ]);
  expect(manifest.permissions).toContain("activeTab");
  expect(manifest.permissions).not.toContain("tabs");
  expect(manifest.host_permissions).not.toContain("https://*/*");
});

test("download policy captures catalog-site files and only safe model formats elsewhere", () => {
  expect(typeof captureState.shouldCaptureDownload).toBe("function");
  expect(
    captureState.shouldCaptureDownload({
      filename: "/Downloads/reference.pdf",
      referrer: "https://www.printables.com/account/downloads",
    }),
  ).toBe(true);
  for (const filename of ["part.stl", "part.obj", "part.3mf"]) {
    expect(
      captureState.shouldCaptureDownload({
        filename: `/Downloads/${filename}`,
        referrer: "https://models.example.org/design/42?tracking=1",
      }),
    ).toBe(true);
  }
  for (const item of [
    { filename: "/Downloads/archive.zip", referrer: "https://example.org/design/42" },
    { filename: "/Downloads/reference.pdf", referrer: "https://example.org/design/42" },
    { filename: "/Downloads/part.stl", referrer: "https://user:secret@example.org/design/42" },
    { filename: "/Downloads/part.stl", referrer: "https://example.org:444/design/42" },
    { filename: "/Downloads/part.stl", referrer: "http://example.org/design/42" },
    { filename: "/Downloads/part.stl", referrer: "https://localhost/design/42" },
    { filename: "/Downloads/part.stl", referrer: "https://127.0.0.1/design/42" },
    { filename: "/Downloads/part.stl", referrer: "https://models.example.test/design/42" },
    { filename: `/Downloads/${"a".repeat(252)}.stl`, referrer: "https://example.org/design/42" },
    { filename: "/Downloads/part\u0000.stl", referrer: "https://example.org/design/42" },
  ]) {
    expect(captureState.shouldCaptureDownload(item)).toBe(false);
  }
});

test("unattributed model downloads become minimal HTTPS bookmarks without local paths", async () => {
  const store = createCaptureStore(storageFixture(), async () => {});
  await store.rememberDownload({
    id: 41,
    referrer: "https://models.example.org/design/42?utm_source=download#part",
    url: "https://cdn.example.org/files/part.stl?signature=private",
    filename: "C:\\Users\\person\\Downloads\\part.stl",
  });
  expect(
    await store.downloadPage({ id: 41, filename: "C:\\Users\\person\\Downloads\\part.stl" }),
  ).toEqual({
    url: "https://models.example.org/design/42",
    title: "part.stl",
    designer: null,
    license: null,
    notes: null,
    parentExternalId: null,
  });

  const catalogStore = createCaptureStore(storageFixture(), async () => {});
  await catalogStore.rememberDownload({
    id: 42,
    referrer: "https://eu.printables.com/account/export?session=private",
    url: "https://cdn.example.org/files/guide.pdf",
    filename: "/Downloads/guide.pdf",
  });
  expect(await catalogStore.downloadPage({ id: 42, filename: "/Downloads/guide.pdf" })).toEqual({
    url: "https://eu.printables.com/account/export",
    title: "guide.pdf",
    designer: null,
    license: null,
    notes: null,
    parentExternalId: null,
  });
});

test("reviewed popup fields are bounded while extracted parent relation stays unchanged", () => {
  expect(typeof captureShared.reviewPage).toBe("function");
  const source = {
    ...page("https://makerworld.com/en/models/123-part"),
    parentExternalId: "collection:42",
  };
  const reviewed = captureShared.reviewPage(source, {
    title: "  Edited title  ",
    designer: "  Ada Lovelace  ",
    license: "",
    sourceUrl: "https://www.printables.com/prints/77-model?utm_source=popup",
    notes: "  Reviewed note  ",
    parentExternalId: "must-not-replace-relation",
  });
  expect(reviewed).toEqual({
    url: "https://www.printables.com/prints/77-model",
    title: "Edited title",
    designer: "Ada Lovelace",
    license: null,
    notes: "Reviewed note",
    parentExternalId: "collection:42",
  });
  expect(() => captureShared.reviewPage(source, { title: " ", sourceUrl: source.url })).toThrow(
    "title",
  );
  expect(() =>
    captureShared.reviewPage(source, {
      title: "ok",
      sourceUrl: "https://www.printables.com.attacker.test/prints/77",
    }),
  ).toThrow("source URL");
  expect(() =>
    captureShared.reviewPage(source, {
      title: "ok",
      sourceUrl: "https://user@www.printables.com/prints/77",
    }),
  ).toThrow("source URL");
  expect(() =>
    captureShared.reviewPage(source, {
      title: "ok",
      sourceUrl: "https://www.printables.com:444/prints/77",
    }),
  ).toThrow("source URL");
});

test("popup loads extracted fields for review, then queues only the reviewed metadata", async () => {
  const { document, elements } = popupFixture();
  const messages = [];
  const runtime = {
    async sendMessage(message) {
      messages.push(structuredClone(message));
      if (message.type === "capture.preview")
        return {
          ok: true,
          page: page("https://www.printables.com/model/123-part", "Original title"),
        };
      return { ok: true };
    },
  };
  const previousDocument = globalThis.document;
  const previousChrome = globalThis.chrome;
  globalThis.document = document;
  globalThis.chrome = { runtime };
  try {
    const popup = await import(`../../browser-extension/popup.js?test=${Date.now()}`);
    await popup.popupReady;
    expect(messages).toEqual([{ type: "capture.preview" }]);
    expect(elements.get("#title").value).toBe("Original title");
    expect(elements.get("#source-url").value).toBe("https://www.printables.com/model/123-part");
    expect(elements.get("#enrich").hidden).toBe(true);
    elements.get("#title").value = "Reviewed title";
    elements.get("#designer").value = "Reviewed designer";
    elements.get("#license").value = "CC-BY";
    elements.get("#source-url").value = "https://www.printables.com/prints/123-part";
    elements.get("#notes").value = "Reviewed notes";
    await elements.get("#capture-form").dispatch("submit");
    expect(messages[1]).toEqual({
      type: "capture.activePage",
      review: {
        title: "Reviewed title",
        designer: "Reviewed designer",
        license: "CC-BY",
        sourceUrl: "https://www.printables.com/prints/123-part",
        notes: "Reviewed notes",
      },
    });
    expect(elements.get("#status").textContent).toMatch(/queued/i);
  } finally {
    if (previousDocument === undefined) delete globalThis.document;
    else globalThis.document = previousDocument;
    if (previousChrome === undefined) delete globalThis.chrome;
    else globalThis.chrome = previousChrome;
  }
});

test("popup enrichment runs only on click, fills editable suggestions, and never queues automatically", async () => {
  const { document, elements } = popupFixture();
  const messages = [];
  const runtime = {
    async sendMessage(message) {
      messages.push(structuredClone(message));
      if (message.type === "capture.preview") {
        return {
          ok: true,
          enrichmentAvailable: true,
          page: page("https://www.printables.com/model/123-part", "Local title"),
        };
      }
      if (message.type === "capture.enrichPage") {
        return {
          ok: true,
          suggestions: {
            title: "Public catalog title",
            designer: "Public maker",
            license: "CC-BY",
            notes: "Public notes",
          },
        };
      }
      return { ok: true };
    },
  };
  const previousDocument = globalThis.document;
  const previousChrome = globalThis.chrome;
  globalThis.document = document;
  globalThis.chrome = { runtime };
  try {
    const popup = await import(`../../browser-extension/popup.js?enrichment=${Date.now()}`);
    await popup.popupReady;
    expect(elements.get("#enrich").hidden).toBe(false);
    expect(messages).toEqual([{ type: "capture.preview" }]);

    await elements.get("#enrich").dispatch("click");
    expect(messages[1]).toEqual({ type: "capture.enrichPage" });
    expect(elements.get("#title").value).toBe("Public catalog title");
    expect(elements.get("#designer").value).toBe("Public maker");
    expect(elements.get("#source-url").value).toBe("https://www.printables.com/model/123-part");
    expect(elements.get("#status").textContent).toMatch(/review/i);
    expect(messages.some((message) => message.type === "capture.activePage")).toBe(false);

    elements.get("#title").value = "Reviewed API title";
    await elements.get("#capture-form").dispatch("submit");
    expect(messages[2]).toMatchObject({
      type: "capture.activePage",
      review: { title: "Reviewed API title", designer: "Public maker" },
    });
  } finally {
    if (previousDocument === undefined) delete globalThis.document;
    else globalThis.document = previousDocument;
    if (previousChrome === undefined) delete globalThis.chrome;
    else globalThis.chrome = previousChrome;
  }
});

test("popup keeps local fields when an explicit enrichment request fails", async () => {
  const { document, elements } = popupFixture();
  const messages = [];
  const runtime = {
    async sendMessage(message) {
      messages.push(structuredClone(message));
      if (message.type === "capture.preview") {
        return {
          ok: true,
          enrichmentAvailable: true,
          page: page("https://www.printables.com/model/123-part", "Local title"),
        };
      }
      return { ok: false, error: "Catalog is temporarily unavailable" };
    },
  };
  const previousDocument = globalThis.document;
  const previousChrome = globalThis.chrome;
  globalThis.document = document;
  globalThis.chrome = { runtime };
  try {
    const popup = await import(`../../browser-extension/popup.js?enrichment-failure=${Date.now()}`);
    await popup.popupReady;
    await elements.get("#enrich").dispatch("click");
    expect(elements.get("#title").value).toBe("Local title");
    expect(elements.get("#enrich").disabled).toBe(false);
    expect(elements.get("#status").textContent).toBe("Catalog is temporarily unavailable");
    expect(messages.some((message) => message.type === "capture.activePage")).toBe(false);
  } finally {
    if (previousDocument === undefined) delete globalThis.document;
    else globalThis.document = previousDocument;
    if (previousChrome === undefined) delete globalThis.chrome;
    else globalThis.chrome = previousChrome;
  }
});

test("API enrichment permissions and setting stay off until the user enables them", async () => {
  const api = await import("../../browser-extension/api-enrichment.js");
  expect(api.OPTIONAL_API_ORIGINS).toEqual([
    "https://api.printables.com/*",
    "https://api.bambulab.com/*",
  ]);
  const storage = storageFixture();
  const calls = [];
  let grant = true;
  const permissions = {
    async contains(request) {
      calls.push({ type: "contains", request });
      return false;
    },
    async request(request) {
      calls.push({ type: "request", request });
      return grant;
    },
    async remove(request) {
      calls.push({ type: "remove", request });
      return true;
    },
  };
  const settings = api.createEnrichmentSettings(storage, permissions);
  const checkbox = fakeElement();
  checkbox.checked = false;
  const status = fakeElement();
  const binding = api.bindEnrichmentSetting(checkbox, status, settings);
  await binding.ready;
  expect(checkbox.checked).toBe(false);
  expect(calls.some((call) => call.type === "request")).toBe(false);

  checkbox.checked = true;
  await checkbox.dispatch("change");
  expect(storage.values.apiEnrichmentEnabled).toBe(true);
  expect(calls.find((call) => call.type === "request").request).toEqual({
    origins: api.OPTIONAL_API_ORIGINS,
  });

  checkbox.checked = false;
  await checkbox.dispatch("change");
  expect(storage.values.apiEnrichmentEnabled).toBe(false);
  expect(calls.find((call) => call.type === "remove").request).toEqual({
    origins: api.OPTIONAL_API_ORIGINS,
  });

  grant = false;
  checkbox.checked = true;
  await checkbox.dispatch("change");
  expect(checkbox.checked).toBe(false);
  expect(storage.values.apiEnrichmentEnabled).toBe(false);
  expect(status.textContent).toMatch(/not granted/i);
});

test("Printables enrichment sends only the public model ID without cookies", async () => {
  const api = await import("../../browser-extension/api-enrichment.js");
  const calls = [];
  const enricher = api.createCatalogEnricher({
    isEnabled: async () => true,
    hasPermission: async (origin) => origin === "https://api.printables.com/*",
    fetchFn: async (url, init) => {
      calls.push({ url, init });
      return new Response(
        JSON.stringify({
          data: {
            print: {
              id: "123",
              name: "Public Printables title",
              summary: "Short notes",
              description: "<p>Long <strong>notes</strong></p>",
              user: { handle: "maker", publicUsername: "Maker Public" },
              license: { id: "cc0", name: "CC0" },
            },
          },
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    },
  });
  const result = await enricher.enrich("https://www.printables.com/prints/123-part");
  expect(result).toEqual({
    title: "Public Printables title",
    designer: "Maker Public",
    license: "CC0",
    notes: "Long notes",
  });
  expect(calls).toHaveLength(1);
  expect(calls[0].url).toBe("https://api.printables.com/graphql/");
  expect(calls[0].init).toMatchObject({
    method: "POST",
    credentials: "omit",
    redirect: "error",
  });
  const request = JSON.parse(calls[0].init.body);
  expect(request.operationName).toBe("PrintProfile");
  expect(request.variables).toEqual({ id: "123" });
  expect(request.query).toContain("print(id: $id)");
  expect(request.query).toContain("user { handle publicUsername }");
});

test("MakerWorld enrichment tries its origin then only uses Bambu with optional permission", async () => {
  const api = await import("../../browser-extension/api-enrichment.js");
  const calls = [];
  const enricher = api.createCatalogEnricher({
    isEnabled: async () => true,
    hasPermission: async (origin) => origin === "https://api.bambulab.com/*",
    fetchFn: async (url, init) => {
      calls.push({ url, init });
      if (url === "https://api.bambulab.com/v1/design-service/design/456") {
        return new Response(
          JSON.stringify({
            data: {
              title: "Public MakerWorld title",
              designCreator: { name: "Public designer" },
              summary: "<p>Public summary</p>",
              license: { name: "CC-BY" },
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      return new Response("", { status: 404 });
    },
  });
  const result = await enricher.enrich("https://www.makerworld.com/en/models/456-part");
  expect(result).toEqual({
    title: "Public MakerWorld title",
    designer: "Public designer",
    license: "CC-BY",
    notes: "Public summary",
  });
  expect(calls.map((call) => call.url)).toEqual([
    "https://www.makerworld.com/api/v1/design-service/design/456",
    "https://makerworld.com/api/v1/design-service/design/456",
    "https://api.bambulab.com/v1/design-service/design/456",
  ]);
  expect(calls.every((call) => call.init.credentials === "omit")).toBe(true);
});

test("API enrichment refuses disabled, unpermitted, oversized, and timed-out requests", async () => {
  const api = await import("../../browser-extension/api-enrichment.js");
  let fetchCalls = 0;
  const disabled = api.createCatalogEnricher({
    isEnabled: async () => false,
    hasPermission: async () => true,
    fetchFn: async () => {
      fetchCalls += 1;
      return new Response("{}");
    },
  });
  expect(await disabled.enrich("https://www.printables.com/model/5-part")).toBeNull();
  expect(fetchCalls).toBe(0);

  const unpermitted = api.createCatalogEnricher({
    isEnabled: async () => true,
    hasPermission: async () => false,
    fetchFn: async () => {
      fetchCalls += 1;
      return new Response("{}");
    },
  });
  expect(await unpermitted.enrich("https://www.printables.com/model/5-part")).toBeNull();
  expect(fetchCalls).toBe(0);

  const oversized = api.createCatalogEnricher({
    isEnabled: async () => true,
    hasPermission: async () => true,
    fetchFn: async () => new Response("x".repeat(70 * 1024), { status: 200 }),
  });
  expect(await oversized.enrich("https://www.printables.com/model/5-part")).toBeNull();

  const timeout = api.createCatalogEnricher({
    isEnabled: async () => true,
    hasPermission: async () => true,
    timeoutMs: 5,
    fetchFn: (_url, init) =>
      new Promise((_resolve, reject) => {
        init.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
      }),
  });
  expect(await timeout.enrich("https://www.printables.com/model/5-part")).toBeNull();
});

test("classic content-script and module validators enforce the same boundary", async () => {
  delete globalThis.GoggleCapture;
  await import(`../../browser-extension/shared.js?validator=${Date.now()}`);
  const supported = [
    "https://makerworld.com/en/models/123-part",
    "https://www.makerworld.com/en-us/models/123-part",
    "https://cdn.makerworld.com/en/models/123-part",
    "https://thangs.com/model/widget-123",
    "https://www.thangs.com/design/widget-123",
    "https://www.printables.com/prints/123-part",
    "https://nested.assets.printables.com/prints/124-part",
    "https://thingiverse.com/thing:123",
    "https://www.thingiverse.com/thing/123",
    "https://cults3d.com/en/3d-model/tool/widget",
    "https://www.cults3d.com/design/widget",
    "https://myminifactory.com/object/widget-123",
  ];
  const unsupported = [
    "https://evilmakerworld.com/en/models/123-part",
    "https://makerworld.com.attacker.test/en/models/123-part",
    "https://user@printables.com/model/123-part",
    "https://www.printables.com:444/prints/123-part",
    "https://thingiverse.com/prefix/thing:123",
    "https://thangs.com.attacker.test/model/widget-123",
  ];
  for (const url of [...supported, ...unsupported]) {
    expect(globalThis.GoggleCapture.supportedPage(url)).toBe(supportedPage(url));
  }
  for (const url of supported) {
    expect(globalThis.GoggleCapture.supportedPage(url)).toBe(true);
  }
  for (const url of unsupported) {
    expect(globalThis.GoggleCapture.supportedPage(url)).toBe(false);
  }
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

test("CDN downloads use the recent model snapshot from their originating tab", async () => {
  const storage = storageFixture();
  const store = createCaptureStore(storage, async () => {});
  const source = page("https://www.printables.com/model/321-part", "Captured tab model");
  await store.savePage(source, 27);
  const item = {
    id: 88,
    tabId: 27,
    url: "https://cdn.example.org/files/part.stl?token=private",
    finalUrl: "https://cdn.example.org/files/part.stl?token=private",
    filename: "C:\\Users\\person\\Downloads\\part.stl",
  };
  await store.rememberDownload(item);
  expect(await store.downloadPage(item)).toMatchObject({
    url: source.url,
    title: "Captured tab model",
  });
});

test("tab fallback still rejects ambiguous model URLs and expires or clears old snapshots", async () => {
  const start = Date.parse("2026-10-13T12:00:00Z");
  let current = start;
  const storage = storageFixture();
  const store = createCaptureStore(
    storage,
    async () => {},
    () => new Date(current).toISOString(),
  );
  await store.savePage(page("https://www.printables.com/model/1"), 7);
  await expect(
    store.downloadPage({
      id: 90,
      tabId: 7,
      referrer: "https://www.printables.com/model/1",
      url: "https://makerworld.com/en/models/2",
      filename: "/Downloads/part.stl",
    }),
  ).rejects.toThrow("ambiguous");

  current += 31 * 60 * 1000;
  expect(
    await store.downloadPage({
      id: 91,
      tabId: 7,
      url: "https://cdn.example.org/files/guide.pdf",
      filename: "/Downloads/guide.pdf",
    }),
  ).toBeNull();
  await store.removeTab(7);
  expect(storage.values.captureTabPages).toEqual([]);

  const boundedStorage = storageFixture();
  const boundedStore = createCaptureStore(boundedStorage, async () => {});
  for (let tabId = 0; tabId < 105; tabId += 1) {
    await boundedStore.savePage(page(`https://www.printables.com/model/${tabId}`), tabId);
  }
  expect(boundedStorage.values.captureTabPages).toHaveLength(100);
  expect(boundedStorage.values.captureTabPages[0].tabId).toBe(5);
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
