import type {
  LibraryModel,
  LibraryFolder,
  LibraryTag,
  MetadataPatch,
  PrintStatus,
  VersionedModelTarget,
} from "./ipc";

export type SelectionScope = "visible" | "filtered";
export type PrintedFilter = "all" | "never" | "ever" | "last30";

export function selectionScopeIds(
  filteredIds: string[],
  visibleIds: string[],
  scope: SelectionScope,
): string[] {
  return [...new Set(scope === "visible" ? visibleIds : filteredIds)];
}

export function buildMetadataPatch(
  values: Record<string, string>,
  appliedFields: ReadonlySet<string>,
  tagIds: string[],
  tagMode: "replace" | "merge",
): MetadataPatch {
  const patch: MetadataPatch = {};
  for (const field of [
    "designer",
    "source_url",
    "notes",
    "model_license",
    "print_status",
    "folder_id",
    "parent_model_id",
  ] as const) {
    if (appliedFields.has(field)) Object.assign(patch, { [field]: values[field] ?? "" });
  }
  if (appliedFields.has("tags")) {
    patch.tags = [...tagIds];
    patch.tag_mode = tagMode;
  }
  return patch;
}

export function versionedMetadataTargets(
  models: LibraryModel[],
  selectedIds: readonly string[],
): VersionedModelTarget[] {
  const selected = new Set(selectedIds);
  return models
    .filter((model) => selected.has(model.id))
    .map((model) => ({
      model_id: model.id,
      expected_version: model.metadata_version,
    }));
}

export function matchesPrintedFilter(
  model: LibraryModel,
  filter: PrintedFilter,
  now = Date.now(),
): boolean {
  if (filter === "all") return true;
  const count = model.metadata?.print_count ?? 0;
  if (filter === "never") return count === 0;
  if (filter === "ever") return count > 0;
  const printed = model.metadata?.last_printed_at;
  if (!printed) return false;
  const milliseconds = Date.parse(printed);
  return Number.isFinite(milliseconds) && now - milliseconds <= 30 * 24 * 60 * 60 * 1000;
}

export function isMetadataVersionError(error: unknown): boolean {
  if (typeof error !== "object" || error === null || !("message" in error)) return false;
  const message = String((error as { message: unknown }).message).toLocaleLowerCase();
  return message.includes("version") || message.includes("conflict") || message.includes("changed");
}

export function restoreLibraryFocus(
  target: { isConnected: boolean; focus: () => void } | null,
  fallback: () => void,
): void {
  if (target?.isConnected) target.focus();
  else fallback();
}

type EditorSave = (targets: VersionedModelTarget[], patch: MetadataPatch) => Promise<void>;

