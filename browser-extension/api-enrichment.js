import { supportedPage } from "./shared-module.js";

export const OPTIONAL_API_ORIGINS = ["https://api.printables.com/*", "https://api.bambulab.com/*"];

const PRINTABLES_ORIGIN = OPTIONAL_API_ORIGINS[0];
const BAMBU_ORIGIN = OPTIONAL_API_ORIGINS[1];
const MAX_RESPONSE_BYTES = 64 * 1024;
const DEFAULT_TIMEOUT_MS = 6000;

function hostFamily(hostname) {
  if (hostname === "printables.com" || hostname.endsWith(".printables.com")) return "printables";
  if (hostname === "makerworld.com" || hostname.endsWith(".makerworld.com")) return "makerworld";
  return null;
}

export function parseEnrichmentTarget(value) {
  try {
    if (!supportedPage(value)) return null;
    const url = new URL(value);
    const site = hostFamily(url.hostname);
    if (site === "makerworld") {
      const id = url.pathname.match(/\/(?:[a-z]{2}(?:-[a-z]{2})?\/)?models\/(\d+)(?:[-/]|$)/i)?.[1];
      return id ? { site, id, origin: url.origin } : null;
    }
    if (site === "printables") {
      const id = url.pathname.match(/\/(?:prints|model)\/(\d+)(?:[-/]|$)/i)?.[1];
      return id ? { site, id, origin: url.origin } : null;
    }
    return null;
  } catch {
    return null;
  }
}

function plainText(value, max) {
  if (typeof value !== "string") return null;
  const text = value
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return text ? text.slice(0, max) : null;
}

function namedText(value, max) {
  if (typeof value === "string") return plainText(value, max);
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  for (const key of ["name", "publicUsername", "handle", "displayName", "username", "nickname"]) {
    const text = plainText(value[key], max);
    if (text) return text;
  }
  return null;
}

function suggestions(fields) {
  const result = {
    title: plainText(fields.title, 512),
    designer: namedText(fields.designer, 256),
    license: namedText(fields.license, 256),
    notes: plainText(fields.notes, 4000),
  };
  return Object.values(result).some(Boolean) ? result : null;
}

async function readBoundedJson(response, maxBytes) {
  if (!response?.ok || !response.body?.getReader) return null;
  const headerLength = response.headers?.get?.("content-length");
  if (headerLength && Number(headerLength) > maxBytes) {
    await response.body.cancel().catch(() => {});
    return null;
  }

  const reader = response.body.getReader();
  const chunks = [];
  let byteLength = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      byteLength += value.byteLength;
      if (byteLength > maxBytes) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock?.();
  }

  const bytes = new Uint8Array(byteLength);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
}

function makerWorldRecord(payload) {
  let record = payload;
  for (let depth = 0; depth < 4 && record && typeof record === "object"; depth += 1) {
    const nested = record.data || record.result || record.design;
    if (!nested || typeof nested !== "object") break;
    record = nested;
  }
  if (!record || typeof record !== "object") return null;
  return suggestions({
    title:
      record.designTitle || record.modelName || record.title || record.name || record.printName,
    designer:
      record.designCreator ||
      record.designer ||
      record.author ||
      record.user ||
      record.creator ||
      record.profile ||
      record.authorName ||
      record.designerName ||
      record.nickname,
    license: record.licenseTitle || record.licenseName || record.license,
    notes: record.summary || record.description || record.desc || record.modelDescription,
  });
}

function printablesRecord(payload) {
  const print = payload?.data?.print;
  if (!print || typeof print !== "object") return null;
  return suggestions({
    title: print.name,
    designer: print.user?.publicUsername || print.user?.handle || print.user?.displayName,
    license: print.license?.name,
    notes: print.description || print.summary,
  });
}

export function createEnrichmentSettings(storage, permissions) {
  async function isEnabled() {
    const { apiEnrichmentEnabled = false } = await storage.get({ apiEnrichmentEnabled: false });
    return apiEnrichmentEnabled === true;
  }

  async function hasPermission(origin) {
    return permissions.contains({ origins: [origin] });
  }

  async function hasAllPermissions() {
    const results = await Promise.all(OPTIONAL_API_ORIGINS.map(hasPermission));
    return results.every(Boolean);
  }

  async function setEnabled(enabled) {
    if (enabled) {
      const granted = await permissions.request({ origins: OPTIONAL_API_ORIGINS });
      if (!granted) {
        try {
          await permissions.remove({ origins: OPTIONAL_API_ORIGINS });
        } catch {}
        return false;
      }
      await storage.set({ apiEnrichmentEnabled: true });
      return true;
    }

    await storage.set({ apiEnrichmentEnabled: false });
    try {
      await permissions.remove({ origins: OPTIONAL_API_ORIGINS });
    } catch {}
    return false;
  }

  async function clearStoredPreference() {
    await storage.set({ apiEnrichmentEnabled: false });
  }

  return { isEnabled, hasPermission, hasAllPermissions, setEnabled, clearStoredPreference };
}

