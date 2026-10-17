const { boundedText, canonicalPage, supportedPage } = globalThis.GoggleCapture;
const MAX_JSON_SCRIPT_LENGTH = 64 * 1024;
const MAX_JSON_LD_SCRIPTS = 16;
const MAX_NEXT_DATA_NODES = 2048;
const MAX_NEXT_DATA_DEPTH = 12;
const MAX_NEXT_DATA_CHILDREN = 80;
const SITE_ROOTS = new Map([
  ["makerworld.com", "makerworld"],
  ["thangs.com", "thangs"],
  ["printables.com", "printables"],
  ["thingiverse.com", "thingiverse"],
  ["cults3d.com", "cults"],
  ["myminifactory.com", "myminifactory"],
]);

function meta(name) {
  return document.querySelector(`meta[property="${name}"],meta[name="${name}"]`)?.content ?? null;
}

function siteFamily(hostname) {
  for (const [root, family] of SITE_ROOTS) {
    if (hostname === root || hostname.endsWith(`.${root}`)) return family;
  }
  return null;
}

function displayValue(value) {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  for (const key of ["name", "displayName", "title", "username", "handle"]) {
    if (typeof value[key] === "string") return value[key];
  }
  return null;
}

function firstValue(record, keys) {
  if (!record || typeof record !== "object") return null;
  for (const key of keys) {
    const value = displayValue(record[key]);
    if (value) return value;
  }
  return null;
}

function structuredData() {
  const scripts = [...document.querySelectorAll('script[type="application/ld+json"]')].slice(
    0,
    MAX_JSON_LD_SCRIPTS,
  );
  for (const script of scripts) {
    if (!script.textContent || script.textContent.length > MAX_JSON_SCRIPT_LENGTH) continue;
    try {
      const raw = JSON.parse(script.textContent);
      const candidates = Array.isArray(raw) ? raw : raw["@graph"] || [raw];
      const item = candidates.find((value) => {
        const type = value && value["@type"];
        return (
          ["Product", "CreativeWork", "3DModel"].includes(type) ||
          (Array.isArray(type) &&
            type.some((entry) => ["Product", "CreativeWork", "3DModel"].includes(entry)))
        );
      });
      if (item) return item;
    } catch {}
  }
  return null;
}

function nextDataRecord() {
  const script = document.querySelector("script#__NEXT_DATA__");
  const source = script?.textContent;
  if (!source || source.length > MAX_JSON_SCRIPT_LENGTH) return null;
  let root;
  try {
    root = JSON.parse(source);
  } catch {
    return null;
  }

  const queue = [{ value: root, depth: 0 }];
  let best = null;
  let bestScore = 0;
  for (let index = 0; index < queue.length && index < MAX_NEXT_DATA_NODES; index += 1) {
    const { value, depth } = queue[index];
    if (!value || typeof value !== "object") continue;
    if (Array.isArray(value)) {
      if (depth < MAX_NEXT_DATA_DEPTH) {
        for (const child of value.slice(0, MAX_NEXT_DATA_CHILDREN))
          queue.push({ value: child, depth: depth + 1 });
      }
      continue;
    }

    const name = firstValue(value, ["name", "title", "modelName", "displayName"]);
    if (name) {
      const score =
        3 +
        [
          "id",
          "modelId",
          "model_id",
          "designer",
          "author",
          "creator",
          "owner",
          "license",
          "description",
          "summary",
          "slug",
        ].reduce((total, key) => total + (value[key] == null ? 0 : 1), 0);
      if (score > bestScore) {
        best = value;
        bestScore = score;
      }
    }

    if (depth < MAX_NEXT_DATA_DEPTH) {
      for (const child of Object.values(value).slice(0, MAX_NEXT_DATA_CHILDREN)) {
        if (child && typeof child === "object") queue.push({ value: child, depth: depth + 1 });
      }
    }
  }
  return best;
}

function firstDomText(selectors, max = 512) {
  for (const selector of selectors) {
    const element = document.querySelector(selector);
    const text = boundedText(
      element?.textContent || element?.getAttribute?.("aria-label") || element?.content,
      max,
    );
    if (text) return text;
  }
  return null;
}

function siteDesigner(family) {
  const selectors = {
    makerworld: ['[data-testid="model-author"]', 'a[href*="/u/"]', 'a[href*="/user/"]'],
    printables: ['[data-testid*="author"]', 'a[href*="/@"]', 'a[href*="/user/"]'],
    thangs: ['[data-testid*="designer"]', 'a[href*="/designer/"]'],
    thingiverse: ['[data-testid*="author"]', 'a[href*="/users/"]'],
    cults: ['[data-testid*="designer"]', 'a[href*="/designer/"]'],
    myminifactory: ['[data-testid*="designer"]', 'a[href*="/users/"]', 'a[href*="/user/"]'],
  };
  return firstDomText(selectors[family] || [], 256);
}

