import type {
  CatalogSettings,
  IndexRoot,
  LibraryFolder,
  LibraryModel,
  LibraryScanStatus,
  LibraryTag,
  MetadataPatch,
  PrintAttempt,
  PrintAttemptInput,
  PrintStatus,
  VersionedModelTarget,
  VersionedRemovalTarget,
} from "./ipc";
import {
  LibraryMetadataEditor,
  matchesPrintedFilter,
  selectionScopeIds,
  type PrintedFilter,
  type SelectionScope,
} from "./library-editor";
import { LibraryPrintHistory } from "./library-history";
import {
  chooseRouletteModel,
  explicitScopeIds,
  partitionBundleGroups,
  removalTargets,
} from "./library-management";
import { LibraryTagManager } from "./tag-manager";

export type LibrarySort = "name" | "size" | "date";
export type LibrarySourceFilter = "all" | "managed" | "indexed";
export type LibraryStatusFilter = "all" | "available" | "missing";
export type LibraryViewMode = "list" | "preview" | "detailed";
export type LibraryGroupFilter = "all" | "standalone" | "bundle" | "zip";
export const LIBRARY_CATALOG_TIER = "free" as const;
export const LIBRARY_VIEW_MODES: readonly LibraryViewMode[] = ["list", "preview", "detailed"];

const DEFAULT_CATALOG_SETTINGS: CatalogSettings = {
  thumbnail_size: 256,
  thumbnail_background: "#12171F",
  thumbnail_concurrency: 2,
  scan_batch_size: 128,
};

export interface LibraryViewOptions {
  query: string;
  folderId: string | null;
  rootId: string | null;
  source: LibrarySourceFilter;
  status: LibraryStatusFilter;
  sort: LibrarySort;
}

export interface LibraryMetadataFilters {
  printStatus: "all" | PrintStatus;
  designer: string;
  tagId: string;
  category: string;
  parentId: string;
  license: string;
}

export interface LibraryDirectoryNode {
  prefix: string;
  name: string;
  depth: number;
  count: number;
}

export function clampLibraryNavigationWidth(width: number): number {
  return Math.min(420, Math.max(200, Math.round(width)));
}

export function nextLibraryNavigationWidth(current: number, key: string): number {
  if (key === "Home") return 200;
  if (key === "End") return 420;
  if (key === "ArrowLeft") return clampLibraryNavigationWidth(current - 16);
  if (key === "ArrowRight") return clampLibraryNavigationWidth(current + 16);
  return clampLibraryNavigationWidth(current);
}

function normalizedCatalogPath(path: string): string {
  return path.replaceAll("\\", "/").replace(/^\/+|\/+$/g, "");
}

export function indexedDirectoryNodes(
  models: LibraryModel[],
  rootId: string,
): LibraryDirectoryNode[] {
  const counts = new Map<string, number>();
  for (const model of models) {
    if (model.source_root_id !== rootId || !model.source_path) continue;
    const parts = normalizedCatalogPath(model.source_path).split("/").filter(Boolean);
    parts.pop();
    for (let index = 1; index <= parts.length; index += 1) {
      const prefix = parts.slice(0, index).join("/");
      counts.set(prefix, (counts.get(prefix) ?? 0) + 1);
    }
  }
  return [...counts]
    .map(([prefix, count]) => {
      const parts = prefix.split("/");
      return { prefix, count, name: parts.at(-1) ?? prefix, depth: parts.length - 1 };
    })
    .sort((a, b) => a.prefix.localeCompare(b.prefix, undefined, { sensitivity: "base" }));
}

export function matchesIndexedDirectory(
  model: LibraryModel,
  rootId: string | null,
  prefix: string | null,
): boolean {
  if (!rootId || !prefix) return true;
  if (model.source_root_id !== rootId || !model.source_path) return false;
  const path = normalizedCatalogPath(model.source_path);
  const normalizedPrefix = normalizedCatalogPath(prefix);
  return path.startsWith(`${normalizedPrefix}/`);
}

const PAGE_SIZE = 36;
const THUMBNAIL_CONCURRENCY = 4;

export function formatLibraryBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function libraryFolderDepths(folders: LibraryFolder[]): Map<string, number> {
  const depths = new Map<string, number>();
  const byId = new Map(folders.map((folder) => [folder.id, folder]));
  for (const folder of folders) {
    let depth = 0;
    let current = folder;
    const visited = new Set<string>();
    while (current.parent_id) {
      if (visited.has(current.id) || visited.has(current.parent_id)) {
        depth = 0;
        break;
      }
      visited.add(current.id);
      const parent = byId.get(current.parent_id);
      if (!parent) break;
      depth += 1;
      current = parent;
    }
    depths.set(folder.id, depth);
  }
  return depths;
}

function modelSource(model: LibraryModel): "managed" | "indexed" {
  return model.source_kind ?? "managed";
}

export function normalizeLibraryTimestamp(value: number | string | null | undefined): number {
  if (value === null || value === undefined || value === "") return 0;
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    if (numeric > 1_000_000_000_000_000) return numeric / 1_000_000;
    return numeric < 1_000_000_000_000 ? numeric * 1000 : numeric;
  }
  return typeof value === "string" ? Date.parse(value) || 0 : 0;
}

function modelDate(model: LibraryModel): number {
  return normalizeLibraryTimestamp(model.source_modified_at ?? model.updated_at);
}

export function filterAndSortLibraryModels(
  models: LibraryModel[],
  options: LibraryViewOptions,
): LibraryModel[] {
  const query = options.query.trim().toLocaleLowerCase();
  return models
    .filter((model) => {
      if (options.folderId && model.folder_id !== options.folderId) return false;
      if (options.rootId && model.source_root_id !== options.rootId) return false;
      if (options.source !== "all" && modelSource(model) !== options.source) return false;
      if (options.status === "missing" && !model.source_missing) return false;
      if (options.status === "available" && model.source_missing) return false;
      if (!query) return true;
      return [
        model.display_name,
        model.original_name ?? "",
        model.description,
        model.source_path ?? "",
        model.format ?? "",
        model.metadata?.designer ?? "",
        model.metadata?.source_url ?? "",
        model.metadata?.notes ?? "",
        model.metadata?.model_license ?? "",
        model.metadata?.print_status ?? "",
        ...model.tags,
      ]
        .join(" ")
        .toLocaleLowerCase()
        .includes(query);
    })
    .sort((a, b) => {
      if (options.sort === "size") {
        return b.bytes - a.bytes || a.display_name.localeCompare(b.display_name);
      }
      if (options.sort === "date") {
        return modelDate(b) - modelDate(a) || a.display_name.localeCompare(b.display_name);
      }
      return a.display_name.localeCompare(b.display_name, undefined, {
        numeric: true,
        sensitivity: "base",
      });
    });
}

export function scanStatusMessage(status: LibraryScanStatus): string {
  if (status.running) {
    if (status.jobKind === "thumbnails") {
      const action = status.cancelling ? "Stopping preview generation" : "Generating previews";
      return `${action} · ${status.thumbnailsCompleted.toLocaleString()}/${status.thumbnailsTotal.toLocaleString()} complete`;
    }
    const action = status.cancelling ? "Stopping scan" : "Scanning";
    const roots = status.rootsTotal
      ? ` · ${status.rootsCompleted}/${status.rootsTotal} folders`
      : "";
    return `${action}${roots} · ${status.filesVisited.toLocaleString()} checked · ${status.filesIndexed.toLocaleString()} indexed`;
  }
  if (status.jobKind === "thumbnails" && status.finishedAt) {
    return `Preview generation finished · ${status.thumbnailsCompleted.toLocaleString()}/${status.thumbnailsTotal.toLocaleString()} complete`;
  }
  if (!status.finishedAt) return "Ready to scan indexed folders.";
  const errors = status.errors.length
    ? ` · ${status.errors.length} issue${status.errors.length === 1 ? "" : "s"}`
    : "";
  return `Scan finished · ${status.filesIndexed.toLocaleString()} indexed${errors}`;
}

