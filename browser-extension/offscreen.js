async function handle() {
  const db = await new Promise((resolve, reject) => {
    const request = indexedDB.open("gogglelab-capture", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("handles");
    request.onerror = () => reject(request.error);
    request.onsuccess = () => resolve(request.result);
  });
  return await new Promise((resolve, reject) => {
    const request = db.transaction("handles").objectStore("handles").get("inbox");
    request.onsuccess = () => resolve(request.result || null);
    request.onerror = () => reject(request.error);
  });
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (
    message?.type !== "inbox.writeFolder" ||
    sender?.id !== chrome.runtime.id ||
    sender.tab
  ) return;
  void (async () => {
    const root = await handle();
    if (!root || await root.queryPermission({ mode: "readwrite" }) !== "granted") return { ok: false, reason: "permission" };
    const inbox = await root.getDirectoryHandle("GoggleLabInbox", { create: true });
    const file = await inbox.getFileHandle(`${message.payload.itemId}.gogglelab-capture.json`, { create: true });
    const writable = await file.createWritable();
    await writable.write(JSON.stringify(message.payload, null, 2));
    await writable.close();
    return { ok: true, via: "folder", fileName: file.name };
  })().then(respond).catch((error) => respond({ ok: false, reason: error.message }));
  return true;
});
