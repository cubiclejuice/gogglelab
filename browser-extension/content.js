const { boundedText, canonicalPage, supportedPage } = globalThis.GoggleCapture;

function meta(name) {
  return document.querySelector(`meta[property="${name}"],meta[name="${name}"]`)?.content ?? null;
}

function structuredData() {
  for (const script of document.querySelectorAll('script[type="application/ld+json"]')) {
    if (!script.textContent || script.textContent.length > 64 * 1024) continue;
    try {
      const raw = JSON.parse(script.textContent);
      const candidates = Array.isArray(raw) ? raw : raw["@graph"] || [raw];
      const item = candidates.find((value) => {
        const type = value && value["@type"];
        return ["Product", "CreativeWork", "3DModel"].includes(type) ||
          (Array.isArray(type) && type.some((entry) => ["Product", "CreativeWork"].includes(entry)));
      });
      if (item) return item;
    } catch {}
  }
  return null;
}

function extract() {
  if (!supportedPage(location.href)) return null;
  const url = canonicalPage(location.href);
  const data = structuredData();
  const author = typeof data?.author === "string" ? data.author : data?.author?.name;
  const parent = typeof data?.isPartOf === "string"
    ? data.isPartOf
    : data?.isPartOf?.identifier || data?.isPartOf?.name;
  const title = boundedText(data?.name || meta("og:title") || document.querySelector("h1")?.textContent || document.title, 512);
  if (!title) return null;
  return {
    url,
    title,
    designer: boundedText(author || meta("author") || document.querySelector('[rel="author"]')?.textContent, 256),
    license: boundedText(data?.license || document.querySelector('[itemprop="license"]')?.textContent, 256),
    notes: boundedText(data?.description || meta("description") || meta("og:description"), 4000),
    parentExternalId: boundedText(parent, 256)
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
