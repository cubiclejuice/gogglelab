import {
  HttpLibraryClient,
  LibraryHttpError,
  formatWireInteger,
  type BrowserModel,
  type BrowserModelQuery,
  type CaptureEnvelope,
  type HostCapabilities,
} from "./lib/library-client";
import { isBrowserDialogCurrent } from "./lib/browser-dialog";
import {
  BoundedTaskQueue,
  isThumbnailRequestCurrent,
  shouldRetryStaleThumbnail,
} from "./lib/browser-thumbnail-queue";
import { decodeGeometryBuffer } from "./lib/geometry-wire";
import {
  browserSelectionTargets,
  ensureBulkSelectionLimit,
  pagesToRestore,
} from "./lib/browser-selection";
import { LibraryAiDialog } from "./lib/library-ai-dialog";
import "./styles.css";

const root = document.querySelector<HTMLElement>("#browser-app")!;
const client = new HttpLibraryClient();
const integrationDialog = new LibraryAiDialog(document.body);
let bundleDialog: import("./lib/library-bundle-dialog").LibraryBundleDialog | null = null;
async function getBundleDialog(): Promise<
  import("./lib/library-bundle-dialog").LibraryBundleDialog
> {
  if (bundleDialog) return bundleDialog;
  const { LibraryBundleDialog } = await import("./lib/library-bundle-dialog");
  const dialog = new LibraryBundleDialog(document.body);
  dialog.onLoadGeometry = async (token) =>
    decodeGeometryBuffer(await client.readBundleGeometry(token));
  dialog.onClosePreview = (token) => client.closeBundle(token);
  dialog.onRemoteDownload = downloadBundleArchive;
  bundleDialog = dialog;
  return dialog;
}

async function downloadBundleArchive(token: string): Promise<void> {
  const picker = (
    window as unknown as {
      showSaveFilePicker?: (options: unknown) => Promise<{
        createWritable(): Promise<{
          write(chunk: Uint8Array): Promise<void>;
          close(): Promise<void>;
          abort(reason?: unknown): Promise<void>;
        }>;
      }>;
    }
  ).showSaveFilePicker;
  const handle = picker
    ? await picker.call(window, {
        suggestedName: "gogglelab-bundle.zip",
        types: [{ description: "ZIP archive", accept: { "application/zip": [".zip"] } }],
      })
    : null;
  const lease = await client.issueBundleDownload(token);
  if (handle) {
    const response = await client.contentResponse(lease.id);
    if (!response.body) throw new Error("The bundle stream is unavailable");
    const writable = await handle.createWritable();
    const reader = response.body.getReader();
    let written = 0n;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        written += BigInt(value.byteLength);
        if (written > BigInt(lease.bytes))
          throw new Error("Bundle stream exceeded its declared size");
        await writable.write(value);
      }
      if (written !== BigInt(lease.bytes))
        throw new Error("Bundle stream ended before it was complete");
      await writable.close();
    } catch (error) {
      await reader.cancel(error).catch(() => {});
      await writable.abort(error).catch(() => {});
      throw error;
    }
    return;
  }
  if (BigInt(lease.bytes) > 100n * 1024n * 1024n)
    throw new Error(
      "This bundle is too large for the in-memory fallback. Use a browser with streaming file save support.",
    );
  const response = await client.contentResponse(lease.id);
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = lease.fileName;
  link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
let capabilities: HostCapabilities | null = null;
let captureRoutesAvailable = false;
let galleryRoutesAvailable = false;
let downloadRoutesAvailable = false;
let metadataRoutesAvailable = false;
let historyRoutesAvailable = false;
let historyBatchRoutesAvailable = false;
let filamentRoutesAvailable = false;
let scanRoutesAvailable = false;
let captureSummaryRoutesAvailable = false;
let bundleRoutesAvailable = false;
let geometryRoutesAvailable = false;
let embeddedRoutesAvailable = false;
let duplicateRoutesAvailable = false;
let catalogSettingsRoutesAvailable = false;
let managementRoutesAvailable = false;
let integrationRoutesAvailable = false;
let thumbnailJobRoutesAvailable = false;
let retryCaptureItemId: string | null = null;
let eventController: AbortController | null = null;
let modelController: AbortController | null = null;
let modelLoadRevision = 0;
let loadedModels: BrowserModel[] = [];
let nextModelCursor: string | null = null;
const selectedModels = new Map<string, BrowserModel>();
const thumbnailQueue = new BoundedTaskQueue(4);
const THUMBNAIL_CACHE_COUNT = 80;
const THUMBNAIL_CACHE_BYTES = 32 * 1024 * 1024;
const thumbnailCache = new Map<string, { url: string; bytes: number }>();
const thumbnailPending = new Map<string, Promise<string>>();
let thumbnailObserver: IntersectionObserver | null = null;
let thumbnailGeneration = 0;
let thumbnailController = new AbortController();

