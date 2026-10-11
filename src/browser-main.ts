import {
  HttpLibraryClient,
  formatWireInteger,
  type BrowserModel,
  type CaptureEnvelope,
  type HostCapabilities,
} from "./lib/library-client";
import { isBrowserDialogCurrent } from "./lib/browser-dialog";
import "./styles.css";

const root = document.querySelector<HTMLElement>("#browser-app")!;
const client = new HttpLibraryClient();
let capabilities: HostCapabilities | null = null;
let captureRoutesAvailable = false;
let galleryRoutesAvailable = false;
let downloadRoutesAvailable = false;
let metadataRoutesAvailable = false;
let historyRoutesAvailable = false;
let filamentRoutesAvailable = false;
let eventController: AbortController | null = null;
let modelController: AbortController | null = null;
let modelLoadRevision = 0;

const header = document.createElement("header");
header.className = "browser-header";
const title = document.createElement("h1");
title.textContent = "GoggleLab Library";
const hostStatus = document.createElement("span");
hostStatus.textContent = "Disconnected";
header.append(title, hostStatus);

const login = document.createElement("form");
login.className = "browser-login";
const token = document.createElement("input");
token.type = "password";
token.autocomplete = "off";
token.placeholder = "Server access key";
token.setAttribute("aria-label", "Server access key");
token.setAttribute("aria-describedby", "browser-access-key-help");
const tokenHelp = document.createElement("small");
tokenHelp.id = "browser-access-key-help";
tokenHelp.textContent = "Kept only while connected.";
const connect = document.createElement("button");
connect.type = "submit";
connect.className = "btn primary";
connect.textContent = "Connect";
const disconnect = document.createElement("button");
disconnect.type = "button";
disconnect.className = "btn";
disconnect.textContent = "Disconnect";
disconnect.disabled = true;
login.append(token, tokenHelp, connect, disconnect);

if (client.endpointSecurityError) {
  token.disabled = true;
  connect.disabled = true;
  token.placeholder = "HTTPS required";
}

const toolbar = document.createElement("div");
toolbar.className = "browser-toolbar";
const search = document.createElement("input");
search.type = "search";
search.placeholder = "Search the Library";
search.setAttribute("aria-label", "Search the remote Library");
const refresh = document.createElement("button");
refresh.type = "button";
refresh.className = "btn";
refresh.textContent = "Refresh";
refresh.disabled = true;
toolbar.append(search, refresh);

const status = document.createElement("div");
status.className = "library-status";
status.setAttribute("role", "status");
const models = document.createElement("main");
models.className = "browser-model-grid";
const capture = document.createElement("section");
capture.className = "browser-capture-import";
capture.hidden = true;
const captureTitle = document.createElement("h2");
captureTitle.textContent = "Import capture queue file";
const captureFile = document.createElement("input");
captureFile.type = "file";
captureFile.accept = ".json,.gogglelab-capture.json,application/json";
captureFile.setAttribute("aria-label", "Capture queue JSON file");
const modelFiles = document.createElement("input");
modelFiles.type = "file";
modelFiles.multiple = true;
modelFiles.accept = ".stl,.3mf";
modelFiles.setAttribute("aria-label", "Downloaded STL or 3MF files referenced by the capture");
const importCapture = document.createElement("button");
importCapture.type = "button";
importCapture.className = "btn";
importCapture.textContent = "Import capture";
importCapture.disabled = true;
capture.append(captureTitle, captureFile, modelFiles, importCapture);
root.append(header, login, toolbar, capture, status, models);
if (client.endpointSecurityError) {
  status.setAttribute("role", "alert");
  status.textContent = client.endpointSecurityError.message;
}