export function normalizeCatalogSettings(settings: CatalogSettings): CatalogSettings {
  const thumbnailSize = [128, 256, 512].includes(settings.thumbnail_size)
    ? settings.thumbnail_size
    : 256;
  const concurrency = Number.isFinite(settings.thumbnail_concurrency)
    ? Math.round(settings.thumbnail_concurrency)
    : DEFAULT_CATALOG_SETTINGS.thumbnail_concurrency;
  const batchSize = Number.isFinite(settings.scan_batch_size)
    ? Math.round(settings.scan_batch_size)
    : DEFAULT_CATALOG_SETTINGS.scan_batch_size;
  return {
    thumbnail_size: thumbnailSize as 128 | 256 | 512,
    thumbnail_background: /^#[0-9a-f]{6}$/i.test(settings.thumbnail_background)
      ? settings.thumbnail_background.toUpperCase()
      : DEFAULT_CATALOG_SETTINGS.thumbnail_background,
    thumbnail_concurrency: Math.min(2, Math.max(1, concurrency)),
    scan_batch_size: Math.min(256, Math.max(16, batchSize)),
  };
}

export function canOpenLibraryModel(model: LibraryModel): boolean {
  return !model.source_missing;
}

export function matchesLibraryGroup(model: LibraryModel, group: LibraryGroupFilter): boolean {
  if (group === "standalone") return !model.bundle_id && !model.source_archive_member;
  if (group === "bundle") return Boolean(model.bundle_id);
  if (group === "zip") return Boolean(model.source_archive_member);
  return true;
}

export function matchesLibraryMetadataFilters(
  model: LibraryModel,
  filters: LibraryMetadataFilters,
  tags: LibraryTag[],
): boolean {
  if (
    filters.printStatus !== "all" &&
    (model.metadata?.print_status ?? "unprinted") !== filters.printStatus
  ) {
    return false;
  }
  if (filters.designer && (model.metadata?.designer ?? "") !== filters.designer) return false;
  if (filters.tagId) {
    const tag = tags.find((candidate) => candidate.id === filters.tagId);
    if (!tag || !model.tags.includes(tag.name)) return false;
  }
  if (filters.category) {
    const categoryNames = new Set(
      tags.filter((tag) => (tag.category ?? "") === filters.category).map((tag) => tag.name),
    );
    if (!model.tags.some((name) => categoryNames.has(name))) return false;
  }
  const parent = model.metadata?.parent_model_id ?? "";
  if (filters.parentId === "__none__" && parent) return false;
  if (filters.parentId && filters.parentId !== "__none__" && parent !== filters.parentId)
    return false;
  if (filters.license && (model.metadata?.model_license ?? "") !== filters.license) return false;
  return true;
}

export function didLibraryScanComplete(
  wasRunning: boolean,
  status: Pick<LibraryScanStatus, "running">,
): boolean {
  return wasRunning && !status.running;
}

export function thumbnailCacheInvalidations(
  previous: LibraryModel[],
  next: LibraryModel[],
): string[] {
  const nextById = new Map(next.map((model) => [model.id, model]));
  const invalidations: string[] = [];
  for (const model of previous) {
    const replacement = nextById.get(model.id);
    if (
      !replacement ||
      model.content_hash !== replacement.content_hash ||
      model.thumbnail_path !== replacement.thumbnail_path ||
      Boolean(model.source_missing) !== Boolean(replacement.source_missing)
    ) {
      invalidations.push(model.id);
    }
  }
  return invalidations;
}

function libraryErrorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return String(error);
}

/** Guards async thumbnail writes when a card is recycled or regenerated. */
export class ThumbnailRequestGuard {
  private readonly revisions = new Map<string, number>();

  begin(modelId: string): number {
    const next = (this.revisions.get(modelId) ?? 0) + 1;
    this.revisions.set(modelId, next);
    return next;
  }

  isCurrent(modelId: string, revision: number): boolean {
    return this.revisions.get(modelId) === revision;
  }

  invalidate(modelId: string): void {
    this.begin(modelId);
  }
}

export class LibraryPanel {
  private readonly root: HTMLDialogElement;
  private readonly status: HTMLElement;
  private readonly scanStatus: HTMLElement;
  private readonly searchInput: HTMLInputElement;
  private readonly navigation: HTMLElement;
  private readonly navigationResize: HTMLElement;
  private readonly gallery: HTMLElement;
  private readonly resultCount: HTMLElement;
  private readonly sortSelect: HTMLSelectElement;
  private readonly sourceSelect: HTMLSelectElement;
  private readonly statusSelect: HTMLSelectElement;
  private readonly scanButton: HTMLButtonElement;
  private readonly cancelScanButton: HTMLButtonElement;
  private readonly importButton: HTMLButtonElement;
  private readonly addRootButton: HTMLButtonElement;
  private readonly purgeButton: HTMLButtonElement;
  private readonly generateMissingButton: HTMLButtonElement;
  private readonly regenerateAllButton: HTMLButtonElement;
  private readonly viewButtons = new Map<LibraryViewMode, HTMLButtonElement>();
  private readonly thumbnailSizeSelect: HTMLSelectElement;
  private readonly thumbnailBackgroundInput: HTMLInputElement;
  private readonly thumbnailConcurrencyInput: HTMLInputElement;
  private readonly scanBatchSizeInput: HTMLInputElement;
  private readonly printedSelect: HTMLSelectElement;
  private readonly groupSelect: HTMLSelectElement;
  private readonly printStatusSelect: HTMLSelectElement;
  private readonly designerSelect: HTMLSelectElement;
  private readonly tagSelect: HTMLSelectElement;
  private readonly categorySelect: HTMLSelectElement;
  private readonly parentSelect: HTMLSelectElement;
  private readonly licenseSelect: HTMLSelectElement;
  private readonly body: HTMLElement;
  private readonly selectionStatus: HTMLElement;
  private readonly metadataEditor: LibraryMetadataEditor;
  private readonly tagManager: LibraryTagManager;
  private readonly printHistory: LibraryPrintHistory;
  private invoker: HTMLElement | null = null;
  private allFolders: LibraryFolder[] = [];
  private allModels: LibraryModel[] = [];
  private allRoots: IndexRoot[] = [];
  private allTags: LibraryTag[] = [];
  private filteredModels: LibraryModel[] = [];
  private visibleModelIds: string[] = [];
  private readonly selectedModelIds = new Set<string>();
  private activeFolderId: string | null = null;
  private activeRootId: string | null = null;
  private activeDirectoryRootId: string | null = null;
  private activeDirectoryPrefix: string | null = null;
  private activeHistoryModelId: string | null = null;
  private viewMode: LibraryViewMode = "preview";
  private catalogSettings = DEFAULT_CATALOG_SETTINGS;
  private page = 1;
  private renderRevision = 0;
  private scanWasRunning = false;
  private pollTimer: number | undefined;
  private scanPollRevision = 0;
  private scanPollInFlight = false;
  private observer: IntersectionObserver | null = null;
  private readonly thumbnailCache = new Map<string, string | null>();
  private readonly thumbnailGuard = new ThumbnailRequestGuard();
  private readonly thumbnailQueue: Array<() => Promise<void>> = [];
  private thumbnailWorkers = 0;