function evictThumbnailCache(): void {
  const displayed = new Set(
    [...models.querySelectorAll<HTMLImageElement>(".browser-card-preview img")].map(
      (image) => image.src,
    ),
  );
  let bytes = [...thumbnailCache.values()].reduce((total, entry) => total + entry.bytes, 0);
  for (const [key, entry] of thumbnailCache) {
    if (thumbnailCache.size <= THUMBNAIL_CACHE_COUNT && bytes <= THUMBNAIL_CACHE_BYTES) break;
    if (displayed.has(entry.url)) continue;
    thumbnailCache.delete(key);
    bytes -= entry.bytes;
    URL.revokeObjectURL(entry.url);
  }
}

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
const manageCatalog = actionButton("Manage catalog", () => void showCatalogManager());
manageCatalog.disabled = true;
const reviewDuplicates = actionButton("Duplicates in loaded", () => void showDuplicates());
reviewDuplicates.disabled = true;
const browserSettings = actionButton("Catalog settings", () => void showBrowserSettings());
browserSettings.disabled = true;
const browserManagement = actionButton("Backup and recovery", () => void showManagement());
browserManagement.disabled = true;
const browserIntegrations = actionButton("Integrations", () => void showIntegrations());
browserIntegrations.disabled = true;
const generateSelected = actionButton(
  "Generate missing selected",
  () => void startSelectedThumbnails(false),
);
const regenerateSelected = actionButton(
  "Regenerate selected",
  () => void startSelectedThumbnails(true),
);
const selectionStatus = document.createElement("span");
selectionStatus.textContent = "0 selected";
const selectVisible = actionButton("Select loaded", () => {
  for (const model of loadedModels) selectedModels.set(model.id, model);
  renderModels(loadedModels);
});
const selectFiltered = actionButton("Select all filtered", () => void selectAllFiltered());
const clearSelection = actionButton("Clear selection", () => {
  selectedModels.clear();
  renderModels(loadedModels);
});
const editSelection = actionButton("Edit selected", () => void showBulkMetadata());
const sort = document.createElement("select");
sort.setAttribute("aria-label", "Sort models");
for (const [value, label] of [
  ["date", "Date"],
  ["name", "Name"],
  ["size", "Size"],
]) {
  const option = document.createElement("option");
  option.value = value;
  option.textContent = label;
  sort.append(option);
}
const direction = document.createElement("select");
direction.setAttribute("aria-label", "Sort direction");
for (const [value, label] of [
  ["desc", "Descending"],
  ["asc", "Ascending"],
]) {
  const option = document.createElement("option");
  option.value = value;
  option.textContent = label;
  direction.append(option);
}
const folderFilter = document.createElement("select");
folderFilter.setAttribute("aria-label", "Folder filter");
const tagFilter = document.createElement("select");
tagFilter.setAttribute("aria-label", "Tag filter");
const filamentFilter = document.createElement("select");
filamentFilter.setAttribute("aria-label", "Filament filter");
const rootFilter = document.createElement("select");
rootFilter.setAttribute("aria-label", "Indexed root filter");
const directoryFilter = document.createElement("input");
directoryFilter.placeholder = "Relative source directory";
directoryFilter.setAttribute("aria-label", "Relative source directory filter");
const statusFilter = document.createElement("select");
statusFilter.setAttribute("aria-label", "Print status filter");
const sourceFilter = document.createElement("select");
sourceFilter.setAttribute("aria-label", "Source filter");
const groupFilter = document.createElement("select");
groupFilter.setAttribute("aria-label", "Group filter");
const categoryFilter = document.createElement("input");
categoryFilter.placeholder = "Category";
categoryFilter.setAttribute("aria-label", "Category filter");
const designerFilter = document.createElement("input");
designerFilter.placeholder = "Designer";
designerFilter.setAttribute("aria-label", "Designer filter");
const licenseFilter = document.createElement("input");
licenseFilter.placeholder = "License";
licenseFilter.setAttribute("aria-label", "License filter");
const availabilityFilter = document.createElement("select");
availabilityFilter.setAttribute("aria-label", "Source availability filter");
const printedFilter = document.createElement("select");
printedFilter.setAttribute("aria-label", "Print history filter");
const thumbnailFilter = document.createElement("select");
thumbnailFilter.setAttribute("aria-label", "Thumbnail filter");
const ratingFilter = document.createElement("select");
ratingFilter.setAttribute("aria-label", "Minimum rating filter");
const favoriteFilter = document.createElement("select");
favoriteFilter.setAttribute("aria-label", "Favorite filter");
const duplicatesFilter = document.createElement("label");
const duplicatesOnly = document.createElement("input");
duplicatesOnly.type = "checkbox";
duplicatesFilter.append(duplicatesOnly, document.createTextNode("Duplicates"));
const viewMode = document.createElement("select");
viewMode.setAttribute("aria-label", "Catalog view");
function fillFilter(
  select: HTMLSelectElement,
  first: string,
  entries: Array<[string, string]>,
): void {
  select.replaceChildren();
  const all = document.createElement("option");
  all.value = "";
  all.textContent = first;
  select.append(all);
  for (const [value, label] of entries) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    select.append(option);
  }
}
fillFilter(
  statusFilter,
  "All print statuses",
  ["unprinted", "want", "queued", "printing", "printed", "failed"].map((v) => [v, v]),
);
fillFilter(sourceFilter, "All sources", [
  ["managed", "Managed copies"],
  ["indexed", "Indexed sources"],
]);
fillFilter(groupFilter, "All groups", [
  ["standalone", "Standalone"],
  ["bundle", "Bundles"],
  ["archive", "Archives"],
]);
fillFilter(availabilityFilter, "All availability", [
  ["false", "Available source"],
  ["true", "Missing source"],
]);
fillFilter(printedFilter, "Any print history", [
  ["true", "Ever printed"],
  ["false", "Never printed"],
]);
fillFilter(thumbnailFilter, "All thumbnails", [
  ["missing", "Missing preview"],
  ["default", "Generated preview"],
  ["custom", "Custom preview"],
]);
fillFilter(ratingFilter, "Any rating", [
  ...[1, 2, 3, 4, 5].map(
    (rating) => [String(rating), `${rating} stars or more`] as [string, string],
  ),
]);
fillFilter(favoriteFilter, "All favorites", [
  ["true", "Favorites"],
  ["false", "Not favorites"],
]);
fillFilter(folderFilter, "All folders", []);
fillFilter(tagFilter, "All tags", []);
fillFilter(filamentFilter, "All filaments", []);
fillFilter(rootFilter, "All indexed roots", []);
fillFilter(viewMode, "Gallery", [
  ["list", "List"],
  ["detailed", "Detailed"],
]);
toolbar.append(
  search,
  sort,
  direction,
  folderFilter,
  tagFilter,
  filamentFilter,
  rootFilter,
  directoryFilter,
  statusFilter,
  sourceFilter,
  groupFilter,
  categoryFilter,
  designerFilter,
  licenseFilter,
  availabilityFilter,
  printedFilter,
  thumbnailFilter,
  ratingFilter,
  favoriteFilter,
  duplicatesFilter,
  viewMode,
  refresh,
  manageCatalog,
  reviewDuplicates,
  browserSettings,
  browserManagement,
  browserIntegrations,
  generateSelected,
  regenerateSelected,
  selectionStatus,
  selectVisible,
  selectFiltered,
  clearSelection,
  editSelection,
);
const catalogControls = [
  search,
  sort,
  direction,
  folderFilter,
  tagFilter,
  filamentFilter,
  rootFilter,
  directoryFilter,
  statusFilter,
  sourceFilter,
  groupFilter,
  categoryFilter,
  designerFilter,
  licenseFilter,
  availabilityFilter,
  printedFilter,
  thumbnailFilter,
  ratingFilter,
  favoriteFilter,
  duplicatesOnly,
  viewMode,
  refresh,
  manageCatalog,
  reviewDuplicates,
  browserSettings,
  browserManagement,
  browserIntegrations,
  generateSelected,
  regenerateSelected,
  selectVisible,
  selectFiltered,
  clearSelection,
  editSelection,
];
for (const control of catalogControls) control.disabled = true;

const status = document.createElement("div");
status.className = "library-status";
status.setAttribute("role", "status");
const models = document.createElement("main");
models.className = "browser-model-grid";
models.dataset.view = "gallery";
const loadMore = document.createElement("button");
loadMore.type = "button";
loadMore.className = "btn browser-load-more";
loadMore.textContent = "Load more";
loadMore.hidden = true;

