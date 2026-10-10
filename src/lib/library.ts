import type { LibraryFolder, LibraryModel } from "./ipc";

function formatBytes(bytes: number): string {
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

export class LibraryPanel {
  private readonly root: HTMLDialogElement;
  private readonly folders: HTMLElement;
  private readonly list: HTMLElement;
  private readonly status: HTMLElement;
  private readonly searchInput: HTMLInputElement;
  private invoker: HTMLElement | null = null;
  private allFolders: LibraryFolder[] = [];
  private allModels: LibraryModel[] = [];
  private activeFolderId: string | null = null;
  private importButton: HTMLButtonElement;

  onImport: () => void = () => {};
  onOpenModel: (modelId: string) => void = () => {};

  constructor(container: HTMLElement) {
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

    const header = document.createElement("div");
    header.className = "library-header";
    const title = document.createElement("h2");
    title.id = "library-title";
    title.className = "library-title";
    title.textContent = "Library";
    const actions = document.createElement("div");
    actions.className = "library-actions";
    this.importButton = document.createElement("button");
    this.importButton.type = "button";
    this.importButton.className = "btn primary";
    this.importButton.textContent = "Import models…";
    this.importButton.addEventListener("click", () => this.onImport());
    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "btn";
    closeButton.textContent = "Close";
    closeButton.addEventListener("click", () => this.hide());
    actions.append(this.importButton, closeButton);
    header.append(title, actions);

    this.status = document.createElement("div");
    this.status.className = "library-status hint";
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");

    this.searchInput = document.createElement("input");
    this.searchInput.type = "search";
    this.searchInput.className = "library-search";
    this.searchInput.placeholder = "Search library";
    this.searchInput.setAttribute("aria-label", "Search library");
    this.searchInput.addEventListener("input", () => this.renderContent());

    this.folders = document.createElement("div");
    this.folders.className = "library-folders";
    this.list = document.createElement("div");
    this.list.className = "library-list";
    this.root.append(header, this.status, this.searchInput, this.folders, this.list);
    container.appendChild(this.root);
  }

  get isOpen(): boolean {
    return this.root.open;
  }

  show(invoker?: HTMLElement): void {
    const active = invoker ?? document.activeElement;
    this.invoker = active instanceof HTMLElement ? active : null;
    if (!this.root.open) this.root.showModal();
  }

  hide(): void {
    if (this.root.open) this.root.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  setStatus(message: string): void {
    this.status.textContent = message;
  }

  setImportBusy(busy: boolean): void {
    this.importButton.disabled = busy;
    this.importButton.setAttribute("aria-disabled", String(busy));
  }

  render(folders: LibraryFolder[], models: LibraryModel[]): void {
    this.allFolders = folders;
    this.allModels = models;
    if (this.activeFolderId && !folders.some((folder) => folder.id === this.activeFolderId)) {
      this.activeFolderId = null;
    }
    this.renderContent();
  }

  private renderContent(): void {
    const depths = libraryFolderDepths(this.allFolders);

    this.folders.replaceChildren();
    const folderHeading = document.createElement("div");
    folderHeading.className = "library-section-label";
    folderHeading.textContent = "Folders";
    this.folders.appendChild(folderHeading);
    for (const folder of this.allFolders) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "library-folder-row";
      row.style.setProperty("--folder-depth", String(depths.get(folder.id) ?? 0));
      row.setAttribute("aria-pressed", String(this.activeFolderId === folder.id));
      row.textContent = `${folder.name} · ${folder.model_count}`;
      row.addEventListener("click", () => {
        this.activeFolderId = this.activeFolderId === folder.id ? null : folder.id;
        this.renderContent();
      });
      this.folders.appendChild(row);
    }

    const query = this.searchInput.value.trim().toLocaleLowerCase();
    const models = this.allModels.filter((model) => {
      if (this.activeFolderId && model.folder_id !== this.activeFolderId) return false;
      if (!query) return true;
      return [model.display_name, model.original_name ?? "", model.description, ...model.tags]
        .join(" ")
        .toLocaleLowerCase()
        .includes(query);
    });
    this.list.replaceChildren();
    const modelHeading = document.createElement("div");
    modelHeading.className = "library-section-label";
    modelHeading.textContent = `${models.length} model${models.length === 1 ? "" : "s"}`;
    this.list.appendChild(modelHeading);
    if (models.length === 0) {
      const empty = document.createElement("div");
      empty.className = "library-empty hint";
      empty.textContent =
        query || this.activeFolderId ? "No matching models." : "Your managed library is empty.";
      this.list.appendChild(empty);
      return;
    }
    for (const model of models) {
      const button = document.createElement("button");
      button.className = "library-model";
      button.type = "button";
      button.addEventListener("click", () => this.onOpenModel(model.id));
      const name = document.createElement("span");
      name.className = "library-model-name";
      name.textContent = model.display_name;
      const meta = document.createElement("span");
      meta.className = "library-model-meta";
      meta.textContent = `${model.original_name ?? "No source"} · ${formatBytes(model.bytes)}`;
      button.append(name, meta);
      this.list.appendChild(button);
    }
  }
}