export function bindEnrichmentSetting(checkbox, status, settings) {
  async function load() {
    checkbox.disabled = true;
    try {
      const [enabled, permissionsGranted] = await Promise.all([
        settings.isEnabled(),
        settings.hasAllPermissions(),
      ]);
      checkbox.checked = enabled && permissionsGranted;
      if (enabled && !permissionsGranted) await settings.clearStoredPreference();
      status.textContent = checkbox.checked
        ? "Optional API enrichment is enabled. Requests happen only when you click Enrich."
        : "Off by default. Enabling requests optional API access; Enrich is always a separate click.";
    } catch (error) {
      checkbox.checked = false;
      status.textContent = error?.message || "Could not load API enrichment settings.";
    } finally {
      checkbox.disabled = false;
    }
  }

  checkbox.addEventListener("change", async () => {
    const shouldEnable = checkbox.checked;
    checkbox.disabled = true;
    status.textContent = shouldEnable
      ? "Requesting optional API permissions…"
      : "Turning off API enrichment…";
    try {
      const enabled = await settings.setEnabled(shouldEnable);
      checkbox.checked = enabled;
      status.textContent = enabled
        ? "Optional API enrichment is enabled. Requests happen only when you click Enrich."
        : shouldEnable
          ? "API permissions were not granted; enrichment remains off."
          : "API enrichment is off. No requests will be made.";
    } catch (error) {
      checkbox.checked = false;
      status.textContent = error?.message || "Could not update API enrichment settings.";
    } finally {
      checkbox.disabled = false;
    }
  });

  return { ready: load() };
}

export function createCatalogEnricher({
  isEnabled,
  hasPermission,
  fetchFn = (...args) => fetch(...args),
  timeoutMs = DEFAULT_TIMEOUT_MS,
  maxResponseBytes = MAX_RESPONSE_BYTES,
}) {
  async function canEnrich(value) {
    const target = parseEnrichmentTarget(value);
    if (!target || !(await isEnabled())) return false;
    if (target.site === "printables") return hasPermission(PRINTABLES_ORIGIN);
    return target.site === "makerworld";
  }

  async function getJson(url, init = {}, deadline = Date.now() + timeoutMs) {
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) return null;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), Math.max(1, remainingMs));
    try {
      const response = await fetchFn(url, {
        ...init,
        credentials: "omit",
        redirect: "error",
        signal: controller.signal,
      });
      return await readBoundedJson(response, maxResponseBytes);
    } catch {
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  async function enrich(value) {
    const target = parseEnrichmentTarget(value);
    if (!target || !(await canEnrich(value))) return null;
    const deadline = Date.now() + Math.max(1, timeoutMs);
    if (target.site === "printables") {
      if (!(await hasPermission(PRINTABLES_ORIGIN))) return null;
      const query =
        "query PrintProfile($id: ID!) { print(id: $id) { id name slug summary description user { handle publicUsername } license { id name } } }";
      const data = await getJson(
        "https://api.printables.com/graphql/",
        {
          method: "POST",
          headers: { "Content-Type": "application/json", Accept: "application/json" },
          body: JSON.stringify({
            operationName: "PrintProfile",
            query,
            variables: { id: String(target.id) },
          }),
        },
        deadline,
      );
      return printablesRecord(data);
    }

    const urls = [
      `${target.origin}/api/v1/design-service/design/${target.id}`,
      `https://makerworld.com/api/v1/design-service/design/${target.id}`,
    ];
    for (const url of [...new Set(urls)]) {
      const data = await getJson(url, {}, deadline);
      const result = makerWorldRecord(data);
      if (result) return result;
    }
    if (await hasPermission(BAMBU_ORIGIN)) {
      const data = await getJson(
        `https://api.bambulab.com/v1/design-service/design/${target.id}`,
        {},
        deadline,
      );
      return makerWorldRecord(data);
    }
    return null;
  }

  return { canEnrich, enrich };
}