async function showCatalogManager(): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-catalog-manager";
  const title = document.createElement("h2");
  title.textContent = "Catalog lists";
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const content = document.createElement("div");
  const close = actionButton("Close", () => dialog.close());
  dialog.append(title, message, content, close);
  document.body.append(dialog);
  dialog.showModal();
  close.focus();
  let revision = 0;
  const render = async () => {
    const request = ++revision;
    try {
      const [folders, tags, filaments] = await Promise.all([
        client.listFolders(),
        client.listTags(),
        client.listFilaments(true),
      ]);
      if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
      content.replaceChildren(
        managerSection(
          "Folders",
          folders,
          (item) => item.name,
          async (name) => {
            await client.createFolder(name);
          },
          async (item, name) => {
            await client.renameFolder(item.id, name);
          },
          async (item) => {
            await client.deleteFolder(item.id);
          },
        ),
        managerSection(
          "Tags",
          tags,
          (item) => (item.category ? `${item.name} | ${item.category}` : item.name),
          async (value) => {
            const [name, category] = value.split("|").map((part) => part.trim());
            await client.createTag(name, category || null);
          },
          async (item, value) => {
            const [name, category] = value.split("|").map((part) => part.trim());
            await client.updateTag(item.id, name, category || null);
          },
          async (item) => {
            await client.deleteTag(item.id);
          },
        ),
        filamentManager(filaments),
      );
    } catch (error) {
      if (isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected))
        message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  function managerSection<T extends { id: string }>(
    heading: string,
    items: T[],
    label: (item: T) => string,
    create: (value: string) => Promise<void>,
    update: (item: T, value: string) => Promise<void>,
    remove: (item: T) => Promise<void>,
  ): HTMLElement {
    const section = document.createElement("section");
    const h = document.createElement("h3");
    h.textContent = heading;
    const createInput = document.createElement("input");
    createInput.setAttribute("aria-label", `New ${heading.slice(0, -1)}`);
    const add = actionButton("Add", () => void run(() => create(createInput.value)));
    section.append(h, createInput, add);
    for (const item of items) {
      const row = document.createElement("div");
      const input = document.createElement("input");
      input.value = label(item);
      input.setAttribute("aria-label", `${heading.slice(0, -1)} name`);
      row.append(
        input,
        actionButton("Save", () => void run(() => update(item, input.value))),
        actionButton("Delete", () => void run(() => remove(item))),
      );
      section.append(row);
    }
    return section;
  }
  function filamentManager(items: import("./lib/library-client").BrowserFilament[]): HTMLElement {
    const section = document.createElement("section");
    const h = document.createElement("h3");
    h.textContent = "Filaments";
    section.append(h);
    const fields = ["brand", "material", "color", "name", "notes"] as const;
    const inputs = fields.map((field) => {
      const input = document.createElement("input");
      input.placeholder = field;
      input.setAttribute("aria-label", `Filament ${field}`);
      section.append(input);
      return input;
    });
    section.append(
      actionButton(
        "Add filament",
        () =>
          void run(() =>
            client.createFilament({
              brand: inputs[0].value,
              material: inputs[1].value,
              color: inputs[2].value,
              name: inputs[3].value,
              notes: inputs[4].value,
            }),
          ),
      ),
    );
    for (const item of items) {
      const row = document.createElement("div");
      const label = document.createElement("span");
      label.textContent = `${item.brand} ${item.name || item.material}${item.archived || item.sourceMissing ? " · unavailable" : ""}`;
      row.append(label);
      if (item.origin === "manual")
        row.append(
          actionButton(
            "Archive",
            () => void run(() => client.deleteFilament(item.id, item.version)),
          ),
        );
      section.append(row);
    }
    return section;
  }
  async function run(operation: () => Promise<unknown>): Promise<void> {
    message.textContent = "Saving…";
    try {
      await operation();
      if (!dialog.open) return;
      message.textContent = "Saved.";
      await render();
      await configureCatalogFilters({
        "/api/v1/folders": {},
        "/api/v1/tags": {},
        "/api/v1/filaments": {},
      });
    } catch (error) {
      if (dialog.open)
        message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  }
  dialog.addEventListener(
    "close",
    () => {
      revision += 1;
      dialog.remove();
    },
    { once: true },
  );
  await render();
}

async function showBrowserSettings(): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-catalog-manager";
  const title = document.createElement("h2");
  title.textContent = "Catalog and index settings";
  const content = document.createElement("div");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const close = actionButton("Close", () => dialog.close());
  dialog.append(title, message, content, close);
  document.body.append(dialog);
  dialog.showModal();
  try {
    const [roots, settings] = await Promise.all([client.rootSettings(), client.catalogSettings()]);
    const catalog = document.createElement("section");
    const catalogTitle = document.createElement("h3");
    catalogTitle.textContent = "Preview and scan defaults";
    const size = document.createElement("select");
    for (const value of [128, 256, 512]) size.add(new Option(`${value} px`, String(value)));
    size.value = String(settings.thumbnailSize);
    const background = document.createElement("input");
    background.type = "color";
    background.value = settings.thumbnailBackground;
    const concurrency = document.createElement("input");
    concurrency.type = "number";
    concurrency.min = "1";
    concurrency.max = "2";
    concurrency.value = String(settings.thumbnailConcurrency);
    const batch = document.createElement("input");
    batch.type = "number";
    batch.min = "16";
    batch.max = "256";
    batch.value = String(settings.scanBatchSize);
    catalog.append(
      catalogTitle,
      size,
      background,
      concurrency,
      batch,
      actionButton(
        "Save catalog settings",
        () =>
          void client
            .updateCatalogSettings({
              thumbnailSize: Number(size.value) as 128 | 256 | 512,
              thumbnailBackground: background.value,
              thumbnailConcurrency: Number(concurrency.value) as 1 | 2,
              scanBatchSize: Number(batch.value),
            })
            .then(() => {
              message.textContent = "Catalog settings saved.";
            })
            .catch((error) => {
              message.textContent = String(error?.message ?? error);
            }),
      ),
    );
    content.append(catalog);
    for (const rootSetting of roots) {
      const section = document.createElement("section");
      const heading = document.createElement("h3");
      heading.textContent = rootSetting.label;
      const enabled = document.createElement("input");
      enabled.type = "checkbox";
      enabled.checked = rootSetting.enabled;
      const interval = document.createElement("input");
      interval.type = "number";
      interval.min = "1";
      interval.max = "1440";
      interval.value = String(rootSetting.scanIntervalMinutes);
      const maxMb = document.createElement("input");
      maxMb.type = "number";
      maxMb.min = "1";
      maxMb.max = "100";
      maxMb.value = String(Number(BigInt(rootSetting.maxFileBytes) / (1024n * 1024n)) || 1);
      const zip = document.createElement("input");
      zip.type = "checkbox";
      zip.checked = rootSetting.scanZip;
      const labeled = (text: string, control: HTMLElement) => {
        const label = document.createElement("label");
        label.append(document.createTextNode(text), control);
        return label;
      };
      section.append(
        heading,
        labeled("Enabled ", enabled),
        labeled("Interval minutes ", interval),
        labeled("Maximum file MiB ", maxMb),
        labeled("Scan ZIP archives ", zip),
        actionButton(
          "Save root",
          () =>
            void client
              .updateRootSettings(rootSetting.handleId, {
                enabled: enabled.checked,
                scanIntervalMinutes: Number(interval.value),
                maxFileBytes: String(Number(maxMb.value) * 1024 * 1024),
                scanZip: zip.checked,
              })
              .then(() => {
                message.textContent = `${rootSetting.label} saved.`;
              })
              .catch((error) => {
                message.textContent = String(error?.message ?? error);
              }),
        ),
      );
      content.append(section);
    }
  } catch (error) {
    message.textContent = String((error as { message?: unknown }).message ?? error);
  }
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showManagement(): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-catalog-manager";
  const title = document.createElement("h2");
  title.textContent = "Backup and recovery";
  const controls = document.createElement("div");
  const planArea = document.createElement("section");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  dialog.append(
    title,
    controls,
    planArea,
    message,
    actionButton("Close", () => dialog.close()),
  );
  document.body.append(dialog);
  dialog.showModal();
  const roots = await client.rootSettings();
  const handles = roots.filter((root) => root.access !== "readOnly");
  const handle = document.createElement("select");
  handle.setAttribute("aria-label", "Operator storage handle");
  for (const root of handles) handle.add(new Option(root.label, root.handleId));
  const fileName = document.createElement("input");
  fileName.value = "gogglelab-backup.zip";
  fileName.setAttribute("aria-label", "Backup or export filename");
  const snapshot = document.createElement("input");
  snapshot.placeholder = "Removal snapshot ID";
  const backupRoot = document.createElement("input");
  backupRoot.placeholder = "Backup catalog root ID";
  const mappingHandle = handle.cloneNode(true) as HTMLSelectElement;
  const targets = () => browserSelectionTargets(selectedModels.values());
  const plan = async (action: import("./lib/library-client").ManagementAction) => {
    message.textContent = "Planning…";
    try {
      const result = await client.planManagementAction(action);
      planArea.replaceChildren();
      const heading = document.createElement("h3");
      heading.textContent = `${result.actionKind} · expires ${new Date(result.expiresAt).toLocaleTimeString()}`;
      const list = document.createElement("ul");
      for (const impact of result.impact) {
        const item = document.createElement("li");
        item.textContent = impact;
        list.append(item);
      }
      const execute = actionButton("Execute this plan", () => {
        execute.disabled = true;
        void client
          .executeManagementAction(result.token)
          .then(async (outcome) => {
            message.textContent = JSON.stringify(outcome);
            selectedModels.clear();
            await reloadLoadedPages();
          })
          .catch((error) => {
            message.textContent = String(error?.message ?? error);
            execute.disabled = false;
          });
      });
      planArea.append(heading, list, execute);
      message.textContent = "Review the impact, then execute explicitly.";
    } catch (error) {
      message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  controls.append(
    handle,
    fileName,
    actionButton(
      "Plan backup",
      () =>
        void plan({
          kind: "createBackup",
          destinationHandleId: handle.value,
          fileName: fileName.value,
        }),
    ),
    actionButton("Plan export selected", () => {
      const selected = targets();
      if (!selected.length) {
        message.textContent = "Select models first.";
        return;
      }
      void plan({
        kind: "exportSelection",
        targets: selected,
        destinationHandleId: handle.value,
        fileName: fileName.value,
      });
    }),
    actionButton("Plan recoverable removal", () => {
      const selected = targets();
      if (!selected.length) {
        message.textContent = "Select models first.";
        return;
      }
      void plan({ kind: "recoverableRemove", targets: selected });
    }),
    snapshot,
    actionButton(
      "Plan restore removal",
      () => void plan({ kind: "restoreRemoval", snapshotId: snapshot.value }),
    ),
    backupRoot,
    mappingHandle,
    actionButton(
      "Plan restore backup",
      () =>
        void plan({
          kind: "restoreBackup",
          sourceHandleId: handle.value,
          fileName: fileName.value,
          rootMappings: backupRoot.value
            ? [
                {
                  backupCatalogRootId: backupRoot.value,
                  operatorHandleId: mappingHandle.value,
                  enabled: true,
                },
              ]
            : [],
        }),
    ),
  );
  if (capabilities?.features.trashSource) {
    const selected = [...selectedModels.values()];
    const physical =
      selected.length === 1 && selected[0].sourceKind === "indexed" && !selected[0].archiveMember
        ? browserSelectionTargets(selected)[0]
        : null;
    const writable = roots.filter((root) => root.access === "writable");
    if (physical && writable.length) {
      const physicalHandle = document.createElement("select");
      for (const root of writable) physicalHandle.add(new Option(root.label, root.handleId));
      const physicalName = document.createElement("input");
      physicalName.value = selected[0].displayName;
      controls.append(
        actionButton(
          "Plan quarantine source",
          () => void plan({ kind: "quarantineLooseSource", target: physical }),
        ),
        physicalHandle,
        physicalName,
        actionButton(
          "Plan move source",
          () =>
            void plan({
              kind: "moveLooseSource",
              target: physical,
              destinationHandleId: physicalHandle.value,
              fileName: physicalName.value,
            }),
        ),
      );
    }
    controls.append(
      actionButton(
        "Plan undo quarantine",
        () => void plan({ kind: "undoQuarantine", snapshotId: snapshot.value }),
      ),
      actionButton(
        "Plan catalog purge with backup",
        () =>
          void plan({
            kind: "purgeCatalog",
            backupDestinationHandleId: handle.value,
            backupFileName: fileName.value,
          }),
      ),
      actionButton("Plan orphan cleanup", () => void plan({ kind: "cleanupOrphans" })),
    );
  } else {
    const notice = document.createElement("p");
    notice.textContent = "Physical source actions are disabled on this host.";
    controls.append(notice);
  }
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showIntegrations(): Promise<void> {
  try {
    const settings = await client.integrationSettings();
    const chosen = selectedModels.size ? [...selectedModels.values()] : loadedModels;
    integrationDialog.show(
      settings as unknown as import("./lib/ipc").IntegrationSettings,
      chosen.map((model) => ({
        id: model.id,
        display_name: model.displayName,
        metadata_version: model.metadataVersion,
        tags: model.tags,
      })) as unknown as import("./lib/ipc").LibraryModel[],
      browserIntegrations,
      {
        ai: capabilities?.features.ai ?? false,
        spoolman: capabilities?.features.spoolman ?? false,
      },
    );
    if (!capabilities?.features.ai && !capabilities?.features.spoolman)
      status.textContent =
        "Integration settings are available, but provider networking is disabled on this host.";
  } catch (error) {
    status.textContent = String((error as { message?: unknown }).message ?? error);
  }
}

integrationDialog.onSaveSpoolman = async (config) => {
  await client.saveSpoolmanConfig(config);
};
integrationDialog.onPullSpoolman = async () => {
  if (!capabilities?.features.spoolman)
    throw new Error("Spoolman networking is disabled on this host.");
  return JSON.stringify(await client.pullSpoolman());
};
integrationDialog.onSaveProvider = async (config, credential) => {
  await client.saveAiProvider(config, credential);
};
integrationDialog.onDeleteProvider = (providerId) => client.deleteAiProvider(providerId);
integrationDialog.onSuggest = async (providerId, targets) => {
  if (!capabilities?.features.ai)
    throw new Error("AI provider networking is disabled on this host.");
  return (await client.suggestAiTags(
    providerId,
    targets.map((target) => ({
      modelId: target.model_id,
      expectedVersion: String(target.expected_version),
    })),
  )) as unknown as import("./lib/ipc").AiSuggestionBatch;
};
integrationDialog.onApply = async (reviewToken, decisions, strategy) => {
  await client.applyAiReview(reviewToken, decisions, strategy);
  await reloadLoadedPages();
};
integrationDialog.onDiscard = (reviewToken) => client.discardAiReview(reviewToken);

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
const viewCaptures = actionButton("View capture inbox", () => void showCaptureInbox());
capture.append(captureTitle, captureFile, modelFiles, importCapture, viewCaptures);
const scanPanel = document.createElement("section");
scanPanel.className = "browser-scan-panel";
scanPanel.hidden = true;
const scanRoot = document.createElement("select");
scanRoot.setAttribute("aria-label", "Indexed root to scan");
const scanStart = actionButton("Start scan", () => void startBrowserScan());
const scanCancel = actionButton("Cancel scan", () => void cancelBrowserScan());
scanCancel.disabled = true;
const scanProgress = document.createElement("span");
scanProgress.setAttribute("role", "status");
scanPanel.append(scanRoot, scanStart, scanCancel, scanProgress);
root.append(header, login, toolbar, scanPanel, capture, status, models, loadMore);
if (client.endpointSecurityError) {
  status.setAttribute("role", "alert");
  status.textContent = client.endpointSecurityError.message;
}

let scanPoll: number | null = null;
function renderScanState(state: import("./lib/library-client").BrowserScanStatus): void {
  scanStart.disabled = state.running;
  scanCancel.disabled = !state.running || state.cancelling;
  scanProgress.textContent = state.running
    ? state.jobKind === "thumbnails"
      ? `${state.cancelling ? "Cancelling" : "Rendering previews"} · ${state.thumbnailsCompleted}/${state.thumbnailsTotal}`
      : `${state.cancelling ? "Cancelling" : "Scanning"} · ${state.filesVisited} visited · ${state.filesIndexed} indexed`
    : state.errors.length
      ? `Finished with ${state.errors.length} errors`
      : "Scanner idle";
  if (scanPoll !== null) window.clearTimeout(scanPoll);
  scanPoll =
    state.running && capabilities && !document.hidden
      ? window.setTimeout(() => void refreshScanState(), 1500)
      : null;
}
async function refreshScanState(): Promise<void> {
  if (!scanRoutesAvailable || !capabilities) return;
  try {
    renderScanState(await client.scanStatus());
  } catch (error) {
    scanProgress.textContent = String((error as { message?: unknown }).message ?? error);
  }
}
async function startBrowserScan(): Promise<void> {
  try {
    renderScanState(await client.startScan(scanRoot.value || null));
  } catch (error) {
    scanProgress.textContent = String((error as { message?: unknown }).message ?? error);
  }
}
async function startSelectedThumbnails(force: boolean): Promise<void> {
  const ids = [...selectedModels.keys()];
  if (!ids.length) {
    status.textContent = "Select models first.";
    return;
  }
  ensureBulkSelectionLimit(ids.length);
  try {
    renderScanState(await client.startThumbnailJob(ids, force));
  } catch (error) {
    scanProgress.textContent = String((error as { message?: unknown }).message ?? error);
  }
}
async function cancelBrowserScan(): Promise<void> {
  try {
    renderScanState(await client.cancelScan());
  } catch (error) {
    scanProgress.textContent = String((error as { message?: unknown }).message ?? error);
  }
}
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) void refreshScanState();
  else if (scanPoll !== null) {
    window.clearTimeout(scanPoll);
    scanPoll = null;
  }
});

