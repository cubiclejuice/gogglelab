const MAX_URL_LENGTH = 2048;
const ROUTES = new Map([
  ["makerworld.com", /^\/(?:[a-z]{2}\/)?models\/\d+(?:[-/]|$)/i], ["www.makerworld.com", /^\/(?:[a-z]{2}\/)?models\/\d+(?:[-/]|$)/i],
  ["thangs.com", /^\/(?:designer\/[^/]+\/)?3d-model\/[^/]+/i], ["www.thangs.com", /^\/(?:designer\/[^/]+\/)?3d-model\/[^/]+/i],
  ["printables.com", /^\/model\/\d+(?:[-/]|$)/i], ["www.printables.com", /^\/model\/\d+(?:[-/]|$)/i],
  ["thingiverse.com", /^\/thing:\d+(?:\/|$)/i], ["www.thingiverse.com", /^\/thing:\d+(?:\/|$)/i],
  ["cults3d.com", /^\/(?:[a-z]{2}\/)?3d-model\/[^/]+/i], ["www.cults3d.com", /^\/(?:[a-z]{2}\/)?3d-model\/[^/]+/i],
  ["myminifactory.com", /^\/object\/3d-print-[^/]+/i], ["www.myminifactory.com", /^\/object\/3d-print-[^/]+/i],
]);
export function supportedPage(value) { try { if (typeof value !== "string" || value.length > MAX_URL_LENGTH) return false; const url = new URL(value); const route = ROUTES.get(url.hostname); return url.protocol === "https:" && !url.username && !url.password && !url.port && !!route?.test(url.pathname); } catch { return false; } }
export function canonicalPage(value) { const url = new URL(value); url.hash = ""; url.search = ""; return url.toString(); }
export function newId() { return crypto.randomUUID(); }
export async function stableId(value) { const bytes = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value))); return [...bytes.slice(0, 16)].map((byte) => byte.toString(16).padStart(2, "0")).join(""); }
export function basename(path) { return String(path || "").replaceAll("\\", "/").split("/").pop() || "model"; }
