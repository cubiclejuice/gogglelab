// Folder sidebar with native-root watching. Model navigation stays separate
// from archive actions, which are grouped at the bottom of the tree.

import {
  extractZip,
  describeError,
  isAppError,
  listDir,
  listModelFiles,
  listZipFiles,
  moveTreeEntry,
  listenFolderChanges,
  searchDir,
  watchFolder,
  type DirEntry,
  type ExtractResult,
  type FolderChangeEvent,
  type ModelFileList,
  type ZipArchiveList,
} from "./ipc";
import { RefreshQueue } from "./refresh-queue";

interface Node {
  entry: DirEntry;
  depth: number;
  children: Node[] | null;
  el: HTMLElement;
  action: HTMLButtonElement | null;
  searchResult: boolean;
  relDir?: string;
}

interface ContextMenuState {
  node: Node;
  trigger: HTMLElement;
}

export interface FileTreeApi {
  listDir(root: string, dir: string): Promise<DirEntry[]>;
  searchDir(root: string, query: string): Promise<DirEntry[]>;
  listModelFiles(root: string): Promise<ModelFileList>;
  listZipFiles(root: string): Promise<ZipArchiveList>;
  moveTreeEntry(root: string, path: string, destinationDirectory: string): Promise<void>;
  extractZip(root: string, path: string): Promise<ExtractResult>;
  watchFolder(root: string | null, watchId: number): Promise<string | null>;
  listenFolderChanges(callback: (event: FolderChangeEvent) => void): Promise<() => void>;
}

const nativeApi: FileTreeApi = {
  listDir,
  searchDir,
  listModelFiles,
  listZipFiles,
  moveTreeEntry,
  extractZip,
  watchFolder,
  listenFolderChanges,
};
const TREE_ENTRY_MIME = "application/x-gogglelab-tree-entry";
const MAX_TREE_DEPTH = 32;
const REFRESH_DELAY_MS = 220;
let nextWatchId = Date.now() * 1000;
const CHEVRON_RIGHT =
  '<svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>';
const CHEVRON_DOWN =
  '<svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>';
const FOLDER_ICON =
  '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const ZIP_ICON =
  '<svg class="zip-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M4 1.5h5l3 3V14H4z"/><path d="M9 1.5V5h3M7 6v1M7 9v1M7 12v1"/></svg>';
const SEARCH_ICON =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><line x1="15.5" y1="15.5" x2="21" y2="21"/></svg>';
function isZip(entry: DirEntry) {
  return !entry.is_dir && entry.name.toLowerCase().endsWith(".zip");
}

function isModel(entry: DirEntry) {
  return !entry.is_dir && /\.(?:stl|3mf|step|stp|f3d)$/i.test(entry.name);
}

function watchId() {
  nextWatchId += 1;
  return nextWatchId;
}

export class FileTree {
  private root: HTMLElement;
  private title: HTMLElement;
  private pick: HTMLButtonElement;
  private newFolderButton: HTMLButtonElement | null = null;
  private createFolderDialog: HTMLDialogElement | null = null;
  private folderNameInput: HTMLInputElement | null = null;
  private folderCreationError: HTMLElement | null = null;
  private createFolderSubmit: HTMLButtonElement | null = null;
  private createFolderCancel: HTMLButtonElement | null = null;
  private list: HTMLElement;
  private footer: HTMLElement;
  private status: HTMLElement;
  private refreshButton: HTMLButtonElement;
  private search: HTMLInputElement;
  private searchWrap: HTMLElement;
  private searchOpen = false;
  private searchResults: Node[] | null = null;
  private searchSeq = 0;
  private rootPath: string | null = null;
  private rootRevision = 0;
  private expansionRevision = 0;
  private nodes: Node[] = [];
  private renderedNodes: Node[] = [];
  private modelCount = 0;
  private modelTruncated = false;
  private modelScan: Promise<ModelFileList> | null = null;
  private modelScanRoot: string | null = null;
  private modelScanAgain = false;
  private archiveNodes: Node[] = [];
  private archiveTruncated = false;
  private archiveScan: Promise<ZipArchiveList> | null = null;
  private archiveScanRoot: string | null = null;
  private archiveScanAgain = false;
  private expandedPaths = new Set<string>();
  private currentPath: string | null = null;
  private rootAliases = new Map<string, string>();
  private statusOwner = "general";
  private activeWatchId = 0;
  private extractingPath: string | null = null;
  private trashingPath: string | null = null;
  private creatingFolder = false;
  private movingPath: string | null = null;
  private refreshOperations = 0;
  private refreshTimer: number | undefined;
  private unsubscribe: (() => void) | null = null;
  private disposed = false;
  private openRevision = 0;
  private listenerReady: Promise<void>;
  private listenerPending = false;
  private refreshQueue: RefreshQueue;
  private modelsOpen = false;
  private archivesOpen = false;
  private contextMenu: HTMLElement;
  private contextMenuState: ContextMenuState | null = null;

  private onDocumentPointerDown = (event: PointerEvent) => {
    const state = this.contextMenuState;
    const target = event.target as globalThis.Node | null;
    if (!state || !target || this.contextMenu.contains(target) || state.trigger.contains(target))
      return;
    this.closeContextMenu();
  };

