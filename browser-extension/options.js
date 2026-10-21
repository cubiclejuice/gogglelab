import { bindEnrichmentSetting, createEnrichmentSettings } from "./api-enrichment.js";

const status = document.querySelector("#status");
const queue = document.querySelector("#queue");
const apiEnrichmentStatus = document.querySelector("#api-enrichment-status");
const apiEnrichment = createEnrichmentSettings(chrome.storage.local, chrome.permissions);
const apiEnrichmentSetting = bindEnrichmentSetting(
  document.querySelector("#api-enrichment-enabled"),
  apiEnrichmentStatus,
  apiEnrichment,
);
void apiEnrichmentSetting.ready;

async function database() {
  return await new Promise((resolve, reject) => {
    const request = indexedDB.open("gogglelab-capture", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("handles");
    request.onerror = () => reject(request.error);
    request.onsuccess = () => resolve(request.result);
  });
}

async function storeHandle(handle) {
  const db = await database();
  const tx = db.transaction("handles", "readwrite");
  tx.objectStore("handles").put(handle, "inbox");
  await new Promise((resolve, reject) => {
    tx.oncomplete = resolve;
    tx.onerror = () => reject(tx.error);
  });
}

document.querySelector("#choose").addEventListener("click", async () => {
  try {
    const handle = await showDirectoryPicker({ mode: "readwrite", id: "gogglelab-inbox" });
    if ((await handle.requestPermission({ mode: "readwrite" })) !== "granted")
      throw new Error("Write permission was not granted");
    await storeHandle(handle);
    await chrome.storage.local.set({ inboxFolderName: handle.name });
    status.textContent = `Using ${handle.name}.`;
  } catch (error) {
    if (error.name !== "AbortError") status.textContent = error.message;
  }
});
document.querySelector("#clear").addEventListener("click", async () => {
  const db = await database();
  db.transaction("handles", "readwrite").objectStore("handles").delete("inbox");
  await chrome.storage.local.remove("inboxFolderName");
  status.textContent = "Using Downloads/GoggleLabInbox.";
});

chrome.runtime
  .sendMessage({ type: "queue.list" })
  .then(({ captureQueue = [], captureFailures = [] } = {}) => {
    for (const item of captureQueue) {
      const li = document.createElement("li");
      li.textContent = `${item.page?.title || "Untitled"} · ${item.downloads?.length || 0} downloads`;
      const retry = document.createElement("button");
      retry.textContent = "Write inbox file again";
      retry.addEventListener("click", async () => {
        retry.disabled = true;
        const result = await chrome.runtime.sendMessage({
          type: "queue.retry",
          itemId: item.itemId,
        });
        status.textContent = result?.ok ? "Inbox file written." : result?.error || "Retry failed.";
        retry.disabled = false;
      });
      li.append(" ", retry);
      queue.append(li);
    }
    if (captureFailures.length)
      status.textContent = `${captureFailures.length} recent automatic capture failures are retained for diagnosis and retry.`;
  });