function titleCase(value) {
  const cleaned = String(value || "")
    .replace(/^3d-print-/i, "")
    .replace(/-\d{3,}$/, "")
    .replace(/[-_]+/g, " ")
    .trim();
  return cleaned ? cleaned.replace(/\b\p{L}/gu, (letter) => letter.toUpperCase()) : null;
}

function slugTitle(url, family) {
  const path = url.pathname;
  let match;
  if (family === "makerworld" || family === "printables") {
    match = path.match(/\/(?:models|prints|model)\/(\d+)(?:-([^/]+))?\/?$/i);
    if (match)
      return (
        titleCase(match[2]) ||
        `${family === "makerworld" ? "MakerWorld" : "Printables"} model ${match[1]}`
      );
  }
  if (family === "thingiverse") {
    match = path.match(/\/thing(?::|\/)(\d+)\/?$/i);
    if (match) return `Thingiverse model ${match[1]}`;
  }
  if (family === "myminifactory") {
    match = path.match(/\/object\/([^/]+)\/?$/i);
    if (match) return titleCase(match[1]);
  }
  if (family === "thangs" && /\/designer\/[^/]+\/3d-model\//i.test(path)) {
    const designer = path.match(/\/designer\/([^/]+)\/3d-model\//i)?.[1];
    const slug = path.match(/\/3d-model\/([^/]+)\/?$/i)?.[1];
    return titleCase(slug) || titleCase(designer);
  }
  if (family === "thangs" || family === "cults") {
    const segments = path.split("/").filter(Boolean);
    const slug = segments.at(-1);
    return titleCase(slug);
  }
  return null;
}

function documentTitle(family) {
  const brands = {
    makerworld: "MakerWorld",
    printables: "Printables",
    thingiverse: "Thingiverse",
    thangs: "Thangs",
    cults: "Cults3D",
    myminifactory: "MyMiniFactory",
  };
  const title = boundedText(document.title, 512);
  if (!title) return null;
  const brand = brands[family];
  const normalized = brand
    ? title.replace(new RegExp(`\\s*[-|–—]\\s*${brand}$`, "i"), "").trim()
    : title;
  return normalized && normalized.toLowerCase() !== brand?.toLowerCase() ? normalized : null;
}

function extract() {
  if (!supportedPage(location.href)) return null;
  const url = canonicalPage(location.href);
  const parsedUrl = new URL(url);
  const family = siteFamily(parsedUrl.hostname);
  const data = structuredData() || nextDataRecord();
  const title = [
    firstValue(data, ["name", "headline", "title", "modelName", "displayName"]),
    meta("og:title"),
    meta("twitter:title"),
    firstDomText(["h1", '[itemprop="name"]']),
    documentTitle(family),
    slugTitle(parsedUrl, family),
  ]
    .map((value) => boundedText(value, 512))
    .find(Boolean);
  if (!title) return null;

  const partOf = data?.isPartOf;
  const parent = typeof partOf === "string" ? partOf : partOf?.identifier || partOf?.name;
  const author =
    firstValue(data, ["author", "creator", "designer", "owner", "user"]) ||
    meta("author") ||
    meta("article:author") ||
    meta("twitter:creator") ||
    siteDesigner(family) ||
    firstDomText(['[rel="author"]', '[itemprop="author"]'], 256);
  const license =
    firstValue(data, ["license", "licenseName"]) ||
    meta("license") ||
    meta("og:license") ||
    meta("dcterms.rights") ||
    firstDomText(['[itemprop="license"]'], 256);
  const notes =
    firstValue(data, ["description", "summary"]) ||
    meta("description") ||
    meta("og:description") ||
    meta("twitter:description") ||
    firstDomText(['[data-testid="model-description"]', '[data-testid="description"]'], 4000);
  return {
    url,
    title,
    designer: boundedText(author, 256),
    license: boundedText(license, 256),
    notes: boundedText(notes, 4000),
    parentExternalId: boundedText(parent, 256),
  };
}

function publish() {
  chrome.runtime.sendMessage({ type: "page.metadata", page: extract() }).catch(() => {});
}

publish();
let lastHref = location.href;
setInterval(() => {
  if (location.href === lastHref) return;
  lastHref = location.href;
  publish();
}, 2000);
chrome.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.type !== "page.extract") return;
  respond(extract());
});