async function showCaptureInbox(): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-catalog-manager";
  const title = document.createElement("h2");
  title.textContent = "Capture inbox";
  const list = document.createElement("div");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const more = actionButton("Load more", () => void load());
  const close = actionButton("Close", () => dialog.close());
  dialog.append(title, message, list, more, close);
  document.body.append(dialog);
  dialog.showModal();
  close.focus();
  let cursor: string | undefined;
  let revision = 0;
  const load = async () => {
    const request = ++revision;
    more.disabled = true;
    try {
      const page = await client.listCaptures({ limit: 50, cursor });
      if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
      for (const item of page.items) {
        const row = document.createElement("article");
        const heading = document.createElement("strong");
        heading.textContent = item.title;
        const details = document.createElement("span");
        details.textContent = `${item.status} · ${item.downloadCount} downloads · ${item.failedCount} failed · ${item.needsFileCount} need files`;
        const retry = actionButton("Retry with queue file…", () => {
          retryCaptureItemId = item.itemId;
          captureFile.click();
          dialog.close();
        });
        row.append(heading, details, retry);
        list.append(row);
      }
      cursor = page.nextCursor ?? undefined;
      more.hidden = !cursor;
      more.disabled = false;
      message.textContent = page.items.length ? "" : "No capture records.";
    } catch (error) {
      if (isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected))
        message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  dialog.addEventListener(
    "close",
    () => {
      revision += 1;
      dialog.remove();
    },
    { once: true },
  );
  await load();
}