function renderModels(items: BrowserModel[]): void {
  models.replaceChildren();
  for (const model of items) {
    const card = document.createElement("article");
    card.className = "browser-model-card";
    card.dataset.modelId = model.id;
    const name = document.createElement("h2");
    name.textContent = model.displayName;
    const meta = document.createElement("p");
    const availability = model.availability;
    meta.textContent = `${model.format ?? "Bookmark"} · ${formatWireInteger(model.bytes)} bytes · ${availability}`;
    const tags = document.createElement("p");
    tags.textContent = model.tags.join(", ") || "No tags";
    const actions = document.createElement("div");
    actions.className = "browser-card-actions";
    if (metadataRoutesAvailable) {
      actions.append(actionButton("Edit details", () => void showMetadataEditor(model)));
    }
    if (historyRoutesAvailable) {
      const history = actionButton("Print history", () => void showHistory(model));
      history.dataset.browserAction = "history";
      actions.append(history);
    }
    if (filamentRoutesAvailable)
      actions.append(actionButton("Assign filaments", () => void showFilaments(model)));
    if (galleryRoutesAvailable && availability !== "bookmark") {
      actions.append(actionButton("Preview gallery", () => void showGallery(model)));
    }
    if (
      downloadRoutesAvailable &&
      capabilities?.features.downloadSource &&
      availability === "available"
    ) {
      actions.append(actionButton("Download model", () => void downloadModel(model)));
    }
    if (availability === "bookmark") {
      const bookmark = document.createElement("span");
      bookmark.textContent = "Page-only bookmark — upload a downloaded model to enable preview.";
      card.append(name, meta, tags, bookmark, actions);
    } else card.append(name, meta, tags, actions);
    models.append(card);
  }
  if (!items.length) {
    const empty = document.createElement("p");
    empty.textContent = "No models match this query.";
    models.append(empty);
  }
}

async function showFilaments(model: BrowserModel): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-metadata-dialog";
  const title = document.createElement("h2");
  title.textContent = `Filaments for ${model.displayName}`;
  const form = document.createElement("form");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  dialog.append(title, form, message);
  document.body.append(dialog);
  dialog.showModal();
  try {
    const filaments = await client.listFilaments(true);
    for (const filament of filaments) {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "checkbox";
      input.value = filament.id;
      input.checked = model.filamentIds.includes(filament.id);
      label.append(
        input,
        document.createTextNode(
          `${filament.brand} ${filament.name || filament.material}${filament.sourceMissing || filament.archived ? " · unavailable" : ""}`,
        ),
      );
      form.append(label);
    }
    const save = actionButton("Save", () => {});
    save.type = "submit";
    const cancel = actionButton("Cancel", () => dialog.close());
    form.append(save, cancel);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const ids = [
        ...form.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked'),
      ].map((input) => input.value);
      if (ids.length > 5) {
        message.textContent = "Choose no more than five filaments.";
        return;
      }
      save.disabled = true;
      void client
        .assignFilaments([
          {
            target: { modelId: model.id, expectedVersion: model.metadataVersion },
            filamentIds: ids,
          },
        ])
        .then(async () => {
          dialog.close();
          await loadModels();
        })
        .catch((error) => {
          message.textContent = String(error?.message ?? error);
        })
        .finally(() => {
          save.disabled = false;
        });
    });
  } catch (error) {
    message.textContent = String((error as { message?: unknown }).message ?? error);
  }
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showHistory(model: BrowserModel): Promise<void> {
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const dialog = document.createElement("dialog");
  dialog.className = "browser-metadata-dialog";
  const title = document.createElement("h2");
  title.textContent = `Print history for ${model.displayName}`;
  const list = document.createElement("div");
  const form = document.createElement("form");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const date = document.createElement("input");
  date.type = "date";
  date.setAttribute("aria-label", "Print date");
  const now = new Date();
  date.value = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  const quantity = document.createElement("input");
  quantity.type = "number";
  quantity.setAttribute("aria-label", "Print quantity");
  quantity.min = "1";
  quantity.value = "1";
  const notes = document.createElement("textarea");
  notes.setAttribute("aria-label", "Print notes");
  const outcome = document.createElement("select");
  outcome.setAttribute("aria-label", "Print outcome");
  for (const value of ["printed", "failed"]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    outcome.append(option);
  }
  const save = actionButton("Log print", () => {});
  save.type = "submit";
  const close = actionButton("Close", () => dialog.close());
  form.append(date, quantity, outcome, notes, save);
  dialog.append(title, list, form, message, close);
  document.body.append(dialog);
  dialog.showModal();
  date.focus();
  let revision = 0;
  const render = async () => {
    const request = ++revision;
    try {
      const attempts = await client.listHistory(model.id);
      if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
      list.replaceChildren();
      for (const attempt of attempts) {
        const row = document.createElement("p");
        row.textContent = `${attempt.printedAt.slice(0, 10)} · ${attempt.quantity} · ${attempt.outcome}${attempt.notes ? ` · ${attempt.notes}` : ""}`;
        row.append(
          actionButton(
            "Delete",
            () =>
              void client
                .deleteHistory(attempt.id)
                .then(() => {
                  if (dialog.open && dialog.isConnected) return render();
                })
                .catch((error) => {
                  if (dialog.open && dialog.isConnected)
                    message.textContent = String(error?.message ?? error);
                }),
          ),
        );
        list.append(row);
      }
    } catch (error) {
      if (isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected))
        message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    save.disabled = true;
    const request = revision;
    void client
      .createHistory(model.id, {
        printedAt: `${date.value}T12:00:00.000Z`,
        quantity: Number(quantity.value),
        notes: notes.value,
        filamentIds: model.filamentIds.slice(0, 5),
        outcome: outcome.value as "printed" | "failed",
      })
      .then(async () => {
        await loadModels();
        if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
        notes.value = "";
        await render();
      })
      .catch((error) => {
        if (isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected))
          message.textContent = String(error?.message ?? error);
      })
      .finally(() => {
        if (dialog.open && dialog.isConnected) save.disabled = false;
      });
  });
  void render();
  dialog.addEventListener(
    "close",
    () => {
      revision += 1;
      dialog.remove();
      if (invoker?.isConnected) invoker.focus();
      else {
        const card = [...models.querySelectorAll<HTMLElement>(".browser-model-card")].find(
          (candidate) => candidate.dataset.modelId === model.id,
        );
        (card?.querySelector<HTMLElement>('[data-browser-action="history"]') ?? search).focus();
      }
    },
    { once: true },
  );
}