export class LibraryMetadataEditor {
  private readonly dialog: HTMLDialogElement;
  private invoker: HTMLElement | null = null;
  private restoreFocus: () => void = () => {};
  onSave: EditorSave = async () => {};
  onManageTags: () => void = () => {};
  onHistory: (model: LibraryModel) => void = () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-editor-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
  }

  show(
    models: LibraryModel[],
    allModels: LibraryModel[],
    tags: LibraryTag[],
    folders: LibraryFolder[],
    invoker: HTMLElement,
    restoreFocus: () => void = () => {},
  ): void {
    if (!models.length) return;
    this.invoker = invoker;
    this.restoreFocus = restoreFocus;
    this.dialog.replaceChildren();
    const batch = models.length > 1;
    const form = document.createElement("form");
    form.className = "library-editor-form";
    form.addEventListener("submit", (event) => event.preventDefault());
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = batch ? `Edit ${models.length} models` : `Edit ${models[0].display_name}`;
    const close = this.button("Close", () => this.close());
    header.append(title, close);
    const error = document.createElement("div");
    error.className = "library-editor-error";
    error.setAttribute("role", "alert");
    const values: Record<string, HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement> = {};
    const applied = new Map<string, HTMLInputElement>();
    const metadata = models[0].metadata;
    const addField = (
      key: string,
      label: string,
      control: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
    ) => {
      const row = document.createElement("label");
      row.className = "library-editor-field";
      row.classList.toggle("is-batch", batch);
      if (batch) {
        const apply = document.createElement("input");
        apply.type = "checkbox";
        apply.setAttribute("aria-label", `Apply ${label}`);
        applied.set(key, apply);
        row.append(apply);
      }
      const caption = document.createElement("span");
      caption.textContent = label;
      control.setAttribute("aria-label", label);
      row.append(caption, control);
      values[key] = control;
      form.append(row);
    };
    const text = (value = "", type = "text") => {
      const input = document.createElement("input");
      input.type = type;
      input.value = value;
      return input;
    };
    addField("designer", "Designer", text(metadata?.designer ?? ""));
    addField("source_url", "Source URL", text(metadata?.source_url ?? "", "url"));
    const notes = document.createElement("textarea");
    notes.rows = 4;
    notes.value = metadata?.notes ?? "";
    addField("notes", "Notes", notes);
    addField("model_license", "Model license", text(metadata?.model_license ?? ""));
    const status = document.createElement("select");
    for (const value of [
      "unprinted",
      "want",
      "queued",
      "printing",
      "printed",
      "failed",
    ] as PrintStatus[]) {
      status.add(new Option(value[0].toUpperCase() + value.slice(1), value));
    }
    status.value = metadata?.print_status ?? "unprinted";
    addField("print_status", "Print status", status);
    const folder = document.createElement("select");
    for (const item of folders) folder.add(new Option(item.name, item.id));
    folder.value = models[0].folder_id;
    addField("folder_id", "Collection", folder);
    const parent = document.createElement("select");
    parent.add(new Option("No parent", ""));
    const selectedIds = new Set(models.map((model) => model.id));
    for (const candidate of allModels) {
      if (!selectedIds.has(candidate.id))
        parent.add(new Option(candidate.display_name, candidate.id));
    }
    parent.value = metadata?.parent_model_id ?? "";
    addField("parent_model_id", "Parent model", parent);

    const tagGroup = document.createElement("fieldset");
    tagGroup.className = "library-editor-tags";
    const legend = document.createElement("legend");
    legend.textContent = "Tags";
    tagGroup.append(legend);
    let applyTags: HTMLInputElement | null = null;
    const tagMode = document.createElement("select");
    tagMode.add(new Option("Replace tags", "replace"));
    tagMode.add(new Option("Add tags", "merge"));
    if (batch) {
      applyTags = document.createElement("input");
      applyTags.type = "checkbox";
      applyTags.setAttribute("aria-label", "Apply tags");
      const applyLabel = document.createElement("label");
      applyLabel.append(applyTags, document.createTextNode(" Apply tags "), tagMode);
      tagGroup.append(applyLabel);
    }
    const checkedTags: HTMLInputElement[] = [];
    for (const tag of tags) {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = tag.id;
      checkbox.checked = !batch && models[0].tags.includes(tag.name);
      checkedTags.push(checkbox);
      label.append(
        checkbox,
        document.createTextNode(`${tag.category ? `${tag.category}: ` : ""}${tag.name}`),
      );
      tagGroup.append(label);
    }
    const manageTags = this.button("Manage tags…", () => this.onManageTags());
    tagGroup.append(manageTags);
    form.append(tagGroup);

    const actions = document.createElement("footer");
    if (!batch) actions.append(this.button("Print history…", () => this.onHistory(models[0])));
    const save = this.button("Save", () => void saveChanges());
    save.classList.add("primary");
    actions.append(save);
    form.append(error, actions);
    this.dialog.append(header, form);

    const saveChanges = async () => {
      error.textContent = "";
      if (!form.reportValidity()) return;
      const fields = new Set<string>();
      for (const key of Object.keys(values)) {
        if (!batch || applied.get(key)?.checked) fields.add(key);
      }
      if (!batch || applyTags?.checked) fields.add("tags");
      const raw = Object.fromEntries(
        Object.entries(values).map(([key, input]) => [key, input.value]),
      );
      const patch = buildMetadataPatch(
        raw,
        fields,
        checkedTags.filter((input) => input.checked).map((input) => input.value),
        tagMode.value as "replace" | "merge",
      );
      if (!Object.keys(patch).length) {
        error.textContent = "Choose at least one field to apply.";
        return;
      }
      save.disabled = true;
      try {
        await this.onSave(
          versionedMetadataTargets(
            models,
            models.map((model) => model.id),
          ),
          patch,
        );
        this.close();
      } catch (reason) {
        error.textContent = isMetadataVersionError(reason)
          ? "This model changed since the editor opened. Reload the catalog and try again."
          : String((reason as { message?: unknown })?.message ?? reason);
      } finally {
        save.disabled = false;
      }
    };
    if (!this.dialog.open) this.dialog.showModal();
    (form.querySelector("input, textarea, select") as HTMLElement | null)?.focus();
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  dismiss(): void {
    this.close();
  }

  private close(): void {
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    const fallback = this.restoreFocus;
    this.restoreFocus = () => {};
    restoreLibraryFocus(invoker, fallback);
  }
}