  private onDocumentKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !this.contextMenuState) return;
    event.preventDefault();
    event.stopPropagation();
    this.closeContextMenu(true);
  };

  onOpenFile: (path: string) => void = () => {};
  onPickFolder: () => void = () => {};
  onRootChanged: (root: string) => void = () => {};
  onVisibilityChanged: (visible: boolean) => void = () => {};
  /** Main owns the native trash command; the tree only asks for confirmation. */
  onTrashFile: (path: string) => Promise<void> = async () => {
    throw new Error("Move to Trash is unavailable.");
  };
  constructor(
    container: HTMLElement,
    private api: FileTreeApi = nativeApi,
    private onZipExtracted?: (root: string, zipPath: string) => Promise<void>,
    private onCreateModelsFolder?: (root: string, name: string) => Promise<string>,
  ) {
    this.root = document.createElement("div");
    this.root.className = "sidebar";
    this.root.id = "sidebar";
    if (localStorage.getItem("sidebarVisible") === "false") this.root.classList.add("hidden");
    container.appendChild(this.root);
    this.contextMenu = document.createElement("div");
    this.contextMenu.className = "file-context-menu";
    this.contextMenu.setAttribute("role", "menu");
    this.contextMenu.hidden = true;
    document.body.appendChild(this.contextMenu);
    document.addEventListener("pointerdown", this.onDocumentPointerDown, true);
    document.addEventListener("keydown", this.onDocumentKeyDown, true);
    this.refreshQueue = new RefreshQueue(() => this.refreshNow());
    this.listenerReady = Promise.resolve();

    const saved = Number(localStorage.getItem("sidebarWidth"));
    if (saved >= 160) this.root.style.width = `${saved}px`;
    this.addResizeHandle();

    const header = document.createElement("div");
    header.className = "sidebar-header";
    const pick = document.createElement("button");
    pick.className = "btn folder primary";
    pick.innerHTML = `${FOLDER_ICON}<span>Folder…</span>`;
    pick.title = "Choose a folder of models (⌘⇧O)";
    pick.setAttribute("aria-label", "Choose folder");
    pick.addEventListener("click", () => this.onPickFolder());
    this.pick = pick;
    this.title = document.createElement("span");
    this.title.className = "sidebar-title empty";
    this.title.textContent = "No folder";
    this.enableFolderDropTarget(this.title, () => this.rootPath);
    const searchButton = document.createElement("button");
    searchButton.className = "btn icon";
    searchButton.title = "Search (⌘F)";
    searchButton.setAttribute("aria-label", "Search");
    searchButton.innerHTML = SEARCH_ICON;
    searchButton.addEventListener("click", () => this.focusSearch());
    header.append(pick, this.title, searchButton);
    this.root.appendChild(header);

    this.searchWrap = document.createElement("div");
    this.searchWrap.className = "search-wrap";
    this.search = document.createElement("input");
    this.search.type = "search";
    this.search.className = "search-input";
    this.search.placeholder = "Search models & ZIPs  (⌘F)";
    this.search.autocapitalize = "off";
    this.search.setAttribute("autocorrect", "off");
    this.search.spellcheck = false;
    let searchTimer: number | undefined;
    this.search.addEventListener("input", () => {
      window.clearTimeout(searchTimer);
      searchTimer = window.setTimeout(() => void this.runSearch(this.search.value), 120);
    });
    this.search.addEventListener("keydown", (event) => this.handleSearchKeys(event));
    this.searchWrap.appendChild(this.search);
    this.root.appendChild(this.searchWrap);

    this.list = document.createElement("div");
    this.list.className = "tree";
    this.list.setAttribute("role", "tree");
    this.list.setAttribute("aria-label", "Model files and ZIP archives");
    this.list.setAttribute("aria-busy", "false");
    this.list.addEventListener("wheel", (event) => {
      if (event.deltaY === 0) return;
      event.preventDefault();
      this.list.scrollTop += event.deltaY;
    });
    this.list.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        this.step(event.key === "ArrowDown" ? 1 : -1);
      }
    });
    this.root.appendChild(this.list);
    this.status = document.createElement("p");
    this.status.className = "tree-status";
    this.status.setAttribute("role", "status");
    this.status.hidden = true;
    this.root.appendChild(this.status);

    this.footer = document.createElement("div");
    this.footer.className = "sidebar-footer";
    this.refreshButton = document.createElement("button");
    this.refreshButton.className = "btn tree-refresh";
    this.refreshButton.type = "button";
    this.refreshButton.textContent = "Refresh";
    this.refreshButton.addEventListener("click", () => void this.retryWatchAndRefresh());
    this.footer.appendChild(this.refreshButton);
    if (this.onCreateModelsFolder) this.buildCreateFolderDialog();
    this.root.appendChild(this.footer);
    // The status element exists before a synchronous injected subscription
    // failure can be reported.
    this.listenerReady = this.startListener();
  }

  private buildCreateFolderDialog() {
    const dialog = document.createElement("dialog");
    dialog.className = "settings-dialog";
    dialog.setAttribute("aria-labelledby", "tree-folder-dialog-title");
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog && !this.creatingFolder) dialog.close();
    });
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      if (!this.creatingFolder) dialog.close();
    });

    const card = document.createElement("div");
    card.className = "sheet settings-card";
    const title = document.createElement("h2");
    title.id = "tree-folder-dialog-title";
    title.textContent = "New Folder";
    const description = document.createElement("p");
    description.className = "hint";
    description.textContent = "Create a folder at the top of the selected Models tree.";
    const input = document.createElement("input");
    input.type = "text";
    input.className = "search-input";
    input.maxLength = 255;
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = "Folder name";
    input.setAttribute("aria-label", "Folder name");
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void this.submitCreateFolder();
      }
    });
    const error = document.createElement("p");
    error.className = "settings-error";
    error.setAttribute("role", "alert");
    const actions = document.createElement("div");
    actions.className = "settings-actions";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => {
      if (!this.creatingFolder) dialog.close();
    });
    const create = document.createElement("button");
    create.type = "button";
    create.className = "btn primary";
    create.textContent = "Create";
    create.addEventListener("click", () => void this.submitCreateFolder());
    actions.append(cancel, create);
    card.append(title, description, input, error, actions);
    dialog.appendChild(card);
    document.body.appendChild(dialog);
    this.createFolderDialog = dialog;
    this.folderNameInput = input;
    this.folderCreationError = error;
    this.createFolderCancel = cancel;
    this.createFolderSubmit = create;
  }

  private addResizeHandle() {
    const handle = document.createElement("div");
    handle.className = "resize-handle";
    handle.setAttribute("role", "separator");
    handle.setAttribute("aria-orientation", "vertical");
    handle.tabIndex = 0;
    const setWidth = (width: number) => {
      const w = Math.max(160, Math.min(window.innerWidth * 0.6, width));
      this.root.style.width = `${w}px`;
      handle.setAttribute("aria-valuenow", String(Math.round(w)));
    };
    const stopResize = () => {
      document.body.classList.remove("resizing");
      localStorage.setItem(
        "sidebarWidth",
        String(Math.round(this.root.getBoundingClientRect().width)),
      );
      handle.releasePointerCapture?.(activePointer);
    };
    let activePointer = -1;
    handle.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      activePointer = e.pointerId;
      handle.setPointerCapture(e.pointerId);
      document.body.classList.add("resizing");
      const startX = e.clientX;
      const startW = this.root.getBoundingClientRect().width;
      const onMove = (ev: PointerEvent) => setWidth(startW + ev.clientX - startX);
      const onUp = () => {
        handle.removeEventListener("pointermove", onMove);
        handle.removeEventListener("pointerup", onUp);
        stopResize();
      };
      handle.addEventListener("pointermove", onMove);
      handle.addEventListener("pointerup", onUp, { once: true });
    });
    handle.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      e.preventDefault();
      const current = this.root.getBoundingClientRect().width;
      setWidth(current + (e.key === "ArrowRight" ? 16 : -16));
      localStorage.setItem(
        "sidebarWidth",
        String(Math.round(this.root.getBoundingClientRect().width)),
      );
    });
    this.root.appendChild(handle);
  }

  private async startListener() {
    this.listenerPending = true;
    try {
      const unsubscribe = await this.api.listenFolderChanges((event) =>
        this.onFolderChanged(event),
      );
      if (this.disposed) unsubscribe();
      else this.unsubscribe = unsubscribe;
    } catch (error) {
      this.setStatus(
        `Live folder updates are unavailable: ${this.errorMessage(error)}`,
        true,
        "watch",
      );
    } finally {
      this.listenerPending = false;
    }
  }

  private onFolderChanged(event: FolderChangeEvent) {
    if (
      this.disposed ||
      event.watch_id !== this.activeWatchId ||
      !this.rootPath ||
      event.root !== this.rootPath
    ) {
      return;
    }
    if (event.error) {
      this.setStatus(`Folder watch error: ${event.error}`, true, "watch");
      return;
    }
    window.clearTimeout(this.refreshTimer);
    this.refreshTimer = window.setTimeout(
      () => void this.refresh().catch(() => {}),
      REFRESH_DELAY_MS,
    );
  }

  private handleSearchKeys(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      if (this.search.value) this.clearSearch();
      else this.setSearchOpen(false);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      this.step(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const first = this.visibleFiles()[0];
      if (first && this.currentPath !== first.entry.path) this.onOpenFile(first.entry.path);
    }
  }
  focusSearch() {
    this.setSearchOpen(true);
    window.setTimeout(() => {
      this.search.focus();
      this.search.select();
    }, 30);
  }
  focusTree() {
    const active = this.list.querySelector<HTMLElement>('[tabindex="0"]');
    (active ?? this.pick).focus();
  }

  private setSearchOpen(open: boolean) {
    if (open === this.searchOpen) return;
    this.searchOpen = open;
    this.searchWrap.classList.toggle("open", open);
    if (!open) {
      this.search.blur();
      if (this.search.value) this.clearSearch();
    }
  }

  private async runSearch(query: string) {
    if (this.disposed) return;
    const root = this.rootPath;
    const rootRevision = this.rootRevision;
    const sequence = ++this.searchSeq;
    const trimmed = query.trim();
    if (!trimmed || !root) {
      this.searchResults = null;
      this.renderAll();
      return;
    }
    try {
      const entries = await this.api.searchDir(root, trimmed);
      if (
        this.disposed ||
        sequence !== this.searchSeq ||
        root !== this.rootPath ||
        rootRevision !== this.rootRevision
      ) {
        return;
      }
      const prefix = root === "/" ? "/" : `${root}/`;
      this.searchResults = entries.map((entry) => this.makeNode(entry, 0, true, prefix));
      if (this.searchResults.some((node) => isModel(node.entry))) this.modelsOpen = true;
      this.renderAll();
      if (this.searchResults.length === 0) this.renderEmpty("No matches");
    } catch (error) {
      if (
        !this.disposed &&
        sequence === this.searchSeq &&
        root === this.rootPath &&
        rootRevision === this.rootRevision
      ) {
        this.searchResults = [];
        this.renderAll();
        this.renderEmpty("Search unavailable");
        this.setStatus(`Search failed: ${this.errorMessage(error)}`, true);
      }
    }
  }

  private clearSearch() {
    this.search.value = "";
    this.searchResults = null;
    this.searchSeq++;
    this.renderAll();
  }

  addFooterControl(el: HTMLElement) {
    this.footer.appendChild(el);
  }

  get isVisible(): boolean {
    return !this.root.classList.contains("hidden");
  }
  setVisible(visible: boolean) {
    this.root.classList.toggle("hidden", !visible);
    localStorage.setItem("sidebarVisible", String(visible));
    this.onVisibilityChanged(visible);
  }

  private updateNewFolderButton() {
    if (!this.newFolderButton) return;
    this.newFolderButton.disabled =
      !this.rootPath ||
      this.creatingFolder ||
      this.movingPath !== null ||
      this.extractingPath !== null ||
      this.trashingPath !== null;
  }

  private async createFolder() {
    const root = this.rootPath;
    if (
      !root ||
      !this.onCreateModelsFolder ||
      this.creatingFolder ||
      this.movingPath !== null ||
      this.extractingPath !== null ||
      this.trashingPath !== null
    ) {
      return;
    }
    if (!this.createFolderDialog?.open) {
      if (this.folderCreationError) this.folderCreationError.textContent = "";
      if (this.folderNameInput) this.folderNameInput.value = "";
      this.createFolderDialog?.showModal();
      this.folderNameInput?.focus();
    }
  }

  private async submitCreateFolder() {
    const root = this.rootPath;
    const createFolder = this.onCreateModelsFolder;
    const name = this.folderNameInput?.value.trim() ?? "";
    if (
      !root ||
      !createFolder ||
      !name ||
      this.creatingFolder ||
      this.movingPath !== null ||
      this.extractingPath !== null ||
      this.trashingPath !== null
    ) {
      if (!name && this.folderCreationError) {
        this.folderCreationError.textContent = "Enter a folder name.";
        this.folderNameInput?.focus();
      }
      return;
    }

    const revision = this.rootRevision;
    this.creatingFolder = true;
    if (this.folderCreationError) this.folderCreationError.textContent = "";
    if (this.folderNameInput) this.folderNameInput.disabled = true;
    if (this.createFolderSubmit) this.createFolderSubmit.disabled = true;
    if (this.createFolderCancel) this.createFolderCancel.disabled = true;
    this.updateNewFolderButton();
    this.setStatus(`Creating folder “${name.trim()}”…`, false, "creating-folder");
    try {
      const path = await createFolder(root, name);
      if (this.disposed || root !== this.rootPath || revision !== this.rootRevision) {
        this.createFolderDialog?.close();
        return;
      }
      this.createFolderDialog?.close();
      this.modelsOpen = true;
      this.expandAncestors(`${path}/placeholder`);
      this.expandedPaths.add(path);
      this.expansionRevision++;
      try {
        await this.refresh();
        if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
          this.setStatus(`Created folder “${name}”.`);
        }
      } catch (error) {
        if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
          this.setStatus(
            `Created folder “${name}”, but the tree could not be refreshed: ${this.errorMessage(error)}`,
            true,
            "refresh",
          );
        }
      }
    } catch (error) {
      if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
        if (this.folderCreationError) {
          this.folderCreationError.textContent = this.errorMessage(error);
          this.folderNameInput?.focus();
        }
      }
    } finally {
      this.creatingFolder = false;
      if (this.folderNameInput) this.folderNameInput.disabled = false;
      if (this.createFolderSubmit) this.createFolderSubmit.disabled = false;
      if (this.createFolderCancel) this.createFolderCancel.disabled = false;
      this.updateNewFolderButton();
      if (this.createFolderDialog?.open && this.folderCreationError?.textContent) {
        this.folderNameInput?.focus();
      }
    }
  }

  async setRoot(requestedRoot: string) {
    if (this.disposed) return;
    requestedRoot = requestedRoot.replace(/\/+$/, "") || "/";
    const revision = ++this.rootRevision;
    const id = watchId();
    this.activeWatchId = id;
    await this.listenerReady;
    let canonical: string | null = null;
    try {
      canonical = await this.api.watchFolder(requestedRoot, id);
    } catch (error) {
      if (revision === this.rootRevision) {
        this.setStatus(
          `Live folder updates are unavailable: ${this.errorMessage(error)}. You can still browse this folder.`,
          true,
          "watch",
        );
      }
    }
    if (this.disposed || revision !== this.rootRevision) return;
    const usableRoot = canonical ?? requestedRoot;
    const changed = usableRoot !== this.rootPath;
    if (canonical && canonical !== requestedRoot) this.rootAliases.set(requestedRoot, canonical);
    this.rootPath = usableRoot;
    this.activeWatchId = canonical ? id : 0;
    this.updateNewFolderButton();
    if (changed) {
      this.nodes = [];
      this.modelCount = 0;
      this.modelTruncated = false;
      this.archiveNodes = [];
      this.archiveTruncated = false;
      this.expandedPaths.clear();
      this.searchResults = null;
      this.searchSeq++;
      this.createFolderDialog?.close();
      if (this.statusOwner !== "watch" && this.statusOwner !== "extracting") this.setStatus("");
    }
    if (canonical && this.unsubscribe && this.statusOwner === "watch") this.setStatus("");
    this.title.textContent = usableRoot.split("/").filter(Boolean).pop() || usableRoot;
    this.title.title = usableRoot;
    this.title.classList.remove("empty");
    this.pick.classList.remove("primary");
    this.onRootChanged(usableRoot);
    await this.refresh().catch(() => {});
  }

  async refresh() {
    const root = this.rootPath;
    const revision = this.rootRevision;
    if (!root || this.disposed) return;
    this.trackRefreshOperation(1);
    try {
      await this.refreshQueue.request();
    } catch (error) {
      if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
        this.setStatus(`Folder refresh failed: ${this.errorMessage(error)}`, true, "refresh");
      }
      throw error;
    } finally {
      this.trackRefreshOperation(-1);
    }
  }

  private trackRefreshOperation(change: 1 | -1) {
    this.refreshOperations = Math.max(0, this.refreshOperations + change);
    const refreshing = this.refreshOperations > 0;
    this.refreshButton.textContent = refreshing ? "Refreshing…" : "Refresh";
    this.refreshButton.setAttribute("aria-busy", String(refreshing));
    this.list.setAttribute("aria-busy", String(refreshing));
  }

  private async retryWatchAndRefresh() {
    const root = this.rootPath;
    if (!root) return;
    if (!this.unsubscribe && !this.listenerPending) this.listenerReady = this.startListener();
    await this.setRoot(root);
  }

  private async refreshNow() {
    const root = this.rootPath;
    const rootRevision = this.rootRevision;
    const expansionRevision = this.expansionRevision;
    if (!root) return;
    const snapshot = await this.buildSnapshot(root, root, 0, new Set());
    if (
      this.disposed ||
      root !== this.rootPath ||
      rootRevision !== this.rootRevision ||
      expansionRevision !== this.expansionRevision
    ) {
      return;
    }
    this.nodes = snapshot;
    if (this.statusOwner === "refresh") this.setStatus("");
    const query = this.search.value.trim();
    if (query) await this.runSearch(query);
    else {
      this.searchResults = null;
      this.renderAll();
    }
    this.refreshModelIndex(root, rootRevision);
    this.refreshArchiveIndex(root, rootRevision);
  }

  private refreshModelIndex(root: string, rootRevision: number) {
    if (this.modelScan && this.modelScanRoot === root) {
      this.modelScanAgain = true;
      return;
    }
    this.modelScanAgain = false;
    const scan = this.api.listModelFiles(root);
    this.modelScan = scan;
    this.modelScanRoot = root;
    this.trackRefreshOperation(1);
    void scan
      .then(({ models, truncated }) => {
        if (this.disposed || root !== this.rootPath || rootRevision !== this.rootRevision) return;
        this.modelCount = models.length;
        this.modelTruncated = truncated;
        if (truncated) {
          this.setStatus("Showing the first 10,000 models. Search to find others.", true, "models");
        } else if (this.statusOwner === "models") this.setStatus("");
        this.renderAll();
      })
      .catch((error) => {
        if (this.disposed || root !== this.rootPath || rootRevision !== this.rootRevision) return;
        this.modelCount = 0;
        this.modelTruncated = false;
        this.setStatus(`Model list unavailable: ${this.errorMessage(error)}`, true, "models");
        this.renderAll();
      })
      .finally(() => {
        this.trackRefreshOperation(-1);
        if (this.modelScan !== scan) return;
        this.modelScan = null;
        this.modelScanRoot = null;
        if (this.modelScanAgain && !this.disposed && root === this.rootPath) {
          this.modelScanAgain = false;
          this.refreshModelIndex(root, this.rootRevision);
        }
      });
  }

  private refreshArchiveIndex(root: string, rootRevision: number) {
    if (this.archiveScan && this.archiveScanRoot === root) {
      this.archiveScanAgain = true;
      return;
    }
    this.archiveScanAgain = false;
    const scan = this.api.listZipFiles(root);
    this.archiveScan = scan;
    this.archiveScanRoot = root;
    this.trackRefreshOperation(1);
    void scan
      .then(({ archives, truncated }) => {
        if (this.disposed || root !== this.rootPath || rootRevision !== this.rootRevision) return;
        const prefix = root === "/" ? "/" : `${root}/`;
        this.archiveNodes = archives
          .filter(isZip)
          .map((entry) => this.makeNode(entry, 0, true, prefix));
        this.archiveTruncated = truncated;
        if (truncated) {
          this.setStatus(
            "Showing the first 10,000 ZIP archives. Search to find others.",
            true,
            "archives",
          );
        } else if (this.statusOwner === "archives") this.setStatus("");
        this.renderAll();
      })
      .catch((error) => {
        if (this.disposed || root !== this.rootPath || rootRevision !== this.rootRevision) return;
        this.archiveNodes = [];
        this.archiveTruncated = false;
        this.setStatus(
          `ZIP archive list unavailable: ${this.errorMessage(error)}`,
          true,
          "archives",
        );
        this.renderAll();
      })
      .finally(() => {
        this.trackRefreshOperation(-1);
        if (this.archiveScan !== scan) return;
        this.archiveScan = null;
        this.archiveScanRoot = null;
        if (this.archiveScanAgain && !this.disposed && root === this.rootPath) {
          this.archiveScanAgain = false;
          this.refreshArchiveIndex(root, this.rootRevision);
        }
      });
  }

  private async buildSnapshot(
    root: string,
    directory: string,
    depth: number,
    ancestors: Set<string>,
  ): Promise<Node[]> {
    if (depth > MAX_TREE_DEPTH || ancestors.has(directory)) return [];
    const entries = await this.api.listDir(root, directory);
    const nextAncestors = new Set(ancestors).add(directory);
    const nodes = entries.map((entry) => this.makeNode(entry, depth));
    for (const node of nodes) {
      if (node.entry.is_dir && this.expandedPaths.has(node.entry.path)) {
        node.children = await this.buildSnapshot(root, node.entry.path, depth + 1, nextAncestors);
      }
    }
    return nodes;
  }

  async fileOpened(path: string) {
    if (this.disposed) return;
    const request = ++this.openRevision;
    path = this.resolvePath(path);
    let revision = this.rootRevision;
    if (!this.isInsideRoot(path)) {
      const slash = path.lastIndexOf("/");
      const setting = this.setRoot(slash > 0 ? path.slice(0, slash) : "/");
      revision = this.rootRevision;
      await setting;
      path = this.resolvePath(path);
    }
    if (
      this.disposed ||
      request !== this.openRevision ||
      revision !== this.rootRevision ||
      !this.isInsideRoot(path)
    )
      return;
    this.currentPath = path;
    this.modelsOpen = true;
    this.expandAncestors(path);
    await this.refresh().catch(() => {});
    if (
      this.disposed ||
      request !== this.openRevision ||
      revision !== this.rootRevision ||
      !this.isInsideRoot(path)
    )
      return;
    this.highlight(path, true);
  }

  step(delta: 1 | -1) {
    const files = this.visibleFiles();
    if (files.length === 0) return;
    const current = files.findIndex((node) => node.entry.path === this.currentPath);
    const next = files[Math.max(0, Math.min(files.length - 1, current + delta))];
    if (next && next.entry.path !== this.currentPath) this.onOpenFile(next.entry.path);
  }

  private expandAncestors(path: string) {
    if (!this.rootPath) return;
    const parts = path.slice(this.rootPath === "/" ? 1 : this.rootPath.length + 1).split("/");
    let current = this.rootPath;
    for (const part of parts.slice(0, -1)) {
      current = current === "/" ? `/${part}` : `${current}/${part}`;
      this.expandedPaths.add(current);
    }
    this.expansionRevision++;
  }

  private async toggle(node: Node) {
    if (!node.entry.is_dir) return;
    if (this.expandedPaths.has(node.entry.path)) this.expandedPaths.delete(node.entry.path);
    else this.expandedPaths.add(node.entry.path);
    this.expansionRevision++;
    await this.refresh().catch(() => {});
  }

  private async unzip(node: Node) {
    const root = this.rootPath;
    const revision = this.rootRevision;
    if (
      !root ||
      this.extractingPath ||
      this.movingPath ||
      this.creatingFolder ||
      this.trashingPath ||
      !isZip(node.entry)
    )
      return;
    this.extractingPath = node.entry.path;
    this.updateNewFolderButton();
    this.setStatus(`Extracting ${node.entry.name}…`, false, "extracting");
    this.renderAll();
    let extracted = false;
    let cleanupError: unknown = null;
    try {
      const result = await this.api.extractZip(root, node.entry.path);
      extracted = true;
      if (this.disposed || root !== this.rootPath || revision !== this.rootRevision) return;
      this.expandAncestors(`${result.destination}/placeholder`);
      this.expandedPaths.add(result.destination);
      this.expansionRevision++;
      try {
        await this.onZipExtracted?.(root, node.entry.path);
      } catch (error) {
        cleanupError = error;
      }
      if (this.disposed || root !== this.rootPath || revision !== this.rootRevision) return;
      await this.refresh();
      if (cleanupError) {
        this.setStatus(
          `Extracted ${result.files.toLocaleString()} file${result.files === 1 ? "" : "s"}, but the ZIP could not be moved to Trash: ${this.errorMessage(cleanupError)}`,
          true,
          "extract-result",
        );
      } else {
        this.setStatus(
          `Extracted ${result.files.toLocaleString()} file${result.files === 1 ? "" : "s"}.`,
          false,
          "extract-result",
        );
      }
    } catch (error) {
      if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
        this.setStatus(
          extracted
            ? `Archive extracted, but the folder could not be refreshed${cleanupError ? `; the ZIP could not be moved to Trash: ${this.errorMessage(cleanupError)}` : ""}: ${this.errorMessage(error)}`
            : `Could not unzip ${node.entry.name}: ${this.errorMessage(error)}`,
          true,
          extracted ? "refresh" : "general",
        );
      }
    } finally {
      // An extraction is global: a root switch must never leave this lock set.
      if (this.extractingPath === node.entry.path) this.extractingPath = null;
      this.updateNewFolderButton();
      if (!this.disposed) {
        if (this.statusOwner === "extracting") this.setStatus("");
        this.renderAll();
      }
    }
  }

  private enableFolderDropTarget(target: HTMLElement, destination: () => string | null) {
    target.addEventListener("dragover", (event) => {
      const transfer = (event as DragEvent).dataTransfer;
      if (
        !transfer ||
        !Array.from(transfer.types).includes(TREE_ENTRY_MIME) ||
        !destination() ||
        this.movingPath !== null ||
        this.trashingPath !== null ||
        this.creatingFolder ||
        this.extractingPath !== null
      ) {
        return;
      }
      event.preventDefault();
      transfer.dropEffect = "move";
      target.classList.add("drop-target");
    });
    target.addEventListener("dragleave", (event) => {
      const relatedTarget = (event as DragEvent).relatedTarget;
      if (relatedTarget instanceof globalThis.Node && target.contains(relatedTarget)) return;
      target.classList.remove("drop-target");
    });
    target.addEventListener("drop", (event) => {
      const drag = event as DragEvent;
      const source = drag.dataTransfer?.getData(TREE_ENTRY_MIME);
      const targetPath = destination();
      target.classList.remove("drop-target");
      if (
        !source ||
        !targetPath ||
        this.movingPath !== null ||
        this.trashingPath !== null ||
        this.creatingFolder ||
        this.extractingPath !== null
      ) {
        return;
      }
      drag.preventDefault();
      drag.stopPropagation();
      void this.moveEntry(source, targetPath);
    });
  }

  private async moveEntry(source: string, destinationDirectory: string) {
    const root = this.rootPath;
    if (
      !root ||
      this.movingPath !== null ||
      this.trashingPath !== null ||
      this.creatingFolder ||
      this.extractingPath !== null
    )
      return;
    const sourceParent = source.slice(0, source.lastIndexOf("/")) || "/";
    if (sourceParent === destinationDirectory) return;
    if (source === destinationDirectory || destinationDirectory.startsWith(`${source}/`)) {
      this.setStatus("A folder cannot be moved into itself or one of its subfolders.", true);
      return;
    }
    const name = source.slice(source.lastIndexOf("/") + 1) || source;
    const revision = this.rootRevision;
    this.movingPath = source;
    this.updateNewFolderButton();
    this.setStatus(`Moving “${name}”…`, false, "moving");
    try {
      await this.api.moveTreeEntry(root, source, destinationDirectory);
      if (this.disposed || root !== this.rootPath || revision !== this.rootRevision) return;
      this.expandAncestors(`${destinationDirectory}/placeholder`);
      if (destinationDirectory !== root) this.expandedPaths.add(destinationDirectory);
      this.expansionRevision++;
      await this.refresh();
      if (this.disposed || root !== this.rootPath || revision !== this.rootRevision) return;
      const destinationName =
        destinationDirectory === root
          ? "the selected folder"
          : destinationDirectory.slice(destinationDirectory.lastIndexOf("/") + 1);
      this.setStatus(`Moved “${name}” into ${destinationName}.`);
    } catch (error) {
      if (!this.disposed && root === this.rootPath && revision === this.rootRevision) {
        this.setStatus(`Could not move “${name}”: ${this.errorMessage(error)}`, true);
      }
    } finally {
      this.movingPath = null;
      this.updateNewFolderButton();
      if (!this.disposed) this.renderAll();
    }
  }

  private makeNode(entry: DirEntry, depth: number, searchResult = false, rootPrefix = ""): Node {
    const el = document.createElement("div");
    el.className = entry.is_dir ? "tree-row dir" : "tree-row";
    el.setAttribute("role", "treeitem");
    el.setAttribute("aria-level", String(depth + 1));
    el.dataset.treePath = entry.path;
    el.tabIndex = -1;
    el.draggable = true;
    el.style.paddingLeft = `${(entry.is_dir ? 10 : 24) + depth * 14}px`;
    const node: Node = {
      entry,
      depth,
      children: null,
      el,
      action: null,
      searchResult,
    };
    if (searchResult) {
      const relative = entry.path.startsWith(rootPrefix)
        ? entry.path.slice(rootPrefix.length)
        : entry.path;
      const slash = relative.lastIndexOf("/");
      if (slash >= 0) node.relDir = relative.slice(0, slash + 1);
    }
    el.addEventListener("dragstart", (event) => {
      const drag = event as DragEvent;
      if (
        !drag.dataTransfer ||
        this.movingPath !== null ||
        this.trashingPath !== null ||
        this.creatingFolder ||
        this.extractingPath !== null ||
        (event.target instanceof HTMLElement && event.target.closest("button"))
      ) {
        drag.preventDefault();
        return;
      }
      drag.dataTransfer.setData(TREE_ENTRY_MIME, entry.path);
      drag.dataTransfer.effectAllowed = "move";
      el.classList.add("dragging");
    });
    el.addEventListener("dragend", () => el.classList.remove("dragging"));
    if (entry.is_dir) this.enableFolderDropTarget(el, () => entry.path);
    el.addEventListener("click", () => {
      if (entry.is_dir) {
        if (node.searchResult) void this.revealDirectory(entry.path);
        else void this.toggle(node);
      } else if (!isZip(entry)) this.onOpenFile(entry.path);
    });
    el.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      this.openContextMenu(node, el, event.clientX, event.clientY);
    });
    el.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        this.step(event.key === "ArrowDown" ? 1 : -1);
      } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        this.openContextMenu(node, el);
      } else if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (entry.is_dir) void this.toggle(node);
        else if (!isZip(entry)) this.onOpenFile(entry.path);
      }
    });
    this.label(node);
    return node;
  }

  private label(node: Node) {
    node.el.replaceChildren();
    if (node.entry.is_dir) {
      node.el.setAttribute("aria-expanded", String(this.expandedPaths.has(node.entry.path)));
      node.el.removeAttribute("aria-selected");
      const chevron = document.createElement("span");
      chevron.innerHTML = this.expandedPaths.has(node.entry.path) ? CHEVRON_DOWN : CHEVRON_RIGHT;
      node.el.appendChild(chevron.firstElementChild!);
    } else {
      node.el.setAttribute("aria-selected", "false");
      node.el.removeAttribute("aria-expanded");
      if (isZip(node.entry)) {
        const icon = document.createElement("span");
        icon.innerHTML = ZIP_ICON;
        node.el.appendChild(icon.firstElementChild!);
      }
    }
    const name = document.createElement("span");
    name.className = "name";
    if (node.relDir) {
      const relative = document.createElement("span");
      relative.className = "rel";
      relative.textContent = node.relDir;
      name.appendChild(relative);
    }
    name.appendChild(document.createTextNode(node.entry.name));
    node.el.appendChild(name);
    node.action = null;
    if (isZip(node.entry)) {
      const action = document.createElement("button");
      action.type = "button";
      action.className = "tree-unzip";
      action.textContent = this.extractingPath === node.entry.path ? "Unzipping…" : "Unzip";
      action.setAttribute("aria-label", `Unzip ${node.entry.name}`);
      action.dataset.treePath = node.entry.path;
      const busy = this.extractingPath !== null;
      action.setAttribute("aria-disabled", String(busy));
      action.classList.toggle("busy", busy);
      action.addEventListener("click", (event) => {
        event.stopPropagation();
        if (!this.extractingPath) void this.unzip(node);
      });
      node.action = action;
      node.el.appendChild(action);
    }
  }

  private flatten(nodes: Node[] = this.searchResults ?? this.nodes, output: Node[] = []): Node[] {
    for (const node of nodes) {
      output.push(node);
      if (node.entry.is_dir && this.expandedPaths.has(node.entry.path) && node.children) {
        this.flatten(node.children, output);
      }
    }
    return output;
  }

  private visibleFiles(): Node[] {
    if (!this.modelsOpen) return [];
    return this.flatten().filter((node) => isModel(node.entry));
  }

  private renderAll() {
    const scrollTop = this.list.scrollTop;
    const focusedPath = (document.activeElement as HTMLElement | null)?.dataset.treePath;
    const nodes = this.flatten();
    for (const node of nodes) this.updateAction(node);
    const models = nodes.filter((node) => node.entry.is_dir || isModel(node.entry));
    const modelCount = this.searchResults
      ? models.filter((node) => !node.entry.is_dir).length
      : this.modelCount;
    // Folder traversal only materializes expanded paths. The archive section
    // instead uses the bounded recursive native search, except during a user
    // search where its ZIP matches remain part of that search result.
    const archives = this.searchResults
      ? nodes.filter((node) => isZip(node.entry))
      : this.archiveNodes;
    const renderedNodes = [
      ...(this.modelsOpen ? models : []),
      ...(this.archivesOpen ? archives : []),
    ];
    this.renderedNodes = renderedNodes;
    this.list.replaceChildren();
    if (models.length > 0 || this.modelCount > 0 || this.onCreateModelsFolder) {
      this.list.appendChild(this.modelsGroup(models, modelCount));
    }
    if (archives.length > 0) this.list.appendChild(this.archiveGroup(archives));
    this.list.scrollTop = scrollTop;
    this.highlight(this.currentPath, false);
    if (focusedPath) {
      const focused = renderedNodes.find((node) => node.entry.path === focusedPath)?.el;
      focused?.focus({ preventScroll: true });
    }
  }

  private modelsGroup(models: Node[], modelCount: number) {
    const section = document.createElement("section");
    section.className = "tree-models";
    const header = document.createElement("div");
    header.className = "tree-models-header";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "tree-models-toggle";
    toggle.setAttribute("aria-expanded", String(this.modelsOpen));
    toggle.setAttribute("aria-controls", "tree-models-list");
    const count = `${modelCount}${!this.searchResults && this.modelTruncated ? "+" : ""}`;
    toggle.innerHTML = `${this.modelsOpen ? CHEVRON_DOWN : CHEVRON_RIGHT}<span>Models (${count})</span>`;
    const setOpen = (open: boolean) => {
      this.modelsOpen = open;
      this.renderAll();
      this.list.querySelector<HTMLButtonElement>(".tree-models-toggle")?.focus({
        preventScroll: true,
      });
    };
    toggle.addEventListener("click", () => setOpen(!this.modelsOpen));
    toggle.addEventListener("keydown", (event) => {
      if (event.key === "ArrowRight" && !this.modelsOpen) {
        event.preventDefault();
        setOpen(true);
      } else if (event.key === "ArrowLeft" && this.modelsOpen) {
        event.preventDefault();
        setOpen(false);
      }
    });
    header.appendChild(toggle);
    this.newFolderButton = null;
    if (this.onCreateModelsFolder) {
      const create = document.createElement("button");
      create.type = "button";
      create.className = "btn new-folder";
      create.textContent = "New Folder";
      create.title = "Create a folder in the selected Models folder";
      create.setAttribute("aria-label", "New Folder");
      create.addEventListener("click", () => void this.createFolder());
      this.newFolderButton = create;
      header.appendChild(create);
      this.updateNewFolderButton();
    }
    section.appendChild(header);
    if (this.modelsOpen) {
      const list = document.createElement("div");
      list.id = "tree-models-list";
      list.setAttribute("role", "group");
      for (const model of models) {
        if (this.searchResults) model.relDir = this.relativeDirectory(model.entry.path);
        this.label(model);
        list.appendChild(model.el);
      }
      section.appendChild(list);
    }
    return section;
  }

  private archiveGroup(archives: Node[]) {
    const section = document.createElement("section");
    section.className = "tree-archives";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "tree-archives-toggle";
    toggle.setAttribute("aria-expanded", String(this.archivesOpen));
    toggle.setAttribute("aria-controls", "tree-archives-list");
    const count = `${archives.length}${!this.searchResults && this.archiveTruncated ? "+" : ""}`;
    toggle.innerHTML = `${this.archivesOpen ? CHEVRON_DOWN : CHEVRON_RIGHT}<span>ZIP archives (${count})</span>`;
    const setOpen = (open: boolean) => {
      this.archivesOpen = open;
      this.renderAll();
      this.list.querySelector<HTMLButtonElement>(".tree-archives-toggle")?.focus({
        preventScroll: true,
      });
    };
    toggle.addEventListener("click", () => setOpen(!this.archivesOpen));
    toggle.addEventListener("keydown", (event) => {
      if (event.key === "ArrowRight" && !this.archivesOpen) {
        event.preventDefault();
        setOpen(true);
      } else if (event.key === "ArrowLeft" && this.archivesOpen) {
        event.preventDefault();
        setOpen(false);
      }
    });
    section.appendChild(toggle);
    if (this.archivesOpen) {
      const list = document.createElement("div");
      list.id = "tree-archives-list";
      list.setAttribute("role", "group");
      for (const archive of archives) {
        archive.relDir = this.relativeDirectory(archive.entry.path);
        this.label(archive);
        list.appendChild(archive.el);
      }
      section.appendChild(list);
    }
    return section;
  }

  private relativeDirectory(path: string) {
    if (!this.rootPath) return "";
    const prefix = this.rootPath === "/" ? "/" : `${this.rootPath}/`;
    const relative = path.startsWith(prefix) ? path.slice(prefix.length) : path;
    const slash = relative.lastIndexOf("/");
    return slash >= 0 ? relative.slice(0, slash + 1) : "";
  }

  private openContextMenu(node: Node, trigger: HTMLElement, x?: number, y?: number) {
    if (this.disposed) return;
    this.closeContextMenu();
    const rect = trigger.getBoundingClientRect();
    const left = x ?? rect.right;
    const top = y ?? rect.bottom;
    this.contextMenu.replaceChildren();
    const trash = document.createElement("button");
    trash.type = "button";
    trash.setAttribute("role", "menuitem");
    trash.textContent = node.entry.is_dir ? "Move Folder to Trash" : "Move to Trash";
    trash.disabled =
      this.trashingPath !== null ||
      this.movingPath !== null ||
      this.creatingFolder ||
      this.extractingPath !== null;
    trash.addEventListener("click", () => void this.requestTrash(node));
    this.contextMenu.appendChild(trash);
    this.contextMenu.style.left = `${Math.max(8, Math.min(left, window.innerWidth - 184))}px`;
    this.contextMenu.style.top = `${Math.max(8, Math.min(top, window.innerHeight - 48))}px`;
    this.contextMenu.hidden = false;
    this.contextMenuState = { node, trigger };
    trash.focus();
  }

  private closeContextMenu(restoreFocus = false) {
    const state = this.contextMenuState;
    this.contextMenuState = null;
    this.contextMenu.hidden = true;
    this.contextMenu.replaceChildren();
    if (restoreFocus) state?.trigger.focus({ preventScroll: true });
  }

  private async requestTrash(node: Node) {
    const state = this.contextMenuState;
    if (
      !state ||
      state.node !== node ||
      this.trashingPath ||
      this.movingPath ||
      this.creatingFolder ||
      this.extractingPath
    )
      return;
    const confirmed = window.confirm(
      node.entry.is_dir
        ? `Move folder “${node.entry.name}” and its contents to Trash?`
        : `Move “${node.entry.name}” to Trash?`,
    );
    if (!confirmed) {
      this.closeContextMenu(true);
      return;
    }
    this.closeContextMenu();
    this.trashingPath = node.entry.path;
    this.updateNewFolderButton();
    this.renderAll();
    try {
      await this.onTrashFile(node.entry.path);
      if (this.disposed) return;
      try {
        await this.refresh();
        this.setStatus(`Moved ${node.entry.name} to Trash.`);
      } catch (error) {
        this.setStatus(
          `Moved ${node.entry.name} to Trash, but the folder could not be refreshed: ${this.errorMessage(error)}`,
          true,
          "refresh",
        );
      }
    } catch (error) {
      if (!this.disposed) {
        this.setStatus(
          `Could not move ${node.entry.name} to Trash: ${this.errorMessage(error)}`,
          true,
        );
      }
    } finally {
      if (this.trashingPath === node.entry.path) this.trashingPath = null;
      this.updateNewFolderButton();
      if (!this.disposed) this.renderAll();
    }
  }

  private renderEmpty(message: string) {
    const empty = document.createElement("div");
    empty.className = "tree-empty";
    empty.textContent = message;
    this.list.appendChild(empty);
  }

  private highlight(path: string | null, scroll: boolean) {
    const visible = this.renderedNodes;
    const active =
      visible.find(
        (node) => !node.entry.is_dir && !isZip(node.entry) && node.entry.path === path,
      ) ?? visible.find((node) => !node.entry.is_dir && !isZip(node.entry));
    for (const node of visible) {
      const selected = !node.entry.is_dir && !isZip(node.entry) && node.entry.path === path;
      node.el.classList.toggle("selected", selected);
      node.el.tabIndex = node === active ? 0 : -1;
      if (!node.entry.is_dir) node.el.setAttribute("aria-selected", String(selected));
      if (selected && scroll) node.el.scrollIntoView({ block: "nearest" });
    }
  }

  private setStatus(message: string, error = false, owner = "general") {
    this.statusOwner = owner;
    this.status.textContent = message;
    this.status.hidden = !message;
    this.status.classList.toggle("error", error);
  }

  private updateAction(node: Node) {
    const action = node.action;
    if (!action) return;
    const busy = this.extractingPath !== null;
    action.textContent = this.extractingPath === node.entry.path ? "Unzipping…" : "Unzip";
    action.setAttribute("aria-disabled", String(busy));
    action.classList.toggle("busy", busy);
  }

  private async revealDirectory(path: string) {
    if (!this.isInsideRoot(path)) return;
    this.search.value = "";
    this.searchResults = null;
    this.searchSeq++;
    this.expandAncestors(`${path}/placeholder`);
    this.expandedPaths.add(path);
    this.expansionRevision++;
    await this.refresh().catch(() => {});
  }

  private isInsideRoot(path: string) {
    return (
      this.rootPath !== null &&
      (path === this.rootPath || path.startsWith(this.rootPath === "/" ? "/" : `${this.rootPath}/`))
    );
  }

  private resolvePath(path: string) {
    for (const [alias, canonical] of [...this.rootAliases].sort(
      (a, b) => b[0].length - a[0].length,
    )) {
      if (path === alias || path.startsWith(`${alias}/`))
        return canonical + path.slice(alias.length);
    }
    return path;
  }

  private errorMessage(error: unknown) {
    if (isAppError(error)) return describeError(error);
    return error instanceof Error ? error.message : String(error);
  }

  dispose() {
    this.disposed = true;
    this.closeContextMenu();
    document.removeEventListener("pointerdown", this.onDocumentPointerDown, true);
    document.removeEventListener("keydown", this.onDocumentKeyDown, true);
    this.contextMenu.remove();
    this.createFolderDialog?.remove();
    this.rootRevision++;
    this.activeWatchId = 0;
    window.clearTimeout(this.refreshTimer);
    this.unsubscribe?.();
    this.unsubscribe = null;
    void this.api.watchFolder(null, watchId()).catch(() => {});
  }
}
