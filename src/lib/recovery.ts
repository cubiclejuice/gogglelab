import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { recoveryExportColorways, recoveryExportLibrary, recoveryListLibrary } from "./ipc";
import { COLORWAY_STORAGE_KEY } from "./storage-keys";

export interface RawStringStorage {
  getItem(key: string): string | null;
}

export function readRawColorwayValue(
  storage: RawStringStorage | null = typeof globalThis === "undefined"
    ? null
    : ((globalThis as { localStorage?: RawStringStorage }).localStorage ?? null),
): string | null {
  if (!storage) return null;
  try {
    return storage.getItem(COLORWAY_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function mountRecoveryControl(
  container: HTMLElement,
  addFooterControl: (element: HTMLElement) => void,
): void {
  const launcher = document.createElement("button");
  launcher.type = "button";
  launcher.className = "btn";
  launcher.textContent = "Recover data…";
  launcher.setAttribute("aria-label", "Recover previously saved Library data or color assignments");

  const dialog = document.createElement("dialog");
  dialog.className = "sheet recovery-panel";
  dialog.setAttribute("aria-labelledby", "recovery-title");
  const title = document.createElement("h2");
  title.id = "recovery-title";
  title.textContent = "Recover GoggleLab data";
  const description = document.createElement("p");
  description.textContent =
    "Export a read-only backup of an existing managed Library or the raw saved color assignments from this app profile.";
  const status = document.createElement("p");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  const error = document.createElement("p");
  error.className = "settings-error";
  error.setAttribute("role", "alert");
  const actions = document.createElement("div");
  actions.className = "settings-actions";
  const exportLibrary = document.createElement("button");
  exportLibrary.type = "button";
  exportLibrary.className = "btn primary";
  exportLibrary.textContent = "Export Library backup…";
  const exportColorways = document.createElement("button");
  exportColorways.type = "button";
  exportColorways.className = "btn";
  exportColorways.textContent = "Export saved color assignments…";
  exportLibrary.disabled = true;
  exportColorways.disabled = true;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "btn";
  close.textContent = "Close";
  actions.append(exportLibrary, exportColorways, close);
  dialog.append(title, description, status, error, actions);
  container.appendChild(dialog);

  const updateStatus = async () => {
    error.textContent = "";
    const colorways = readRawColorwayValue();
    exportColorways.disabled = colorways === null;
    const parts: string[] = [];
    try {
      const manifest = await recoveryListLibrary();
      exportLibrary.disabled = false;
      parts.push(`Library found: ${manifest.models.length} cataloged model(s).`);
    } catch {
      exportLibrary.disabled = true;
      parts.push("No supported Library database was found on this device.");
    }
    parts.push(
      colorways === null
        ? "No saved color assignments were found in this app profile."
        : `Saved color assignment data is available (${colorways.length.toLocaleString()} characters).`,
    );
    status.textContent = parts.join(" ");
  };

  launcher.addEventListener("click", () => {
    if (!dialog.open) dialog.showModal();
    void updateStatus();
  });
  exportLibrary.addEventListener("click", async () => {
    error.textContent = "";
    const path = await saveDialog({
      defaultPath: "GoggleLab-Library-Recovery.zip",
      filters: [{ name: "Recovery archive", extensions: ["zip"] }],
    });
    if (!path) return;
    exportLibrary.disabled = true;
    status.textContent = "Reading the Library and verifying managed files…";
    try {
      const manifest = await recoveryExportLibrary([], path);
      status.textContent = `Recovery archive created with ${manifest.models.length} model(s). The Library was not changed.`;
    } catch (reason) {
      error.textContent = `Could not export the Library: ${String(reason)}`;
      await updateStatus();
    } finally {
      exportLibrary.disabled = false;
    }
  });
  exportColorways.addEventListener("click", async () => {
    error.textContent = "";
    const raw = readRawColorwayValue();
    if (raw === null) {
      exportColorways.disabled = true;
      status.textContent = "No saved color assignments were found in this app profile.";
      return;
    }
    const path = await saveDialog({
      defaultPath: "GoggleLab-Saved-Color-Assignments.json",
      filters: [{ name: "JSON recovery file", extensions: ["json"] }],
    });
    if (!path) return;
    exportColorways.disabled = true;
    try {
      const bytes = await recoveryExportColorways(raw, path);
      status.textContent = `Saved ${bytes.toLocaleString()} raw bytes. The original color assignments were not changed.`;
    } catch (reason) {
      error.textContent = `Could not export saved color assignments: ${String(reason)}`;
    } finally {
      exportColorways.disabled = false;
    }
  });
  close.addEventListener("click", () => dialog.close());
  addFooterControl(launcher);
  window.addEventListener(
    "beforeunload",
    () => {
      launcher.remove();
      dialog.remove();
    },
    { once: true },
  );
}
