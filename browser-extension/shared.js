(function (root) {
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
    for (const [rootHost, family] of HOST_FAMILIES) {
      if (hostname === rootHost || hostname.endsWith(`.${rootHost}`)) return family;
    }
    return null;
  }

  function supportedPage(value) {
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