async function showMetadataEditor(model: BrowserModel): Promise<void> {
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const dialog = document.createElement("dialog");
  dialog.className = "browser-metadata-dialog";
  const title = document.createElement("h2");
  title.textContent = `Edit ${model.displayName}`;
  const form = document.createElement("form");
  form.method = "dialog";
  const field = (
    label: string,
    value: string,
    multiline = false,
  ): HTMLInputElement | HTMLTextAreaElement => {
    const wrapper = document.createElement("label");
    wrapper.textContent = label;
    const control = multiline
      ? document.createElement("textarea")
      : document.createElement("input");
    control.value = value;
    wrapper.append(control);
    form.append(wrapper);
    return control;
  };
  const designer = field("Designer", model.designer ?? "");
  const sourceUrl = field("Source URL", model.sourceUrl ?? "");
  const license = field("License", model.modelLicense ?? "");
  const notes = field("Notes", model.notes ?? "", true);
  const statusSelect = document.createElement("select");
  statusSelect.setAttribute("aria-label", "Print status");
  for (const value of ["unprinted", "want", "queued", "printing", "printed", "failed"]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    statusSelect.append(option);
  }
  statusSelect.value = model.printStatus;
  form.append(statusSelect);
  const tags = document.createElement("fieldset");
  const tagsLegend = document.createElement("legend");
  tagsLegend.textContent = "Tags";
  tags.append(tagsLegend);
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const save = actionButton("Save", () => {});
  save.type = "submit";
  const cancel = actionButton("Cancel", () => dialog.close());
  form.append(tags, message, save, cancel);
  dialog.append(title, form);
  document.body.append(dialog);
  try {
    const knownTags = await client.listTags();
    for (const tag of knownTags) {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = tag.id;
      checkbox.checked = model.tags.includes(tag.name);
      label.append(checkbox, document.createTextNode(tag.name));
      tags.append(label);
    }
  } catch (error) {
    message.textContent = String((error as { message?: unknown }).message ?? error);
  }
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    save.disabled = true;
    const tagIds = [
      ...tags.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked'),
    ].map((input) => input.value);
    void client
      .updateMetadata(model.id, model.metadataVersion, {
        designer: designer.value,
        sourceUrl: sourceUrl.value,
        modelLicense: license.value,
        notes: notes.value,
        printStatus: statusSelect.value as
          | "unprinted"
          | "want"
          | "queued"
          | "printing"
          | "printed"
          | "failed",
        tagIds,
        tagMode: "replace",
      })
      .then(async () => {
        dialog.close();
        await loadModels();
      })
      .catch((error) => {
        message.textContent = String((error as { message?: unknown }).message ?? error);
      })
      .finally(() => {
        save.disabled = false;
      });
  });
  dialog.addEventListener(
    "close",
    () => {
      dialog.remove();
      if (invoker?.isConnected) invoker.focus();
    },
    { once: true },
  );
  dialog.showModal();
  designer.focus();
}

