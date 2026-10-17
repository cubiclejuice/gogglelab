const MAX_URL_LENGTH = 2048;
const HOST_FAMILIES = new Map([
  ["makerworld.com", "makerworld"],
  ["thangs.com", "thangs"],
  ["printables.com", "printables"],
  ["thingiverse.com", "thingiverse"],
  ["cults3d.com", "cults"],
  ["myminifactory.com", "myminifactory"],
]);
const ROUTES = {
  makerworld: /^\/(?:[a-z]{2}(?:-[a-z]{2})?\/)?models\/\d+(?:-[^/]+)?\/?$/i,
  printables: /^\/(?:prints|model)\/\d+(?:-[^/]+)?\/?$/i,
  thingiverse: /^\/thing(?::|\/)\d+\/?$/i,
  thangs: /^\/(?:model|design|3d-model)\/[^/]+\/?$|^\/designer\/[^/]+\/3d-model\/[^/]+\/?$/i,
  cults: /^\/(?:[a-z]{2}(?:-[a-z]{2})?\/)?(?:3d-model|design)\/[^/]+(?:\/[^/]+)*\/?$/i,
  myminifactory: /^\/object\/[^/]+\/?$/i,
};

function familyForHostname(hostname) {
  for (const [root, family] of HOST_FAMILIES) {
    if (hostname === root || hostname.endsWith(`.${root}`)) return family;
  }
  return null;
}

function secureHttpsUrl(value) {
  if (
    typeof value !== "string" ||
    value.length > MAX_URL_LENGTH ||
    /[\u0000-\u001f\u007f]/.test(value)
  ) {
    return null;
  }
  const url = new URL(value);
  const hostname = url.hostname.toLowerCase();
  const isIpAddress = hostname.startsWith("[") || /^\d{1,3}(?:\.\d{1,3}){3}$/.test(hostname);
  const isLocalName =
    hostname === "localhost" ||
    hostname === "home.arpa" ||
    [".localhost", ".local", ".internal", ".home.arpa", ".test", ".invalid", ".example"].some(
      (suffix) => hostname.endsWith(suffix),
    );
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    url.port ||
    !hostname.includes(".") ||
    hostname.endsWith(".") ||
    isIpAddress ||
    isLocalName
  ) {
    return null;
  }
  return url;
}

export function supportedCatalogUrl(value) {
  try {
    if (typeof value !== "string" || value.length > MAX_URL_LENGTH) return false;
    const url = secureHttpsUrl(value);
    return !!url && !!familyForHostname(url.hostname);
  } catch {
    return false;
  }
}

export function safeCaptureSourceUrl(value) {
  try {
    const url = secureHttpsUrl(value);
    return url ? canonicalPage(url.toString()) : null;
  } catch {
    return null;
  }
}

export function supportedPage(value) {
  try {
    if (typeof value !== "string" || value.length > MAX_URL_LENGTH) return false;
    const url = new URL(value);
    const family = familyForHostname(url.hostname);
    return (
      url.protocol === "https:" &&
      !url.username &&
      !url.password &&
      !url.port &&
      !!family &&
      ROUTES[family].test(url.pathname)
    );
  } catch {
    return false;
  }
}

export function canonicalPage(value) {
  const url = new URL(value);
  url.hash = "";
  url.search = "";
  return url.toString();
}

export function reviewPage(source, review = {}) {
  if (!source || typeof source !== "object" || !supportedPage(source.url)) {
    throw new Error("Capture source URL must be a supported HTTPS model page");
  }
  if (!review || typeof review !== "object" || Array.isArray(review)) {
    throw new Error("Capture review has an invalid shape");
  }

  const sourceUrl = review.sourceUrl === undefined ? source.url : review.sourceUrl;
  if (!supportedPage(sourceUrl)) {
    throw new Error("Capture source URL must be a supported HTTPS model page");
  }

  const cleanText = (value, fallback, max, field) => {
    const selected = value === undefined ? fallback : value;
    if (selected === null) return null;
    if (typeof selected !== "string") throw new Error(`Capture ${field} has an invalid field type`);
    const text = selected.trim();
    return text ? text.slice(0, max) : null;
  };

  const title = cleanText(review.title, source.title, 512, "title");
  if (!title) throw new Error("Capture title is required");
  return {
    url: canonicalPage(sourceUrl),
    title,
    designer: cleanText(review.designer, source.designer, 256, "designer"),
    license: cleanText(review.license, source.license, 256, "license"),
    notes: cleanText(review.notes, source.notes, 4000, "notes"),
    parentExternalId: cleanText(undefined, source.parentExternalId, 256, "parent relation"),
  };
}

export function newId() {
  return crypto.randomUUID();
}

export async function stableId(value) {
  const bytes = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)),
  );
  return [...bytes.slice(0, 16)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function basename(path) {
  return (
    String(path || "")
      .replaceAll("\\", "/")
      .split("/")
      .pop() || "model"
  );
}