function renderModels(items: BrowserModel[]): void {
  thumbnailObserver?.disconnect();
  thumbnailObserver =
    typeof IntersectionObserver === "undefined"
      ? null
      : new IntersectionObserver(
          (entries) => {
            for (const entry of entries)
              if (entry.isIntersecting) {
                thumbnailObserver?.unobserve(entry.target);
                const card = entry.target as HTMLElement;
                const model = items.find((candidate) => candidate.id === card.dataset.modelId);
                if (model) void renderCardThumbnail(model, card);
              }
          },
          { rootMargin: "180px" },
        );
  models.replaceChildren();
  evictThumbnailCache();
  for (const model of items) {
    const card = document.createElement("article");
    card.className = "browser-model-card";
    card.dataset.modelId = model.id;
    const preview = document.createElement("div");
    preview.className = "browser-card-preview";
    card.append(preview);
    const selected = document.createElement("input");
    selected.type = "checkbox";
    selected.checked = selectedModels.has(model.id);
    selected.setAttribute("aria-label", `Select ${model.displayName}`);
    selected.addEventListener("change", () => {
      if (selected.checked) selectedModels.set(model.id, model);
      else selectedModels.delete(model.id);
      updateSelectionStatus();
    });
    card.append(selected);
    const name = document.createElement("h2");
    name.textContent = model.displayName;
    const meta = document.createElement("p");
    const availability = model.availability;
    meta.textContent = `${model.format ?? "Bookmark"} · ${formatWireInteger(model.bytes)} bytes · ${availability} · Rating ${model.rating}/5${model.favorite ? " · Favorite" : ""}`;
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
    if (bundleRoutesAvailable && model.bundleId) {
      const previewBundle = actionButton(
        "Preview bundle",
        () =>
          void client
            .prepareBundle(model.bundleId!)
            .then(async (summary) => (await getBundleDialog()).show(summary, [], previewBundle))
            .catch((error) => {
              status.textContent = String((error as { message?: unknown }).message ?? error);
            }),
      );
      actions.append(previewBundle);
    }
    if (
      geometryRoutesAvailable &&
      model.availability === "available" &&
      model.catalogKind === "file"
    )
      actions.append(actionButton("Open 3D", () => void showModelViewer(model)));
    if (embeddedRoutesAvailable && model.format === "3mf" && model.availability === "available")
      actions.append(actionButton("Embedded metadata", () => void showEmbeddedMetadata(model)));
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
    if (galleryRoutesAvailable && model.thumbnailId) {
      if (thumbnailObserver) thumbnailObserver.observe(card);
      else void renderCardThumbnail(model, card);
    }
  }
  if (!items.length) {
    const empty = document.createElement("p");
    empty.textContent = "No models match this query.";
    models.append(empty);
  }
  updateSelectionStatus();
}

function updateSelectionStatus(): void {
  selectionStatus.textContent = `${selectedModels.size} selected`;
  clearSelection.disabled = selectedModels.size === 0;
  editSelection.disabled = selectedModels.size === 0;
  generateSelected.disabled = selectedModels.size === 0;
  regenerateSelected.disabled = selectedModels.size === 0;
}

function activeQuery(cursor?: string): BrowserModelQuery {
  return {
    limit: Math.min(capabilities?.limits.pageSizeMax ?? 50, 100),
    cursor,
    query: search.value.trim() || undefined,
    folderId: folderFilter.value || undefined,
    tagId: tagFilter.value || undefined,
    filamentId: filamentFilter.value || undefined,
    rootId: rootFilter.value || undefined,
    sourceDirectory: directoryFilter.value.trim() || undefined,
    printStatus: (statusFilter.value || undefined) as BrowserModelQuery["printStatus"],
    sourceKind: (sourceFilter.value || undefined) as BrowserModelQuery["sourceKind"],
    group: (groupFilter.value || undefined) as BrowserModelQuery["group"],
    category: categoryFilter.value.trim() || undefined,
    designer: designerFilter.value.trim() || undefined,
    modelLicense: licenseFilter.value.trim() || undefined,
    sourceMissing: availabilityFilter.value ? availabilityFilter.value === "true" : undefined,
    everPrinted: printedFilter.value ? printedFilter.value === "true" : undefined,
    duplicatesOnly: duplicatesOnly.checked || undefined,
    thumbnailMode: (thumbnailFilter.value || undefined) as BrowserModelQuery["thumbnailMode"],
    minimumRating: ratingFilter.value ? Number(ratingFilter.value) : undefined,
    favorite: favoriteFilter.value ? favoriteFilter.value === "true" : undefined,
    sort: sort.value as BrowserModelQuery["sort"],
    direction: direction.value as BrowserModelQuery["direction"],
  };
}

async function selectAllFiltered(): Promise<void> {
  selectFiltered.disabled = true;
  try {
    const nextSelection = new Map(selectedModels);
    let cursor: string | undefined;
    do {
      const page = await client.listModels(activeQuery(cursor));
      for (const model of page.items) nextSelection.set(model.id, model);
      ensureBulkSelectionLimit(nextSelection.size);
      cursor = page.nextCursor ?? undefined;
    } while (cursor);
    selectedModels.clear();
    for (const [id, model] of nextSelection) selectedModels.set(id, model);
    renderModels(loadedModels);
  } catch (error) {
    status.textContent = String((error as { message?: unknown }).message ?? error);
  } finally {
    selectFiltered.disabled = false;
  }
}