  onImport: () => void = () => {};
  onOpenModel: (modelId: string) => void = () => {};
  onAddRoot: () => Promise<void> = async () => {};
  onUpdateRoot: (root: IndexRoot) => Promise<void> = async () => {};
  onRemoveRoot: (rootId: string) => Promise<void> = async () => {};
  onStartScan: (rootId: string | null) => Promise<void> = async () => {};
  onCancelScan: () => Promise<void> = async () => {};
  onPollScanStatus: () => Promise<LibraryScanStatus> = async () => ({
    running: false,
    cancelling: false,
    currentRootId: null,
    rootsTotal: 0,
    rootsCompleted: 0,
    filesVisited: 0,
    filesIndexed: 0,
    errors: [],
    startedAt: null,
    finishedAt: null,
    jobKind: null,
    thumbnailsTotal: 0,
    thumbnailsCompleted: 0,
  });
  onScanCompleted: () => void = () => {};
  onGetThumbnail: (modelId: string, regenerate: boolean) => Promise<string | null> = async () =>
    null;
  onPurgeThumbnails: () => Promise<void> = async () => {};
  onStartThumbnailJob: (regenerate: boolean) => Promise<void> = async () => {};
  onSaveCatalogSettings: (settings: CatalogSettings) => Promise<CatalogSettings> = async (
    settings,
  ) => settings;
  onSaveMetadata: (targets: VersionedModelTarget[], patch: MetadataPatch) => Promise<void> =
    async () => {};
  onCreateTag: (name: string, category: string | null) => Promise<void> = async () => {};
  onUpdateTag: (id: string, name: string, category: string | null) => Promise<void> =
    async () => {};
  onDeleteTag: (id: string) => Promise<void> = async () => {};
  onListPrintAttempts: (modelId: string) => Promise<PrintAttempt[]> = async () => [];
  onCreatePrintAttempt: (modelId: string, input: PrintAttemptInput) => Promise<void> =
    async () => {};
  onUpdatePrintAttempt: (attemptId: string, input: PrintAttemptInput) => Promise<void> =
    async () => {};
  onDeletePrintAttempt: (attemptId: string) => Promise<void> = async () => {};
  onCreateBundle: (modelIds: string[]) => Promise<void> = async () => {};
  onCreateCollection: (name: string) => Promise<void> = async () => {};
  onRenameCollection: (id: string, name: string) => Promise<void> = async () => {};
  onDeleteCollection: (id: string) => Promise<void> = async () => {};
  onReviewDuplicates: (modelIds: string[]) => Promise<void> = async () => {};
  onRemoveModels: (targets: VersionedRemovalTarget[]) => Promise<void> = async () => {};
  onRevealSource: (modelId: string) => Promise<void> = async () => {};
  onTrashLooseSource: (model: LibraryModel) => Promise<void> = async () => {};
  onChooseCustomPreview: (model: LibraryModel) => Promise<void> = async () => {};
  onClearCustomPreview: (model: LibraryModel) => Promise<void> = async () => {};
  onPreviewBundle: (bundleId: string) => Promise<void> = async () => {};
  onOpenManagement: () => void = () => {};

