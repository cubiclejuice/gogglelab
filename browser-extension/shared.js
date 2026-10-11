(function (root) {
  const MAX_URL_LENGTH = 2048;
  const ROUTES = new Map([
    ["makerworld.com", /^\/(?:[a-z]{2}\/)?models\/\d+(?:[-/]|$)/i], ["www.makerworld.com", /^\/(?:[a-z]{2}\/)?models\/\d+(?:[-/]|$)/i],
    ["thangs.com", /^\/(?:designer\/[^/]+\/)?3d-model\/[^/]+/i], ["www.thangs.com", /^\/(?:designer\/[^/]+\/)?3d-model\/[^/]+/i],
    ["printables.com", /^\/model\/\d+(?:[-/]|$)/i], ["www.printables.com", /^\/model\/\d+(?:[-/]|$)/i],
    ["thingiverse.com", /^\/thing:\d+(?:\/|$)/i], ["www.thingiverse.com", /^\/thing:\d+(?:\/|$)/i],
    ["cults3d.com", /^\/(?:[a-z]{2}\/)?3d-model\/[^/]+/i], ["www.cults3d.com", /^\/(?:[a-z]{2}\/)?3d-model\/[^/]+/i],
    ["myminifactory.com", /^\/object\/3d-print-[^/]+/i], ["www.myminifactory.com", /^\/object\/3d-print-[^/]+/i],
  ]);
  function supportedPage(value) {
    try {
      if (typeof value !== "string" || value.length > MAX_URL_LENGTH) return false;
      const url = new URL(value);
      const route = ROUTES.get(url.hostname);
      return url.protocol === "https:" && !url.username && !url.password && !url.port && !!route?.test(url.pathname);
    } catch { return false; }
  }
  function canonicalPage(value) {
    const url = new URL(value);
    url.hash = "";
    url.search = "";
    return url.toString();
  }
  function boundedText(value, max = 4000) {
    if (typeof value !== "string") return null;
    const text = value.trim();
    return text ? text.slice(0, max) : null;
  }
  root.GoggleCapture = { supportedPage, canonicalPage, boundedText };
})(globalThis);
