import { newId } from "./shared-module.js";

function dataUrl(payload) {
  const bytes = new TextEncoder().encode(JSON.stringify(payload, null, 2));
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return `data:application/json;base64,${btoa(binary)}`;
}

function inboxId(value) {
  return typeof value === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(value) ? value : newId();
}

export function createInboxWriter(api) {
  let offscreenReady = null;
  async function ensureOffscreen() {
    if (offscreenReady) return offscreenReady;
    offscreenReady = (async () => {
      if (await api.offscreen.hasDocument()) return;
      await api.offscreen.createDocument({
        url: "offscreen.html",
        reasons: ["BLOBS"],
        justification: "Write a bounded GoggleLab capture into the folder explicitly selected by the user."
      });
    })();
    try { await offscreenReady; } finally { offscreenReady = null; }
  }
  return async (payload) => {
    try {
      await ensureOffscreen();
      const folder = await api.runtime.sendMessage({ type: "inbox.writeFolder", payload });
      if (folder?.ok) return folder;
    } catch {}
    const filename = `GoggleLabInbox/${inboxId(payload.itemId)}.gogglelab-capture.json`;
    const downloadId = await api.downloads.download({
      url: dataUrl(payload), filename, conflictAction: "overwrite", saveAs: false
    });
    return { ok: true, via: "downloads", downloadId, filename };
  };
}

export const writeInbox = createInboxWriter(globalThis.chrome);