  constructor(container: HTMLElement) {
    this.metadataEditor = new LibraryMetadataEditor(container);
    this.tagManager = new LibraryTagManager(container);
    this.printHistory = new LibraryPrintHistory(container);
    this.metadataEditor.onSave = async (targets, patch) => {
      await this.onSaveMetadata(targets, patch);
    };
    this.metadataEditor.onManageTags = () => this.tagManager.show(this.allTags);
    this.metadataEditor.onHistory = (model) => void this.openPrintHistory(model);
    this.tagManager.onCreate = (name, category) => this.onCreateTag(name, category);
    this.tagManager.onUpdate = (id, name, category) => this.onUpdateTag(id, name, category);
    this.tagManager.onDelete = (id) => this.onDeleteTag(id);
    this.printHistory.onCreate = async (modelId, input) => {
      await this.onCreatePrintAttempt(modelId, input);
      await this.refreshPrintHistory(modelId);
    };
    this.printHistory.onUpdate = async (attemptId, input) => {
      await this.onUpdatePrintAttempt(attemptId, input);
      if (this.activeHistoryModelId) await this.refreshPrintHistory(this.activeHistoryModelId);
    };
    this.printHistory.onDelete = async (attemptId) => {
      await this.onDeletePrintAttempt(attemptId);
      if (this.activeHistoryModelId) await this.refreshPrintHistory(this.activeHistoryModelId);
    };
    this.root = document.createElement("dialog");
    this.root.className = "library-panel";
    this.root.setAttribute("aria-labelledby", "library-title");
    this.root.addEventListener("click", (event) => {
      if (event.target === this.root) this.hide();
    });
    this.root.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.hide();
    });

    const shell = document.createElement("section");
    shell.className = "library-shell";
    const header = document.createElement("header");
    header.className = "library-header";
    const heading = document.createElement("div");
    const eyebrow = document.createElement("div");
    eyebrow.className = "library-eyebrow";
    eyebrow.textContent = "Model catalog";
    const title = document.createElement("h2");
    title.id = "library-title";
    title.className = "library-title";
    title.textContent = "Library";
    const subtitle = document.createElement("p");
    subtitle.className = "library-subtitle";
    subtitle.textContent =
      "Managed imports are saved copies. Indexed folders stay in place and are read-only.";
    heading.append(eyebrow, title, subtitle);

    const actions = document.createElement("div");
    actions.className = "library-actions";
    this.importButton = this.actionButton("Import copies…", "primary", () => this.onImport());
    this.addRootButton = this.actionButton(
      "Index folder…",
      "",
      () => void this.runAction(this.onAddRoot),
    );
    const closeButton = this.actionButton("Close", "", () => this.hide());
    const managementButton = this.actionButton("Manage…", "", () => this.onOpenManagement());
    actions.append(this.importButton, this.addRootButton, managementButton, closeButton);
    header.append(heading, actions);

    this.status = document.createElement("div");
    this.status.className = "library-status hint";
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");

    const toolbar = document.createElement("div");
    toolbar.className = "library-toolbar";
    this.searchInput = document.createElement("input");
    this.searchInput.type = "search";
    this.searchInput.className = "library-search";
    this.searchInput.placeholder = "Search names, paths, formats, and tags";
    this.searchInput.setAttribute("aria-label", "Search model catalog");
    this.searchInput.addEventListener("input", () => this.resetAndRender());
    this.sourceSelect = this.selectControl("Source", [
      ["all", "All sources"],
      ["managed", "Managed copies"],
      ["indexed", "Indexed folders"],
    ]);
    this.statusSelect = this.selectControl("Availability", [
      ["all", "Any status"],
      ["available", "Available"],
      ["missing", "Missing source"],
    ]);
    this.sortSelect = this.selectControl("Sort", [
      ["name", "Name"],
      ["date", "Newest"],
      ["size", "Largest"],
    ]);
    this.printedSelect = this.selectControl("Print history", [
      ["all", "Any print history"],
      ["never", "Never printed"],
      ["ever", "Ever printed"],
      ["last30", "Printed in 30 days"],
    ]);
    this.groupSelect = this.selectControl("Model group", [
      ["all", "All groups"],
      ["standalone", "Standalone"],
      ["bundle", "Bundles"],
      ["zip", "ZIP members"],
    ]);
    this.printStatusSelect = this.selectControl("Print status", [
      ["all", "Any print status"],
      ["unprinted", "Unprinted"],
      ["want", "Want to print"],
      ["queued", "Queued"],
      ["printing", "Printing"],
      ["printed", "Printed"],
      ["failed", "Failed"],
    ]);
    this.designerSelect = this.selectControl("Designer", [["", "All designers"]]);
    this.tagSelect = this.selectControl("Tag", [["", "All tags"]]);
    this.categorySelect = this.selectControl("Tag category", [["", "All categories"]]);
    this.parentSelect = this.selectControl("Parent model", [["", "All parent relationships"]]);
    this.licenseSelect = this.selectControl("Model license", [["", "All licenses"]]);
    toolbar.append(
      this.searchInput,
      this.sourceSelect,
      this.statusSelect,
      this.sortSelect,
      this.printedSelect,
      this.groupSelect,
      this.printStatusSelect,
      this.designerSelect,
      this.tagSelect,
      this.categorySelect,
      this.parentSelect,
      this.licenseSelect,
    );

    this.body = document.createElement("div");
    this.body.className = "library-body";
    this.navigation = document.createElement("aside");
    this.navigation.className = "library-navigation";
    this.navigation.setAttribute("aria-label", "Library sources and folders");
    const content = document.createElement("main");
    content.className = "library-content";
    const contentHeader = document.createElement("div");
    contentHeader.className = "library-content-header";
    this.resultCount = document.createElement("div");
    this.resultCount.className = "library-result-count";
    const selectionActions = document.createElement("div");
    selectionActions.className = "library-selection-actions";
    this.selectionStatus = document.createElement("span");
    this.selectionStatus.setAttribute("role", "status");
    selectionActions.append(
      this.selectionStatus,
      this.actionButton("Select visible", "", () => this.selectScope("visible")),
      this.actionButton("Select all filtered", "", () => this.selectScope("filtered")),
      this.actionButton("Clear selection", "", () => {
        this.selectedModelIds.clear();
        this.renderContent();
      }),
      this.actionButton("Edit selected…", "", () => this.editSelected()),
      this.actionButton("Create bundle", "", () => void this.createBundle()),
      this.actionButton("Review duplicates", "", () => void this.reviewDuplicates()),
      this.actionButton("Remove selected", "danger", () => void this.removeSelected()),
      this.actionButton("Print roulette", "", () => this.runRoulette()),
    );
    const viewSwitcher = document.createElement("div");
    viewSwitcher.className = "library-view-switcher";
    viewSwitcher.setAttribute("role", "group");
    viewSwitcher.setAttribute("aria-label", "Catalog view");
    for (const [mode, label] of [
      ["list", "List"],
      ["preview", "Previews"],
      ["detailed", "Details"],
    ] as Array<[LibraryViewMode, string]>) {
      const button = this.actionButton(label, "", () => {
        this.viewMode = mode;
        this.page = 1;
        this.renderContent();
      });
      button.setAttribute("aria-pressed", String(mode === this.viewMode));
      this.viewButtons.set(mode, button);
      viewSwitcher.append(button);
    }
    const thumbnailActions = document.createElement("div");
    thumbnailActions.className = "library-thumbnail-actions";
    this.generateMissingButton = this.actionButton(
      "Generate missing",
      "",
      () => void this.startThumbnailJob(false),
    );
    this.regenerateAllButton = this.actionButton(
      "Regenerate all",
      "",
      () => void this.startThumbnailJob(true),
    );
    this.purgeButton = this.actionButton("Clear previews", "", () => void this.purgeThumbnails());
    thumbnailActions.append(this.generateMissingButton, this.regenerateAllButton, this.purgeButton);
    contentHeader.append(this.resultCount, viewSwitcher, thumbnailActions, selectionActions);
    this.gallery = document.createElement("div");
    this.gallery.className = "library-gallery";
    this.gallery.setAttribute("aria-live", "polite");
    content.append(contentHeader, this.gallery);
    this.navigationResize = document.createElement("div");
    this.navigationResize.className = "library-navigation-resize";
    this.navigationResize.tabIndex = 0;
    this.navigationResize.setAttribute("role", "separator");
    this.navigationResize.setAttribute("aria-label", "Resize source and collection sidebar");
    this.navigationResize.setAttribute("aria-orientation", "vertical");
    this.navigationResize.setAttribute("aria-valuemin", "200");
    this.navigationResize.setAttribute("aria-valuemax", "420");
    this.installNavigationResize(this.navigationResize);
    this.navigation.append(this.navigationResize);
    this.body.append(this.navigation, content);

    const catalogSettings = document.createElement("details");
    catalogSettings.className = "library-catalog-settings";
    const settingsSummary = document.createElement("summary");
    settingsSummary.textContent = "Catalog settings";
    const settingsFields = document.createElement("div");
    settingsFields.className = "library-settings-fields";
    this.thumbnailSizeSelect = document.createElement("select");
    this.thumbnailSizeSelect.className = "library-select";
    this.thumbnailSizeSelect.setAttribute("aria-label", "Preview size");
    for (const size of [128, 256, 512]) {
      this.thumbnailSizeSelect.add(new Option(`${size} px`, String(size)));
    }
    this.thumbnailBackgroundInput = document.createElement("input");
    this.thumbnailBackgroundInput.type = "color";
    this.thumbnailBackgroundInput.className = "library-color-input";
    this.thumbnailBackgroundInput.setAttribute("aria-label", "Preview background");
    this.thumbnailConcurrencyInput = document.createElement("input");
    this.thumbnailConcurrencyInput.type = "number";
    this.thumbnailConcurrencyInput.className = "library-number";
    this.thumbnailConcurrencyInput.min = "1";
    this.thumbnailConcurrencyInput.max = "2";
    this.thumbnailConcurrencyInput.setAttribute("aria-label", "Preview concurrency");
    this.scanBatchSizeInput = document.createElement("input");
    this.scanBatchSizeInput.type = "number";
    this.scanBatchSizeInput.className = "library-number";
    this.scanBatchSizeInput.min = "16";
    this.scanBatchSizeInput.max = "256";
    this.scanBatchSizeInput.step = "16";
    this.scanBatchSizeInput.setAttribute("aria-label", "Scan batch size");
    settingsFields.append(
      this.settingField("Preview size", this.thumbnailSizeSelect),
      this.settingField("Background", this.thumbnailBackgroundInput),
      this.settingField("Preview workers", this.thumbnailConcurrencyInput),
      this.settingField("Scan batch", this.scanBatchSizeInput),
      this.actionButton("Save settings", "", () => void this.saveCatalogSettings()),
    );
    catalogSettings.append(settingsSummary, settingsFields);

    const scanBar = document.createElement("footer");
    scanBar.className = "library-scan-bar";
    this.scanStatus = document.createElement("div");
    this.scanStatus.className = "library-scan-status";
    this.scanStatus.setAttribute("role", "status");
    this.scanButton = this.actionButton(
      "Scan enabled folders",
      "",
      () =>
        void this.runAction(async () => {
          await this.onStartScan(null);
          await this.pollScanStatus();
        }),
    );
    this.cancelScanButton = this.actionButton(
      "Cancel scan",
      "",
      () => void this.runAction(this.onCancelScan),
    );
    this.cancelScanButton.hidden = true;
    scanBar.append(this.scanStatus, this.scanButton, this.cancelScanButton);

    shell.append(header, this.status, toolbar, this.body, catalogSettings, scanBar);
    this.root.append(shell);
    container.appendChild(this.root);
  }

  get isOpen(): boolean {
    return this.root.open;
  }

  show(invoker?: HTMLElement): void {
    const active = invoker ?? document.activeElement;
    this.invoker = active instanceof HTMLElement ? active : null;
    if (!this.root.open) this.root.showModal();
    this.startPolling();
    window.setTimeout(() => this.searchInput.focus(), 0);
  }

  hide(): void {
    this.stopPolling();
    this.observer?.disconnect();
    if (this.root.open) this.root.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  dispose(): void {
    this.stopPolling();
    this.observer?.disconnect();
  }

  setStatus(message: string): void {
    this.status.textContent = message;
  }

  setImportBusy(busy: boolean): void {
    this.importButton.disabled = busy;
    this.importButton.setAttribute("aria-disabled", String(busy));
  }

  render(
    folders: LibraryFolder[],
    models: LibraryModel[],
    roots: IndexRoot[] = [],
    settings: CatalogSettings = this.catalogSettings,
    tags: LibraryTag[] = this.allTags,
  ): void {
    for (const modelId of thumbnailCacheInvalidations(this.allModels, models)) {
      this.thumbnailCache.delete(modelId);
      this.thumbnailGuard.invalidate(modelId);
    }
    this.allFolders = folders;
    this.allModels = models;
    this.allRoots = roots;
    this.allTags = tags;
    this.tagManager.update(tags);
    this.refreshMetadataFilterOptions(models, tags);
    const modelIds = new Set(models.map((model) => model.id));
    for (const id of this.selectedModelIds) if (!modelIds.has(id)) this.selectedModelIds.delete(id);
    this.setCatalogSettings(settings);
    if (this.activeFolderId && !folders.some((folder) => folder.id === this.activeFolderId)) {
      this.activeFolderId = null;
    }
    if (this.activeRootId && !roots.some((root) => root.id === this.activeRootId)) {
      this.activeRootId = null;
    }
    if (
      this.activeDirectoryRootId &&
      (!roots.some((root) => root.id === this.activeDirectoryRootId) ||
        !indexedDirectoryNodes(models, this.activeDirectoryRootId).some(
          (node) => node.prefix === this.activeDirectoryPrefix,
        ))
    ) {
      this.activeDirectoryRootId = null;
      this.activeDirectoryPrefix = null;
    }
    this.renderContent();
  }

  setScanStatus(value: LibraryScanStatus): void {
    this.scanStatus.textContent = scanStatusMessage(value);
    this.scanButton.disabled = value.running;
    this.generateMissingButton.disabled = value.running;
    this.regenerateAllButton.disabled = value.running;
    this.cancelScanButton.hidden = !value.running;
    this.cancelScanButton.disabled = value.cancelling;
    this.cancelScanButton.textContent =
      value.jobKind === "thumbnails" ? "Cancel previews" : "Cancel scan";
    if (value.errors.length) this.scanStatus.title = value.errors.join("\n");
    else this.scanStatus.removeAttribute("title");
    if (didLibraryScanComplete(this.scanWasRunning, value)) this.onScanCompleted();
    this.scanWasRunning = value.running;
  }

  clearThumbnailCache(): void {
    this.thumbnailCache.clear();
    for (const model of this.allModels) this.thumbnailGuard.invalidate(model.id);
  }

  invalidateAfterLibraryReplacement(): void {
    this.metadataEditor.dismiss();
    this.tagManager.dismiss();
    this.printHistory.dismiss();
    this.selectedModelIds.clear();
    this.activeHistoryModelId = null;
    this.clearThumbnailCache();
  }

  private actionButton(label: string, variant: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `btn${variant ? ` ${variant}` : ""}`;
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private selectControl(label: string, options: Array<[string, string]>): HTMLSelectElement {
    const select = document.createElement("select");
    select.className = "library-select";
    select.setAttribute("aria-label", label);
    for (const [value, text] of options) select.add(new Option(text, value));
    select.addEventListener("change", () => this.resetAndRender());
    return select;
  }

  private installNavigationResize(handle: HTMLElement): void {
    let startX = 0;
    let startWidth = 270;
    const apply = (width: number) => {
      const clamped = clampLibraryNavigationWidth(width);
      this.body.style.setProperty("--library-nav-width", `${clamped}px`);
      handle.setAttribute("aria-valuenow", String(clamped));
    };
    handle.addEventListener("pointerdown", (event) => {
      if (window.matchMedia("(max-width: 760px)").matches) return;
      startX = event.clientX;
      startWidth = this.navigation.getBoundingClientRect().width;
      handle.setPointerCapture(event.pointerId);
    });
    handle.addEventListener("pointermove", (event) => {
      if (!handle.hasPointerCapture(event.pointerId)) return;
      apply(startWidth + event.clientX - startX);
    });
    handle.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      apply(nextLibraryNavigationWidth(this.navigation.getBoundingClientRect().width, event.key));
    });
    apply(270);
  }

  private refreshMetadataFilterOptions(models: LibraryModel[], tags: LibraryTag[]): void {
    const designers = [
      ...new Set(models.map((model) => model.metadata?.designer?.trim() ?? "").filter(Boolean)),
    ].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
    const categories = [
      ...new Set(tags.map((tag) => tag.category?.trim() ?? "").filter(Boolean)),
    ].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
    const licenses = [
      ...new Set(
        models.map((model) => model.metadata?.model_license?.trim() ?? "").filter(Boolean),
      ),
    ].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
    this.replaceSelectOptions(this.designerSelect, [
      ["", "All designers"],
      ...designers.map((designer): [string, string] => [designer, designer]),
    ]);
    this.replaceSelectOptions(this.tagSelect, [
      ["", "All tags"],
      ...tags.map((tag): [string, string] => [
        tag.id,
        tag.category ? `${tag.category}: ${tag.name}` : tag.name,
      ]),
    ]);
    this.replaceSelectOptions(this.categorySelect, [
      ["", "All categories"],
      ...categories.map((category): [string, string] => [category, category]),
    ]);
    this.replaceSelectOptions(this.parentSelect, [
      ["", "All parent relationships"],
      ["__none__", "No parent"],
      ...models.map((model): [string, string] => [model.id, model.display_name]),
    ]);
    this.replaceSelectOptions(this.licenseSelect, [
      ["", "All licenses"],
      ...licenses.map((license): [string, string] => [license, license]),
    ]);
  }

  private replaceSelectOptions(select: HTMLSelectElement, options: Array<[string, string]>): void {
    const selected = select.value;
    select.replaceChildren();
    for (const [value, label] of options) select.add(new Option(label, value));
    select.value = options.some(([value]) => value === selected) ? selected : options[0][0];
  }

  private settingField(label: string, control: HTMLElement): HTMLLabelElement {
    const field = document.createElement("label");
    field.className = "library-setting-field";
    const caption = document.createElement("span");
    caption.textContent = label;
    field.append(caption, control);
    return field;
  }

  private setCatalogSettings(settings: CatalogSettings): void {
    this.catalogSettings = normalizeCatalogSettings(settings);
    this.thumbnailSizeSelect.value = String(this.catalogSettings.thumbnail_size);
    this.thumbnailBackgroundInput.value = this.catalogSettings.thumbnail_background;
    this.thumbnailConcurrencyInput.value = String(this.catalogSettings.thumbnail_concurrency);
    this.scanBatchSizeInput.value = String(this.catalogSettings.scan_batch_size);
  }

  private async saveCatalogSettings(): Promise<void> {
    const settings = normalizeCatalogSettings({
      thumbnail_size: Number(this.thumbnailSizeSelect.value) as 128 | 256 | 512,
      thumbnail_background: this.thumbnailBackgroundInput.value,
      thumbnail_concurrency: Number(this.thumbnailConcurrencyInput.value),
      scan_batch_size: Number(this.scanBatchSizeInput.value),
    });
    try {
      this.setCatalogSettings(await this.onSaveCatalogSettings(settings));
      this.clearThumbnailCache();
      this.setStatus("Catalog settings saved. Previews will use the new render settings.");
      this.renderContent();
    } catch (error) {
      this.setStatus(`Could not save catalog settings: ${libraryErrorMessage(error)}`);
    }
  }

  private async startThumbnailJob(regenerate: boolean): Promise<void> {
    await this.runAction(async () => {
      this.generateMissingButton.disabled = true;
      this.regenerateAllButton.disabled = true;
      try {
        await this.onStartThumbnailJob(regenerate);
        this.setStatus(
          regenerate ? "Regenerating every catalog preview…" : "Generating missing previews…",
        );
        await this.pollScanStatus();
      } catch (error) {
        this.generateMissingButton.disabled = false;
        this.regenerateAllButton.disabled = false;
        throw error;
      }
    });
  }

  private resetAndRender(): void {
    this.page = 1;
    this.renderContent();
  }

  private selectScope(scope: SelectionScope): void {
    const ids = selectionScopeIds(
      this.filteredModels.map((model) => model.id),
      this.visibleModelIds,
      scope,
    );
    for (const id of ids) this.selectedModelIds.add(id);
    this.renderContent();
  }

  private editSelected(): void {
    const models = this.allModels.filter((model) => this.selectedModelIds.has(model.id));
    if (!models.length) {
      this.setStatus("Select one or more models to edit.");
      return;
    }
    const active = document.activeElement;
    this.metadataEditor.show(
      models,
      this.allModels,
      this.allTags,
      this.allFolders,
      active instanceof HTMLElement ? active : this.root,
      () => this.searchInput.focus(),
    );
  }

  private async createBundle(): Promise<void> {
    const ids = [...this.selectedModelIds];
    if (ids.length < 2) {
      this.setStatus("Select at least two models to create a bundle.");
      return;
    }
    await this.runAction(async () => {
      await this.onCreateBundle(ids);
      this.selectedModelIds.clear();
      this.setStatus(`Created a bundle containing ${ids.length} models.`);
      this.renderContent();
    });
  }

  private async reviewDuplicates(): Promise<void> {
    const ids = explicitScopeIds(this.filteredModels);
    if (!ids.length) {
      this.setStatus("No available models are in the current filter.");
      return;
    }
    await this.runAction(() => this.onReviewDuplicates(ids));
  }

  private async removeSelected(): Promise<void> {
    const models = this.allModels.filter((model) => this.selectedModelIds.has(model.id));
    if (!models.length) {
      this.setStatus("Select models to remove from the catalog.");
      return;
    }
    if (
      !window.confirm(
        `Remove ${models.length} selected model${models.length === 1 ? "" : "s"} from the catalog?\n\nSource files will not be deleted. You can restore these records from Removed items.`,
      )
    ) {
      return;
    }
    await this.runAction(async () => {
      await this.onRemoveModels(removalTargets(models));
      this.selectedModelIds.clear();
    });
  }

  private runRoulette(): void {
    const chosen = chooseRouletteModel(this.filteredModels);
    if (!chosen) {
      this.setStatus("No available models are in the current filter.");
      return;
    }
    this.setStatus(`Print Roulette chose ${chosen.display_name}.`);
    this.onOpenModel(chosen.id);
  }

  private async openPrintHistory(model: LibraryModel): Promise<void> {
    this.activeHistoryModelId = model.id;
    const active = document.activeElement;
    try {
      this.printHistory.show(
        model,
        await this.onListPrintAttempts(model.id),
        active instanceof HTMLElement ? active : undefined,
      );
    } catch (error) {
      this.setStatus(`Could not load print history: ${libraryErrorMessage(error)}`);
    }
  }

  private async refreshPrintHistory(modelId: string): Promise<void> {
    this.printHistory.update(await this.onListPrintAttempts(modelId));
  }

  private renderContent(): void {
    this.renderRevision += 1;
    this.observer?.disconnect();
    this.gallery.dataset.view = this.viewMode;
    for (const [mode, button] of this.viewButtons) {
      button.setAttribute("aria-pressed", String(mode === this.viewMode));
    }
    this.renderNavigation();
    const models = filterAndSortLibraryModels(this.allModels, {
      query: this.searchInput.value,
      folderId: this.activeFolderId,
      rootId: this.activeRootId,
      source: this.sourceSelect.value as LibrarySourceFilter,
      status: this.statusSelect.value as LibraryStatusFilter,
      sort: this.sortSelect.value as LibrarySort,
    }).filter((model) => {
      if (!matchesIndexedDirectory(model, this.activeDirectoryRootId, this.activeDirectoryPrefix)) {
        return false;
      }
      if (!matchesPrintedFilter(model, this.printedSelect.value as PrintedFilter)) return false;
      const group = this.groupSelect.value as LibraryGroupFilter;
      if (!matchesLibraryGroup(model, group)) return false;
      return matchesLibraryMetadataFilters(
        model,
        {
          printStatus: this.printStatusSelect.value as "all" | PrintStatus,
          designer: this.designerSelect.value,
          tagId: this.tagSelect.value,
          category: this.categorySelect.value,
          parentId: this.parentSelect.value,
          license: this.licenseSelect.value,
        },
        this.allTags,
      );
    });
    this.filteredModels = models;
    this.visibleModelIds = [];
    this.selectionStatus.textContent = `${this.selectedModelIds.size} selected`;
    this.resultCount.textContent = `${models.length.toLocaleString()} model${models.length === 1 ? "" : "s"}`;
    this.gallery.replaceChildren();
    if (!models.length) {
      const empty = document.createElement("div");
      empty.className = "library-empty";
      const emptyTitle = document.createElement("strong");
      emptyTitle.textContent = "No models found";
      const emptyHelp = document.createElement("span");
      emptyHelp.textContent = "Adjust the filters, import copies, or index a folder.";
      empty.append(emptyTitle, emptyHelp);
      this.gallery.append(empty);
      return;
    }
    const shown = models.slice(0, this.page * PAGE_SIZE);
    this.visibleModelIds = shown.map((model) => model.id);
    this.observer =
      typeof IntersectionObserver === "undefined"
        ? null
        : new IntersectionObserver(
            (entries) => {
              for (const entry of entries) {
                if (!entry.isIntersecting) continue;
                this.observer?.unobserve(entry.target);
                const card = entry.target as HTMLElement;
                void this.requestThumbnail(card.dataset.modelId ?? "", card, false);
              }
            },
            { root: this.gallery, rootMargin: "160px" },
          );
    const partitioned = partitionBundleGroups(shown);
    for (const group of partitioned.groups)
      this.gallery.append(this.bundleRow(group.bundleId, group.models));
    for (const model of partitioned.singles) this.gallery.append(this.modelCard(model));
    if (shown.length < models.length) {
      const more = this.actionButton(
        `Show ${Math.min(PAGE_SIZE, models.length - shown.length)} more`,
        "",
        () => {
          this.page += 1;
          this.renderContent();
        },
      );
      more.classList.add("library-load-more");
      this.gallery.append(more);
    }
  }

  private renderNavigation(): void {
    this.navigation.replaceChildren();
    const sources = document.createElement("section");
    const sourceHeading = document.createElement("h3");
    sourceHeading.className = "library-section-label";
    sourceHeading.textContent = "Sources";
    sources.append(
      sourceHeading,
      this.filterButton("All models", this.activeRootId === null, () => {
        this.activeRootId = null;
        this.activeDirectoryRootId = null;
        this.activeDirectoryPrefix = null;
        this.resetAndRender();
      }),
    );
    for (const root of this.allRoots) {
      sources.append(this.rootCard(root));
      const tree = document.createElement("div");
      tree.className = "library-directory-tree";
      for (const node of indexedDirectoryNodes(this.allModels, root.id)) {
        const button = this.filterButton(
          `${node.name} · ${node.count}`,
          this.activeDirectoryRootId === root.id && this.activeDirectoryPrefix === node.prefix,
          () => {
            const alreadySelected =
              this.activeDirectoryRootId === root.id && this.activeDirectoryPrefix === node.prefix;
            this.activeRootId = root.id;
            this.activeDirectoryRootId = alreadySelected ? null : root.id;
            this.activeDirectoryPrefix = alreadySelected ? null : node.prefix;
            this.resetAndRender();
          },
        );
        button.style.setProperty("--directory-depth", String(node.depth));
        tree.append(button);
      }
      sources.append(tree);
    }
    if (!this.allRoots.length) {
      const hint = document.createElement("p");
      hint.className = "library-nav-hint";
      hint.textContent = "Index a folder to catalog models without copying them.";
      sources.append(hint);
    }

    const folders = document.createElement("section");
    const folderHeading = document.createElement("h3");
    folderHeading.className = "library-section-label";
    folderHeading.textContent = "Collections";
    const createCollection = this.actionButton("New collection", "", () => {
      const name = window.prompt("Collection name");
      if (!name?.trim()) return;
      void this.runAction(() => this.onCreateCollection(name.trim()));
    });
    folders.append(
      folderHeading,
      createCollection,
      this.filterButton("All collections", this.activeFolderId === null, () => {
        this.activeFolderId = null;
        this.resetAndRender();
      }),
    );
    const depths = libraryFolderDepths(this.allFolders);
    for (const folder of this.allFolders) {
      const button = this.filterButton(
        `${folder.name} · ${folder.model_count}`,
        this.activeFolderId === folder.id,
        () => {
          this.activeFolderId = this.activeFolderId === folder.id ? null : folder.id;
          this.resetAndRender();
        },
      );
      button.style.setProperty("--folder-depth", String(depths.get(folder.id) ?? 0));
      button.classList.add("library-folder-row");
      const row = document.createElement("div");
      row.className = "library-collection-row";
      row.append(button);
      if (folder.id !== "root") {
        row.append(
          this.actionButton("Rename", "", () => {
            const name = window.prompt("Rename collection", folder.name);
            if (!name?.trim()) return;
            void this.runAction(() => this.onRenameCollection(folder.id, name.trim()));
          }),
          this.actionButton("Delete", "danger", () => {
            if (!window.confirm(`Delete the empty collection “${folder.name}”?`)) return;
            void this.runAction(() => this.onDeleteCollection(folder.id));
          }),
        );
      }
      folders.append(row);
    }
    this.navigation.append(sources, folders, this.navigationResize);
  }

  private filterButton(label: string, selected: boolean, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "library-filter-button";
    button.setAttribute("aria-pressed", String(selected));
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private rootCard(root: IndexRoot): HTMLElement {
    const card = document.createElement("article");
    card.className = "library-root-card";
    const row = document.createElement("div");
    row.className = "library-root-heading";
    const select = this.filterButton(
      root.path.split(/[\\/]/).pop() || root.path,
      this.activeRootId === root.id,
      () => {
        this.activeRootId = this.activeRootId === root.id ? null : root.id;
        this.activeDirectoryRootId = null;
        this.activeDirectoryPrefix = null;
        this.resetAndRender();
      },
    );
    select.title = root.path;
    const enabled = document.createElement("input");
    enabled.type = "checkbox";
    enabled.checked = root.enabled;
    enabled.setAttribute("aria-label", `Enable scanning ${root.path}`);
    row.append(select, enabled);
    const path = document.createElement("div");
    path.className = "library-root-path";
    path.textContent = root.path;
    const config = document.createElement("div");
    config.className = "library-root-config";
    const [intervalField, interval] = this.numberField(
      "Minutes",
      root.scan_interval_minutes,
      1,
      1440,
    );
    const [maxMbField, maxMb] = this.numberField(
      "Max MB",
      Math.max(1, Math.round(root.max_file_bytes / 1048576)),
      1,
      100,
    );
    const scanZipField = document.createElement("label");
    scanZipField.className = "library-root-check";
    const scanZip = document.createElement("input");
    scanZip.type = "checkbox";
    scanZip.checked = root.scan_zip;
    scanZipField.append(scanZip, document.createTextNode(" Scan ZIP archives"));
    const save = this.actionButton(
      "Save",
      "",
      () =>
        void this.runAction(() =>
          this.onUpdateRoot({
            ...root,
            enabled: enabled.checked,
            scan_interval_minutes: Number(interval.value),
            max_file_bytes: Number(maxMb.value) * 1048576,
            scan_zip: scanZip.checked,
          }),
        ),
    );
    const scan = this.actionButton(
      "Scan",
      "",
      () => void this.runAction(() => this.onStartScan(root.id)),
    );
    const remove = this.actionButton("Remove", "danger", () => {
      if (
        !window.confirm(
          `Remove “${root.path}” and its indexed entries from the catalog?\n\nIts catalog metadata will be removed. The source folder and files will not be deleted.`,
        )
      ) {
        return;
      }
      void this.runAction(() => this.onRemoveRoot(root.id));
    });
    config.append(intervalField, maxMbField, scanZipField, save, scan, remove);
    const last = document.createElement("div");
    last.className = "library-root-last";
    const lastScanned = normalizeLibraryTimestamp(root.last_scanned_at);
    last.textContent = lastScanned
      ? `Scanned ${new Date(lastScanned).toLocaleString()}`
      : "Not scanned yet";
    card.append(row, path, config, last);
    return card;
  }

  private numberField(
    label: string,
    value: number,
    min: number,
    max: number,
  ): [HTMLLabelElement, HTMLInputElement] {
    const field = document.createElement("label");
    field.className = "library-number-field";
    const caption = document.createElement("span");
    caption.textContent = label;
    const input = document.createElement("input");
    input.type = "number";
    input.className = "library-number";
    input.value = String(value);
    input.min = String(min);
    input.max = String(max);
    input.setAttribute("aria-label", label);
    input.title = label;
    field.append(caption, input);
    return [field, input];
  }

  private modelCard(model: LibraryModel): HTMLElement {
    const article = document.createElement("article");
    article.className = "library-model";
    article.dataset.modelId = model.id;
    if (model.source_missing) article.classList.add("is-missing");
    const selected = document.createElement("input");
    selected.type = "checkbox";
    selected.className = "library-model-select";
    selected.checked = this.selectedModelIds.has(model.id);
    selected.setAttribute("aria-label", `Select ${model.display_name}`);
    selected.addEventListener("change", () => {
      if (selected.checked) this.selectedModelIds.add(model.id);
      else this.selectedModelIds.delete(model.id);
      this.selectionStatus.textContent = `${this.selectedModelIds.size} selected`;
    });
    const preview = document.createElement("div");
    preview.className = "library-model-preview";
    preview.setAttribute("aria-hidden", "true");
    preview.textContent = (model.format ?? "3D").toLocaleUpperCase();
    const cached = this.thumbnailCache.get(model.id);
    if (cached) this.installThumbnail(preview, cached);
    else if (this.thumbnailCache.has(model.id)) preview.classList.add("is-unavailable");
    else if (this.observer) this.observer.observe(article);
    else queueMicrotask(() => void this.requestThumbnail(model.id, article, false));

    const open = document.createElement("button");
    open.type = "button";
    open.className = "library-model-open";
    open.disabled = !canOpenLibraryModel(model);
    open.setAttribute(
      "aria-label",
      model.source_missing ? `${model.display_name}, source missing` : `Open ${model.display_name}`,
    );
    open.addEventListener("click", () => this.onOpenModel(model.id));
    const name = document.createElement("strong");
    name.className = "library-model-name";
    name.textContent = model.display_name;
    const meta = document.createElement("span");
    meta.className = "library-model-meta";
    meta.textContent = `${formatLibraryBytes(model.bytes)} · ${model.format?.toUpperCase() ?? "MODEL"}`;
    const source = document.createElement("span");
    source.className = "library-model-source";
    source.textContent = model.source_missing
      ? "Source missing — unavailable"
      : modelSource(model) === "indexed"
        ? "Indexed · read-only source"
        : "Managed copy";
    const sourceRoot = this.allRoots.find((root) => root.id === model.source_root_id);
    source.title =
      sourceRoot && model.source_path
        ? `${sourceRoot.path.replace(/[\\/]$/, "")}/${model.source_path}`
        : (model.source_path ?? model.original_name ?? "");
    const details = document.createElement("span");
    details.className = "library-model-details";
    const description = document.createElement("span");
    description.textContent = model.description || "No description";
    const path = document.createElement("span");
    path.className = "library-model-path";
    path.textContent = source.title || "No source path";
    const updated = document.createElement("span");
    const updatedAt = normalizeLibraryTimestamp(model.source_modified_at ?? model.updated_at);
    updated.textContent = updatedAt
      ? `Updated ${new Date(updatedAt).toLocaleString()}`
      : "Update time unavailable";
    const tags = document.createElement("span");
    tags.textContent = model.tags.length ? `Tags: ${model.tags.join(", ")}` : "No tags";
    const grouping = document.createElement("span");
    grouping.textContent = model.source_archive_member
      ? `ZIP member: ${model.source_archive_member}`
      : model.bundle_id
        ? `Bundle: ${model.bundle_id}`
        : "Standalone model";
    const print = document.createElement("span");
    print.textContent = `Print status: ${model.metadata?.print_status ?? "unprinted"} · ${model.metadata?.print_count ?? 0} successful`;
    const designer = document.createElement("span");
    designer.textContent = model.metadata?.designer
      ? `Designer: ${model.metadata.designer}`
      : "Designer unknown";
    const sourceUrl = document.createElement("span");
    sourceUrl.textContent = model.metadata?.source_url
      ? `Source: ${model.metadata.source_url}`
      : "No source URL";
    details.append(description, designer, sourceUrl, path, updated, tags, print, grouping);
    open.append(name, meta, source, details);
    const regenerate = this.actionButton("Refresh preview", "library-regenerate", () => {
      this.thumbnailCache.delete(model.id);
      void this.requestThumbnail(model.id, article, true);
    });
    regenerate.setAttribute("aria-label", `Regenerate preview for ${model.display_name}`);
    const edit = this.actionButton("Edit", "library-edit-model", () =>
      this.metadataEditor.show([model], this.allModels, this.allTags, this.allFolders, edit, () =>
        this.focusModelEditor(model.id),
      ),
    );
    edit.setAttribute("aria-label", `Edit metadata for ${model.display_name}`);
    const manage = document.createElement("details");
    manage.className = "library-card-actions";
    const manageSummary = document.createElement("summary");
    manageSummary.textContent = "More";
    manage.append(
      manageSummary,
      this.actionButton(
        "Reveal source",
        "",
        () => void this.runAction(() => this.onRevealSource(model.id)),
      ),
      this.actionButton(
        "Set custom preview…",
        "",
        () => void this.runAction(() => this.onChooseCustomPreview(model)),
      ),
      this.actionButton(
        "Clear custom preview",
        "",
        () => void this.runAction(() => this.onClearCustomPreview(model)),
      ),
    );
    if (model.source_kind === "indexed" && !model.source_archive_member) {
      manage.append(
        this.actionButton("Move original to Trash…", "danger", () => {
          if (!window.confirm(`Move the source file for “${model.display_name}” to Trash?`)) return;
          void this.runAction(() => this.onTrashLooseSource(model));
        }),
      );
    }
    article.append(preview, open, selected, edit, regenerate, manage);
    return article;
  }

  private bundleRow(bundleId: string, models: LibraryModel[]): HTMLElement {
    const details = document.createElement("details");
    details.className = "library-bundle-row";
    const summary = document.createElement("summary");
    const label = document.createElement("strong");
    const fullCount = this.allModels.filter((model) => model.bundle_id === bundleId).length;
    label.textContent =
      models.length === fullCount
        ? `${fullCount} model bundle`
        : `${models.length} shown of ${fullCount} bundle models`;
    const meta = document.createElement("span");
    meta.textContent = `${models.reduce((total, model) => total + model.bytes, 0).toLocaleString()} shown bytes`;
    summary.append(label, meta);
    const actions = document.createElement("div");
    actions.className = "library-bundle-actions";
    actions.append(
      this.actionButton(
        "Preview bundle…",
        "",
        () => void this.runAction(() => this.onPreviewBundle(bundleId)),
      ),
    );
    const members = document.createElement("div");
    members.className = "library-bundle-members";
    for (const model of models) {
      const row = document.createElement("button");
      row.type = "button";
      row.textContent = `${model.display_name} · ${formatLibraryBytes(model.bytes)}`;
      row.disabled = !canOpenLibraryModel(model);
      row.addEventListener("click", () => this.onOpenModel(model.id));
      members.append(row);
    }
    details.append(summary, actions, members);
    return details;
  }

  private focusModelEditor(modelId: string): void {
    const card = [...this.gallery.querySelectorAll<HTMLElement>(".library-model")].find(
      (candidate) => candidate.dataset.modelId === modelId,
    );
    const edit = card?.querySelector<HTMLButtonElement>(".library-edit-model");
    (edit ?? this.searchInput).focus();
  }

  private async requestThumbnail(
    modelId: string,
    card: HTMLElement,
    regenerate: boolean,
  ): Promise<void> {
    if (!modelId || !card.isConnected) return;
    const requestRevision = this.thumbnailGuard.begin(modelId);
    const renderRevision = this.renderRevision;
    const preview = card.querySelector<HTMLElement>(".library-model-preview");
    preview?.classList.add("is-loading");
    this.thumbnailQueue.push(async () => {
      if (renderRevision !== this.renderRevision || !card.isConnected) return;
      try {
        const dataUri = await this.onGetThumbnail(modelId, regenerate);
        if (
          !this.thumbnailGuard.isCurrent(modelId, requestRevision) ||
          renderRevision !== this.renderRevision ||
          !card.isConnected
        ) {
          return;
        }
        this.thumbnailCache.set(modelId, dataUri);
        if (preview && dataUri) this.installThumbnail(preview, dataUri);
        else preview?.classList.add("is-unavailable");
      } catch (error) {
        const isCurrent =
          this.thumbnailGuard.isCurrent(modelId, requestRevision) &&
          renderRevision === this.renderRevision &&
          card.isConnected;
        if (isCurrent) {
          preview?.classList.add("is-unavailable");
        }
        if (regenerate && isCurrent) {
          this.setStatus(`Could not regenerate preview: ${libraryErrorMessage(error)}`);
        }
      } finally {
        preview?.classList.remove("is-loading");
      }
    });
    this.drainThumbnailQueue();
  }

  private drainThumbnailQueue(): void {
    while (this.thumbnailWorkers < THUMBNAIL_CONCURRENCY) {
      const job = this.thumbnailQueue.shift();
      if (!job) return;
      this.thumbnailWorkers += 1;
      void job().finally(() => {
        this.thumbnailWorkers -= 1;
        this.drainThumbnailQueue();
      });
    }
  }

  private installThumbnail(preview: HTMLElement, dataUri: string): void {
    const image = document.createElement("img");
    image.src = dataUri;
    image.alt = "";
    preview.replaceChildren(image);
    preview.classList.remove("is-unavailable", "is-loading");
  }

  private async purgeThumbnails(): Promise<void> {
    this.purgeButton.disabled = true;
    try {
      await this.onPurgeThumbnails();
      this.thumbnailCache.clear();
      for (const model of this.allModels) this.thumbnailGuard.invalidate(model.id);
      this.setStatus(
        "Generated previews cleared. They will be recreated as models become visible.",
      );
      this.renderContent();
    } catch (error) {
      this.setStatus(`Could not clear previews: ${libraryErrorMessage(error)}`);
    } finally {
      this.purgeButton.disabled = false;
    }
  }

  private async runAction(action: () => Promise<void>): Promise<void> {
    try {
      await action();
    } catch (error) {
      this.setStatus(libraryErrorMessage(error));
    }
  }

  private startPolling(): void {
    this.stopPolling();
    void this.pollScanStatus();
    this.pollTimer = window.setInterval(() => void this.pollScanStatus(), 1000);
  }

  private stopPolling(): void {
    if (this.pollTimer !== undefined) window.clearInterval(this.pollTimer);
    this.pollTimer = undefined;
    this.scanPollRevision += 1;
  }

  private async pollScanStatus(): Promise<void> {
    if (!this.root.open || this.scanPollInFlight) return;
    const revision = ++this.scanPollRevision;
    this.scanPollInFlight = true;
    try {
      const status = await this.onPollScanStatus();
      if (this.root.open && revision === this.scanPollRevision) this.setScanStatus(status);
    } catch (error) {
      if (this.root.open && revision === this.scanPollRevision) {
        this.scanStatus.textContent = `Scan status unavailable: ${libraryErrorMessage(error)}`;
      }
    } finally {
      this.scanPollInFlight = false;
    }
  }
}