async function showBulkMetadata(): Promise<void> {
  const snapshot = [...selectedModels.values()];
  if (!snapshot.length) return;
  const dialog = document.createElement("dialog");
  dialog.className = "browser-metadata-dialog";
  const title = document.createElement("h2");
  title.textContent = `Edit ${snapshot.length} selected models`;
  const form = document.createElement("form");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  dialog.append(title, form, message);
  document.body.append(dialog);
  dialog.showModal();
  const [folders, tags, facets, filaments] = await Promise.all([
    client.listFolders(),
    client.listTags(),
    client.catalogFacets(),
    client.listFilaments(true),
  ]);
  const applyStatus = document.createElement("input");
  applyStatus.type = "checkbox";
  const printStatus = document.createElement("select");
  for (const value of ["unprinted", "want", "queued", "printing", "printed", "failed"])
    printStatus.add(new Option(value, value));
  const applyRating = document.createElement("input");
  applyRating.type = "checkbox";
  const rating = document.createElement("input");
  rating.type = "number";
  rating.min = "0";
  rating.max = "5";
  rating.step = "1";
  const applyFavorite = document.createElement("input");
  applyFavorite.type = "checkbox";
  const favorite = document.createElement("input");
  favorite.type = "checkbox";
  const applyFolder = document.createElement("input");
  applyFolder.type = "checkbox";
  const folder = document.createElement("select");
  folder.add(new Option("Root", "root"));
  for (const item of folders) folder.add(new Option(item.name, item.id));
  const applyParent = document.createElement("input");
  applyParent.type = "checkbox";
  const parent = document.createElement("select");
  parent.add(new Option("Clear parent", ""));
  for (const item of facets.parents) parent.add(new Option(item.label, item.id));
  const applyTags = document.createElement("input");
  applyTags.type = "checkbox";
  const tagBox = document.createElement("fieldset");
  for (const item of tags) {
    const label = document.createElement("label");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.value = item.id;
    label.append(input, document.createTextNode(item.name));
    tagBox.append(label);
  }
  const row = (apply: HTMLInputElement, text: string, control: HTMLElement) => {
    const label = document.createElement("label");
    label.append(apply, document.createTextNode(` Apply ${text}`), control);
    form.append(label);
  };
  row(applyStatus, "print status", printStatus);
  row(applyRating, "rating", rating);
  row(applyFavorite, "favorite", favorite);
  row(applyFolder, "folder", folder);
  row(applyParent, "parent model", parent);
  row(applyTags, "tags", tagBox);
  const targets = browserSelectionTargets(snapshot);
  const save = actionButton("Apply metadata", () => {
    const patch: import("./lib/library-client").BrowserMetadataPatch = {};
    if (applyStatus.checked)
      patch.printStatus = printStatus.value as import("./lib/library-client").PrintStatus;
    if (applyRating.checked) {
      const value = Number(rating.value);
      if (!Number.isInteger(value) || value < 0 || value > 5) {
        message.textContent = "Rating must be a whole number from 0 to 5.";
        return;
      }
      patch.rating = value;
    }
    if (applyFavorite.checked) patch.favorite = favorite.checked;
    if (applyFolder.checked) patch.folderId = folder.value;
    if (applyParent.checked) patch.parentModelId = parent.value;
    if (applyTags.checked) {
      patch.tagIds = [...tagBox.querySelectorAll<HTMLInputElement>("input:checked")].map(
        (input) => input.value,
      );
      patch.tagMode = "replace";
    }
    if (!Object.keys(patch).length) {
      message.textContent = "Choose at least one metadata field.";
      return;
    }
    save.disabled = true;
    void client
      .bulkUpdateMetadata(targets, patch)
      .then(async () => {
        dialog.close();
        selectedModels.clear();
        await reloadLoadedPages();
      })
      .catch((error) => {
        message.textContent = String(error?.message ?? error);
        save.disabled = false;
      });
  });
  const filamentBox = document.createElement("select");
  filamentBox.multiple = true;
  filamentBox.setAttribute("aria-label", "Filaments for selected models");
  for (const item of filaments)
    filamentBox.add(new Option(`${item.brand} ${item.name || item.material}`, item.id));
  const assign = actionButton("Assign filaments", () => {
    const ids = [...filamentBox.selectedOptions].map((option) => option.value);
    if (ids.length > 5) {
      message.textContent = "Choose no more than five filaments.";
      return;
    }
    assign.disabled = true;
    void client
      .assignFilaments(targets.map((target) => ({ target, filamentIds: ids })))
      .then(async () => {
        dialog.close();
        selectedModels.clear();
        await reloadLoadedPages();
      })
      .catch((error) => {
        message.textContent = String(error?.message ?? error);
        assign.disabled = false;
      });
  });
  const historyDate = document.createElement("input");
  historyDate.type = "date";
  historyDate.setAttribute("aria-label", "Batch print date");
  const today = new Date();
  historyDate.value = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
  const historyQuantity = document.createElement("input");
  historyQuantity.type = "number";
  historyQuantity.min = "1";
  historyQuantity.value = "1";
  historyQuantity.setAttribute("aria-label", "Batch print quantity per model");
  const historyOutcome = document.createElement("select");
  historyOutcome.add(new Option("Printed", "printed"));
  historyOutcome.add(new Option("Failed", "failed"));
  historyOutcome.setAttribute("aria-label", "Batch print outcome");
  const historyNotes = document.createElement("textarea");
  historyNotes.setAttribute("aria-label", "Batch print notes");
  const logBatch = actionButton("Log print for selected", () => {
    logBatch.disabled = true;
    const items = snapshot.map((model) => ({
      modelId: model.id,
      input: {
        printedAt: `${historyDate.value}T12:00:00.000Z`,
        quantity: Number(historyQuantity.value),
        notes: historyNotes.value,
        filamentIds: model.filamentIds.slice(0, 5),
        outcome: historyOutcome.value as "printed" | "failed",
      },
    }));
    void client
      .createHistoryBatch(crypto.randomUUID(), items)
      .then(async () => {
        dialog.close();
        selectedModels.clear();
        await reloadLoadedPages();
      })
      .catch((error) => {
        message.textContent = String(error?.message ?? error);
        logBatch.disabled = false;
      });
  });
  logBatch.hidden = !historyBatchRoutesAvailable;
  form.append(
    save,
    filamentBox,
    assign,
    historyDate,
    historyQuantity,
    historyOutcome,
    historyNotes,
    logBatch,
    actionButton("Cancel", () => dialog.close()),
  );
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showEmbeddedMetadata(model: BrowserModel): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-metadata-dialog";
  const title = document.createElement("h2");
  title.textContent = `Embedded metadata · ${model.displayName}`;
  const form = document.createElement("form");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  dialog.append(title, form, message);
  document.body.append(dialog);
  dialog.showModal();
  try {
    const proposal = await client.embeddedMetadata(model.id, model.contentHash);
    const selection = { designer: false, sourceUrl: false, notes: false, modelLicense: false };
    for (const key of Object.keys(selection) as Array<keyof typeof selection>) {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "checkbox";
      input.disabled = !proposal.embedded[key];
      input.addEventListener("change", () => (selection[key] = input.checked));
      label.append(
        input,
        document.createTextNode(
          `${key}: ${proposal.current[key] ?? "empty"} → ${proposal.embedded[key] ?? "not embedded"}`,
        ),
      );
      form.append(label);
    }
    const apply = actionButton("Apply selected fields", () => {
      if (!Object.values(selection).some(Boolean)) {
        message.textContent = "Choose at least one embedded field to apply.";
        return;
      }
      apply.disabled = true;
      void client
        .applyEmbeddedMetadata(model.id, model.metadataVersion, proposal.sourceHash, selection)
        .then(async () => {
          dialog.close();
          await reloadLoadedPages();
        })
        .catch((error) => {
          message.textContent = String(error?.message ?? error);
          apply.disabled = false;
        });
    });
    form.append(
      apply,
      actionButton("Cancel", () => dialog.close()),
    );
  } catch (error) {
    message.textContent = String((error as { message?: unknown }).message ?? error);
  }
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showDuplicates(): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "browser-catalog-manager";
  const title = document.createElement("h2");
  title.textContent = "Duplicates in loaded models";
  const list = document.createElement("div");
  const close = actionButton("Close", () => dialog.close());
  dialog.append(title, list, close);
  document.body.append(dialog);
  dialog.showModal();
  try {
    const groups = await client.duplicateGroups(loadedModels.map((model) => model.id));
    for (const group of groups) {
      const row = document.createElement("article");
      const hash = document.createElement("strong");
      hash.textContent = group.contentHash;
      const names = document.createElement("span");
      names.textContent = group.models.map((model) => model.displayName).join(", ");
      row.append(hash, names);
      list.append(row);
    }
    if (!groups.length) list.textContent = "No duplicate groups in the loaded models.";
  } catch (error) {
    list.textContent = String((error as { message?: unknown }).message ?? error);
  }
  dialog.addEventListener("close", () => dialog.remove(), { once: true });
}

async function showModelViewer(model: BrowserModel): Promise<void> {
  const dialog = document.createElement("dialog");
  dialog.className = "library-bundle-dialog";
  const heading = document.createElement("h2");
  heading.textContent = model.displayName;
  const plate = document.createElement("select");
  plate.setAttribute("aria-label", "Plate to preview");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const canvas = document.createElement("div");
  canvas.className = "library-bundle-canvas";
  const close = actionButton("Close", () => dialog.close());
  dialog.append(heading, plate, message, canvas, close);
  document.body.append(dialog);
  dialog.showModal();
  close.focus();
  const { Viewport } = await import("./lib/viewport");
  const viewport = new Viewport(canvas);
  const controller = new AbortController();
  let revision = 0;
  const load = async (plateId: number | null) => {
    const request = ++revision;
    message.textContent = "Loading 3D preview…";
    try {
      const lease = await client.issueModelGeometry(
        model.id,
        model.contentHash,
        plateId,
        controller.signal,
      );
      const geometry = decodeGeometryBuffer(
        await (await client.contentResponse(lease.content.id, controller.signal)).arrayBuffer(),
      );
      if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
      if (!plate.options.length && lease.plates.length > 1) {
        plate.add(new Option("All geometry", ""));
        for (const item of lease.plates) plate.add(new Option(item.name, String(item.id)));
      }
      const min: [number, number, number] = [Infinity, Infinity, Infinity];
      const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
      for (let index = 0; index < geometry.positions.length; index += 3)
        for (let axis = 0; axis < 3; axis += 1) {
          min[axis] = Math.min(min[axis], geometry.positions[index + axis]);
          max[axis] = Math.max(max[axis], geometry.positions[index + axis]);
        }
      const parts = lease.parts.map((item) => ({
        id: item.id,
        name: item.name,
        triangle_start: item.triangleStart,
        triangle_count: item.triangleCount,
        material_id: item.materialId,
        filament_slot: item.filamentSlot,
      }));
      viewport.loadMesh({
        id: plateId ?? 0,
        name: model.displayName,
        geometry,
        stats: geometry.positions.length
          ? {
              bbox: { min, max },
              dimensions: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
              surface_area_mm2: 0,
              volume_mm3: 0,
            }
          : null,
        parts,
      });
      const material = new Map(lease.materials.map((item) => [item.id, item.color]));
      const filament = new Map(lease.embeddedFilaments.map((item) => [item.slot, item.color]));
      const partMap = new Map(lease.parts.map((item) => [item.id, item]));
      viewport.setColorway("color", (target) => {
        const part = target.kind === "part" ? partMap.get(target.id) : null;
        const color = part
          ? ((part.materialId ? material.get(part.materialId) : null) ??
            (part.filamentSlot ? filament.get(part.filamentSlot) : null))
          : filament.get(1);
        return {
          color: color && /^#[0-9a-f]{6}$/i.test(color) ? color : null,
          source: color ? "Embedded 3MF" : "GoggleLab default",
        };
      });
      message.textContent = `${geometry.triangleCount.toLocaleString()} triangles${lease.partWarning ? ` · ${lease.partWarning}` : ""}${lease.plateWarning ? ` · ${lease.plateWarning}` : ""}`;
    } catch (error) {
      if (isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected))
        message.textContent = String((error as { message?: unknown }).message ?? error);
    }
  };
  plate.addEventListener("change", () => void load(plate.value ? Number(plate.value) : null));
  dialog.addEventListener(
    "close",
    () => {
      revision += 1;
      controller.abort();
      viewport.dispose();
      dialog.remove();
    },
    { once: true },
  );
  await load(null);
}