function actionButton(label: string, action: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn";
  button.textContent = label;
  button.addEventListener("click", action);
  return button;
}

async function downloadModel(model: BrowserModel): Promise<void> {
  status.textContent = `Preparing ${model.displayName}…`;
  try {
    const lease = await client.issueModelDownload(model.id, model.contentHash);
    const blob = await client.readContent(lease.id, lease.mime);
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = lease.fileName;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
    status.textContent = `${model.displayName} downloaded.`;
  } catch (error) {
    status.textContent = String((error as { message?: unknown }).message ?? error);
  }
}

async function showGallery(model: BrowserModel): Promise<void> {
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const dialog = document.createElement("dialog");
  dialog.className = "browser-gallery-dialog";
  dialog.setAttribute("aria-label", `Preview gallery for ${model.displayName}`);
  const heading = document.createElement("h2");
  heading.textContent = `${model.displayName} previews`;
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const list = document.createElement("div");
  list.className = "browser-gallery-list";
  const upload = document.createElement("input");
  upload.type = "file";
  upload.accept = "image/png,image/jpeg";
  upload.setAttribute("aria-label", "Add PNG or JPEG preview");
  const close = actionButton("Close", () => dialog.close());
  dialog.append(heading, message, list, upload, close);
  document.body.append(dialog);
  const objectUrls = new Set<string>();
  let version = model.metadataVersion;
  let revision = 0;
  const render = async (): Promise<void> => {
    const requestRevision = ++revision;
    message.textContent = "Loading previews…";
    list.replaceChildren();
    try {
      const entries = await client.listThumbnails(model.id);
      if (requestRevision !== revision || !dialog.open) return;
      for (const entry of entries) {
        const row = document.createElement("article");
        const label = document.createElement("strong");
        label.textContent = `Preview ${entry.ordinal + 1}${entry.active ? " · active" : ""}${entry.custom ? " · custom" : ""}`;
        const image = document.createElement("img");
        image.alt = label.textContent;
        image.loading = "lazy";
        image.addEventListener(
          "error",
          () => {
            image.remove();
            const unavailable = document.createElement("span");
            unavailable.textContent = "Preview unavailable: the image could not be displayed.";
            row.append(unavailable);
          },
          { once: true },
        );
        row.append(label, image);
        try {
          const lease = await client.issueThumbnailMedia(model.id, entry.thumbnailId);
          const blob = await client.readContent(lease.id, lease.mime);
          if (requestRevision !== revision || !dialog.open) return;
          const url = URL.createObjectURL(blob);
          objectUrls.add(url);
          image.src = url;
        } catch (error) {
          if (requestRevision !== revision || !dialog.open) return;
          image.remove();
          const unavailable = document.createElement("span");
          unavailable.textContent = `Preview unavailable: ${String((error as { message?: unknown }).message ?? error)}`;
          row.append(unavailable);
        }
        if (!entry.active) {
          row.append(
            actionButton(
              "Set active",
              () =>
                void mutate(() => client.setActiveThumbnail(model.id, version, entry.thumbnailId)),
            ),
          );
        }
        if (entries.length > 1) {
          row.append(
            actionButton(
              "Delete",
              () => void mutate(() => client.deleteThumbnail(model.id, version, entry.thumbnailId)),
            ),
          );
        }
        list.append(row);
      }
      message.textContent = entries.length ? `${entries.length} previews` : "No previews yet.";
    } catch (error) {
      if (requestRevision === revision) {
        message.textContent = String((error as { message?: unknown }).message ?? error);
      }
    }
  };
  const mutate = async (operation: () => Promise<string>): Promise<void> => {
    try {
      version = await operation();
      await render();
      await loadModels();
    } catch (error) {
      message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  upload.addEventListener("change", () => {
    const file = upload.files?.[0];
    if (!file) return;
    if (capabilities && file.size > capabilities.limits.thumbnailUploadBytes) {
      message.textContent = "That preview exceeds the server upload limit.";
      return;
    }
    upload.disabled = true;
    void mutate(() => client.addThumbnail(model.id, version, file)).finally(() => {
      upload.value = "";
      upload.disabled = false;
    });
  });
  dialog.addEventListener(
    "close",
    () => {
      revision += 1;
      for (const url of objectUrls) URL.revokeObjectURL(url);
      dialog.remove();
      if (invoker?.isConnected) invoker.focus();
    },
    { once: true },
  );
  dialog.showModal();
  close.focus();
  await render();
}

async function loadModels(): Promise<void> {
  const revision = ++modelLoadRevision;
  modelController?.abort();
  modelController = new AbortController();
  status.textContent = "Loading models…";
  try {
    const page = await client.listModels({
      limit: Math.min(capabilities?.limits.pageSizeMax ?? 50, 100),
      query: search.value.trim() || undefined,
      signal: modelController.signal,
    });
    if (revision !== modelLoadRevision) return;
    renderModels(page.items);
    status.textContent = `${page.items.length} models`;
    status.title = `Catalog revision ${page.snapshotRevision}`;
  } catch (error) {
    if (revision !== modelLoadRevision || modelController.signal.aborted) return;
    status.textContent = String((error as { message?: unknown }).message ?? error);
  }
}

async function watchEvents(controller: AbortController): Promise<void> {
  let lastEventId: string | undefined;
  while (!controller.signal.aborted) {
    try {
      for await (const event of client.events(controller.signal, lastEventId)) {
        lastEventId = event.sequence;
        if (event.type === "catalog.changed" || event.type === "library.reset") await loadModels();
        if (event.type === "server.shutdown") hostStatus.textContent = "Server shutting down";
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      status.textContent = `Live updates paused; polling while reconnecting: ${String(error)}`;
      await loadModels();
    }
    await new Promise((resolve) => window.setTimeout(resolve, 5000));
  }
}

login.addEventListener("submit", (event) => {
  event.preventDefault();
  client.setToken(token.value);
  token.value = "";
  connect.disabled = true;
  void (async () => {
    try {
      const [hostCapabilities, openApi] = await Promise.all([
        client.capabilities(),
        client.openApi(),
      ]);
      capabilities = hostCapabilities;
      captureRoutesAvailable =
        "/api/v1/captures/uploads" in openApi.paths && "/api/v1/captures/import" in openApi.paths;
      galleryRoutesAvailable =
        "/api/v1/models/{modelId}/thumbnails" in openApi.paths &&
        "/api/v1/models/{modelId}/thumbnails/{thumbnailId}/media-lease" in openApi.paths;
      downloadRoutesAvailable =
        "/api/v1/models/{modelId}/download-lease" in openApi.paths &&
        "/api/v1/content/{leaseId}" in openApi.paths;
      metadataRoutesAvailable =
        "/api/v1/models/{modelId}/metadata" in openApi.paths && "/api/v1/tags" in openApi.paths;
      historyRoutesAvailable = "/api/v1/models/{modelId}/history" in openApi.paths;
      filamentRoutesAvailable =
        "/api/v1/models/filaments/bulk" in openApi.paths && "/api/v1/filaments" in openApi.paths;
      capture.hidden = !(capabilities.features.captureInbox && captureRoutesAvailable);
      hostStatus.textContent = "Connected to server";
      hostStatus.title = `${capabilities.hostMode} host · library epoch ${capabilities.libraryEpoch}`;
      disconnect.disabled = false;
      refresh.disabled = false;
      await loadModels();
      eventController?.abort();
      eventController = new AbortController();
      void watchEvents(eventController);
    } catch (error) {
      client.clearToken();
      status.textContent = String((error as { message?: unknown }).message ?? error);
    } finally {
      connect.disabled = false;
    }
  })();
});

disconnect.addEventListener("click", () => {
  eventController?.abort();
  modelController?.abort();
  modelLoadRevision += 1;
  eventController = null;
  client.clearToken();
  capabilities = null;
  captureRoutesAvailable = false;
  galleryRoutesAvailable = false;
  downloadRoutesAvailable = false;
  metadataRoutesAvailable = false;
  historyRoutesAvailable = false;
  filamentRoutesAvailable = false;
  capture.hidden = true;
  hostStatus.textContent = "Disconnected";
  hostStatus.removeAttribute("title");
  disconnect.disabled = true;
  refresh.disabled = true;
  models.replaceChildren();
  status.textContent = "Token cleared from memory.";
  token.focus();
});
refresh.addEventListener("click", () => void loadModels());
search.addEventListener("input", () => void loadModels());
captureFile.addEventListener("change", () => {
  importCapture.disabled =
    !capabilities?.features.captureInbox || !captureRoutesAvailable || !captureFile.files?.length;
});
importCapture.addEventListener("click", () => {
  void (async () => {
    const queueFile = captureFile.files?.[0];
    if (!queueFile || queueFile.size > 64 * 1024) throw new Error("Capture JSON exceeds 64 KiB");
    importCapture.disabled = true;
    const envelope = JSON.parse(await queueFile.text()) as CaptureEnvelope;
    if (envelope.version !== 1 || !envelope.itemId || !envelope.page?.url) {
      throw new Error("Capture JSON is invalid");
    }
    const pageUrl = new URL(envelope.page.url);
    if (pageUrl.protocol !== "https:") throw new Error("Capture page URL must use HTTPS");
    const browserEnvelope: CaptureEnvelope = {
      ...envelope,
      downloads: envelope.downloads.map((download) => ({
        ...download,
        relativeInboxPath: null,
      })),
    };
    const files = new Map<string, File>();
    for (const file of modelFiles.files ?? []) {
      const key = file.name.toLocaleLowerCase();
      if (files.has(key)) throw new Error(`Choose only one file named ${file.name}`);
      files.set(key, file);
    }
    const tokens = [];
    for (const download of envelope.downloads) {
      const file = files.get(download.fileName.toLocaleLowerCase());
      if (!file) continue;
      if (capabilities && file.size > capabilities.limits.modelUploadBytes) {
        throw new Error(`${file.name} exceeds the server upload limit`);
      }
      const digest = [
        ...new Uint8Array(await crypto.subtle.digest("SHA-256", await file.arrayBuffer())),
      ]
        .map((byte) => byte.toString(16).padStart(2, "0"))
        .join("");
      const token = await client.stageCaptureUpload(
        {
          itemId: envelope.itemId,
          captureId: download.captureId,
          fileName: file.name,
          bytes: String(file.size),
          sha256: digest,
        },
        file,
      );
      tokens.push(token);
    }
    const result = await client.importCapture(browserEnvelope, tokens);
    status.textContent = `Capture ${result.status}: ${result.downloads.length} download records processed.`;
    await loadModels();
  })()
    .catch((error) => {
      status.textContent = String((error as { message?: unknown }).message ?? error);
    })
    .finally(() => {
      importCapture.disabled =
        !capabilities?.features.captureInbox ||
        !captureRoutesAvailable ||
        !captureFile.files?.length;
    });
});
if (!token.disabled) token.focus();