async function renderCardThumbnail(
  model: BrowserModel,
  card: HTMLElement,
  retried = false,
): Promise<void> {
  if (!model.thumbnailId) return;
  const key = `${model.id}:${model.thumbnailId}`;
  card.dataset.thumbnailKey = key;
  const preview = card.querySelector<HTMLElement>(".browser-card-preview");
  if (!preview) return;
  const generation = thumbnailGeneration;
  const signal = thumbnailController.signal;
  try {
    let cached = thumbnailCache.get(key);
    let url = cached?.url;
    if (cached) {
      thumbnailCache.delete(key);
      thumbnailCache.set(key, cached);
    }
    if (!url) {
      let pending = thumbnailPending.get(key);
      if (!pending) {
        pending = thumbnailQueue.run(async () => {
          if (!isThumbnailRequestCurrent(generation, thumbnailGeneration, signal.aborted))
            throw new DOMException("Thumbnail request superseded", "AbortError");
          const lease = await client.issueThumbnailMedia(model.id, model.thumbnailId!, signal);
          const blob = await client.readContent(lease.id, lease.mime, signal);
          if (!isThumbnailRequestCurrent(generation, thumbnailGeneration, signal.aborted))
            throw new DOMException("Thumbnail request superseded", "AbortError");
          const objectUrl = URL.createObjectURL(blob);
          thumbnailCache.set(key, { url: objectUrl, bytes: blob.size });
          evictThumbnailCache();
          return objectUrl;
        });
        thumbnailPending.set(key, pending);
        void pending.then(
          () => {
            if (thumbnailPending.get(key) === pending) thumbnailPending.delete(key);
          },
          () => {
            if (thumbnailPending.get(key) === pending) thumbnailPending.delete(key);
          },
        );
      }
      url = await pending;
    }
    if (
      !isThumbnailRequestCurrent(generation, thumbnailGeneration, signal.aborted) ||
      !card.isConnected ||
      card.dataset.thumbnailKey !== key
    )
      return;
    const image = document.createElement("img");
    image.alt = "";
    image.src = url;
    preview.replaceChildren(image);
  } catch (error) {
    if (
      shouldRetryStaleThumbnail(
        retried,
        error instanceof LibraryHttpError ? error.code : undefined,
        isThumbnailRequestCurrent(generation, thumbnailGeneration, signal.aborted),
      )
    ) {
      try {
        const current = await client.getModel(model.id, signal);
        const index = loadedModels.findIndex((candidate) => candidate.id === current.id);
        if (index >= 0) loadedModels[index] = current;
        if (current.thumbnailId && current.thumbnailId !== model.thumbnailId) {
          await renderCardThumbnail(current, card, true);
          return;
        }
      } catch {}
    }
    if (card.isConnected && card.dataset.thumbnailKey === key)
      preview.textContent = "Preview unavailable";
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
          await reloadLoadedPages();
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
  let editingAttempt: import("./lib/library-client").BrowserPrintAttempt | null = null;
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
          actionButton("Edit", () => {
            editingAttempt = attempt;
            date.value = attempt.printedAt.slice(0, 10);
            quantity.value = String(attempt.quantity);
            notes.value = attempt.notes;
            outcome.value = attempt.outcome;
            save.textContent = "Save print";
            date.focus();
          }),
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
    const input = {
      printedAt: `${date.value}T12:00:00.000Z`,
      quantity: Number(quantity.value),
      notes: notes.value,
      filamentIds: model.filamentIds.slice(0, 5),
      outcome: outcome.value as "printed" | "failed",
    };
    const operation = editingAttempt
      ? client.updateHistory(editingAttempt.id, input)
      : client.createHistory(model.id, input);
    void operation
      .then(async () => {
        await reloadLoadedPages();
        if (!isBrowserDialogCurrent(request, revision, dialog.open, dialog.isConnected)) return;
        notes.value = "";
        editingAttempt = null;
        save.textContent = "Log print";
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
  const rating = document.createElement("input");
  rating.type = "number";
  rating.min = "0";
  rating.max = "5";
  rating.step = "1";
  rating.value = String(model.rating);
  const ratingLabel = document.createElement("label");
  ratingLabel.textContent = "Rating (0–5 stars)";
  ratingLabel.append(rating);
  const favorite = document.createElement("input");
  favorite.type = "checkbox";
  favorite.checked = model.favorite;
  const favoriteLabel = document.createElement("label");
  favoriteLabel.append(favorite, document.createTextNode(" Favorite"));
  form.append(ratingLabel, favoriteLabel);
  const statusSelect = document.createElement("select");
  statusSelect.setAttribute("aria-label", "Print status");
  for (const value of ["unprinted", "want", "queued", "printing", "printed", "failed"]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    statusSelect.append(option);
  }
  statusSelect.value = model.printStatus;
  const folderSelect = document.createElement("select");
  folderSelect.setAttribute("aria-label", "Folder");
  const parentSelect = document.createElement("select");
  parentSelect.setAttribute("aria-label", "Parent model");
  form.append(statusSelect, folderSelect, parentSelect);
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
    const [knownTags, folders, facets] = await Promise.all([
      client.listTags(),
      client.listFolders(),
      client.catalogFacets(),
    ]);
    folderSelect.add(new Option("Root", "root"));
    for (const folder of folders) folderSelect.add(new Option(folder.name, folder.id));
    folderSelect.value = model.folderId;
    parentSelect.add(new Option("No parent", ""));
    for (const parent of facets.parents)
      if (parent.id !== model.id) parentSelect.add(new Option(parent.label, parent.id));
    parentSelect.value = model.parentModelId ?? "";
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
        folderId: folderSelect.value,
        parentModelId: parentSelect.value,
        rating: Number(rating.value),
        favorite: favorite.checked,
        tagIds,
        tagMode: "replace",
      })
      .then(async () => {
        dialog.close();
        await reloadLoadedPages();
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
      await reloadLoadedPages();
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

async function loadModels(reset = true): Promise<void> {
  const revision = ++modelLoadRevision;
  if (reset) {
    modelController?.abort();
    modelController = new AbortController();
    nextModelCursor = null;
    thumbnailGeneration += 1;
    thumbnailController.abort();
    thumbnailController = new AbortController();
    thumbnailQueue.cancelPending();
    thumbnailPending.clear();
  }
  modelController ??= new AbortController();
  status.textContent = "Loading models…";
  try {
    const page = await client.listModels({
      ...activeQuery(reset ? undefined : (nextModelCursor ?? undefined)),
      signal: modelController.signal,
    });
    if (revision !== modelLoadRevision) return;
    loadedModels = reset ? page.items : [...loadedModels, ...page.items];
    nextModelCursor = page.nextCursor;
    renderModels(loadedModels);
    loadMore.hidden = !nextModelCursor;
    loadMore.disabled = false;
    status.textContent = `${loadedModels.length} models loaded`;
    status.title = `Catalog revision ${page.snapshotRevision}`;
  } catch (error) {
    if (revision !== modelLoadRevision || modelController.signal.aborted) return;
    status.textContent = String((error as { message?: unknown }).message ?? error);
  }
}

async function reloadLoadedPages(): Promise<void> {
  const pageSize = Math.min(capabilities?.limits.pageSizeMax ?? 50, 100);
  const pages = pagesToRestore(loadedModels.length, pageSize);
  await loadModels(true);
  for (let page = 1; page < pages && nextModelCursor; page += 1) await loadModels(false);
}

async function watchEvents(controller: AbortController): Promise<void> {
  let lastEventId: string | undefined;
  while (!controller.signal.aborted) {
    try {
      for await (const event of client.events(controller.signal, lastEventId)) {
        lastEventId = event.sequence;
        if (event.type === "catalog.changed" || event.type === "library.reset")
          await reloadLoadedPages();
        if (event.type === "server.shutdown") hostStatus.textContent = "Server shutting down";
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      status.textContent = `Live updates paused; polling while reconnecting: ${String(error)}`;
      await reloadLoadedPages();
    }
    await new Promise((resolve) => window.setTimeout(resolve, 5000));
  }
}

async function configureCatalogFilters(paths: Record<string, unknown>): Promise<void> {
  const [folders, tags, filaments] = await Promise.all([
    "/api/v1/folders" in paths ? client.listFolders() : Promise.resolve([]),
    "/api/v1/tags" in paths ? client.listTags() : Promise.resolve([]),
    "/api/v1/filaments" in paths ? client.listFilaments(true) : Promise.resolve([]),
  ]);
  fillFilter(
    folderFilter,
    "All folders",
    folders.map((item) => [item.id, item.name]),
  );
  fillFilter(
    tagFilter,
    "All tags",
    tags.map((item) => [item.id, item.category ? `${item.name} · ${item.category}` : item.name]),
  );
  fillFilter(
    filamentFilter,
    "All filaments",
    filaments.map((item) => [item.id, `${item.brand} ${item.name || item.material}`]),
  );
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
      historyBatchRoutesAvailable = "/api/v1/history/bulk" in openApi.paths;
      filamentRoutesAvailable =
        "/api/v1/models/filaments/bulk" in openApi.paths && "/api/v1/filaments" in openApi.paths;
      scanRoutesAvailable = "/api/v1/scan" in openApi.paths && "/api/v1/roots" in openApi.paths;
      captureSummaryRoutesAvailable = "/api/v1/captures" in openApi.paths;
      bundleRoutesAvailable =
        "/api/v1/bundles/{bundleId}/preview" in openApi.paths &&
        "/api/v1/bundle-previews/{token}/geometry" in openApi.paths &&
        "/api/v1/bundle-previews/{token}" in openApi.paths;
      geometryRoutesAvailable = "/api/v1/models/{modelId}/geometry-lease" in openApi.paths;
      embeddedRoutesAvailable =
        "/api/v1/models/{modelId}/embedded-metadata/proposal" in openApi.paths &&
        "/api/v1/models/{modelId}/embedded-metadata/apply" in openApi.paths;
      duplicateRoutesAvailable = "/api/v1/duplicates/query" in openApi.paths;
      reviewDuplicates.hidden = !duplicateRoutesAvailable;
      catalogSettingsRoutesAvailable =
        "/api/v1/settings/roots" in openApi.paths && "/api/v1/settings/catalog" in openApi.paths;
      browserSettings.hidden = !catalogSettingsRoutesAvailable;
      managementRoutesAvailable =
        "/api/v1/management/actions/plan" in openApi.paths &&
        "/api/v1/management/actions/execute" in openApi.paths;
      browserManagement.hidden = !managementRoutesAvailable;
      integrationRoutesAvailable = "/api/v1/integrations/settings" in openApi.paths;
      browserIntegrations.hidden = !integrationRoutesAvailable;
      thumbnailJobRoutesAvailable = "/api/v1/thumbnails/jobs" in openApi.paths;
      generateSelected.hidden = !thumbnailJobRoutesAvailable;
      regenerateSelected.hidden = !thumbnailJobRoutesAvailable;
      capture.hidden = !(capabilities.features.captureInbox && captureRoutesAvailable);
      viewCaptures.hidden = !captureSummaryRoutesAvailable;
      scanPanel.hidden = !scanRoutesAvailable;
      hostStatus.textContent = "Connected to server";
      hostStatus.title = `${capabilities.hostMode} host · library epoch ${capabilities.libraryEpoch}`;
      disconnect.disabled = false;
      for (const control of catalogControls) control.disabled = false;
      await configureCatalogFilters(openApi.paths);
      if (scanRoutesAvailable) {
        const roots = await client.listRoots();
        fillFilter(
          scanRoot,
          "All indexed roots",
          roots
            .filter((item) => item.catalogRootId)
            .map((item) => [item.catalogRootId!, `${item.label} · ${item.access}`]),
        );
        fillFilter(
          rootFilter,
          "All indexed roots",
          roots
            .filter((item) => item.catalogRootId)
            .map((item) => [item.catalogRootId!, item.label]),
        );
        await refreshScanState();
      }
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
  historyBatchRoutesAvailable = false;
  filamentRoutesAvailable = false;
  scanRoutesAvailable = false;
  captureSummaryRoutesAvailable = false;
  bundleRoutesAvailable = false;
  geometryRoutesAvailable = false;
  embeddedRoutesAvailable = false;
  duplicateRoutesAvailable = false;
  catalogSettingsRoutesAvailable = false;
  managementRoutesAvailable = false;
  integrationRoutesAvailable = false;
  void integrationDialog.dismiss();
  thumbnailJobRoutesAvailable = false;
  void bundleDialog?.dismiss();
  retryCaptureItemId = null;
  capture.hidden = true;
  scanPanel.hidden = true;
  if (scanPoll !== null) window.clearTimeout(scanPoll);
  scanPoll = null;
  hostStatus.textContent = "Disconnected";
  hostStatus.removeAttribute("title");
  disconnect.disabled = true;
  for (const control of catalogControls) control.disabled = true;
  models.replaceChildren();
  thumbnailObserver?.disconnect();
  thumbnailGeneration += 1;
  thumbnailController.abort();
  thumbnailController = new AbortController();
  thumbnailQueue.cancelPending();
  for (const entry of thumbnailCache.values()) URL.revokeObjectURL(entry.url);
  thumbnailCache.clear();
  thumbnailPending.clear();
  loadedModels = [];
  selectedModels.clear();
  updateSelectionStatus();
  nextModelCursor = null;
  loadMore.hidden = true;
  status.textContent = "Token cleared from memory.";
  token.focus();
});
refresh.addEventListener("click", () => void loadModels());
search.addEventListener("input", () => void loadModels());
for (const control of [
  sort,
  direction,
  folderFilter,
  tagFilter,
  filamentFilter,
  statusFilter,
  sourceFilter,
  groupFilter,
  availabilityFilter,
  printedFilter,
  thumbnailFilter,
  ratingFilter,
  favoriteFilter,
])
  control.addEventListener("change", () => void loadModels());
for (const control of [categoryFilter, designerFilter, licenseFilter])
  control.addEventListener("input", () => void loadModels());
directoryFilter.addEventListener("input", () => void loadModels());
duplicatesOnly.addEventListener("change", () => void loadModels());
viewMode.addEventListener("change", () => {
  models.dataset.view = viewMode.value || "gallery";
});
loadMore.addEventListener("click", () => {
  loadMore.disabled = true;
  void loadModels(false);
});
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
    if (retryCaptureItemId && envelope.itemId !== retryCaptureItemId) {
      throw new Error("Choose the original queue file for the selected capture item");
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
    retryCaptureItemId = null;
    status.textContent = `Capture ${result.status}: ${result.downloads.length} download records processed.`;
    await reloadLoadedPages();
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
