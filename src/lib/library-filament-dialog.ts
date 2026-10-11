import type { Filament, FilamentInput, LibraryModel, VersionedModelTarget } from "./ipc";
import {
  assignmentIdsPreservingUnknown,
  filamentLabel,
  filamentReferenceLabel,
} from "./library-filaments";

export class LibraryFilamentDialog {
  private readonly dialog: HTMLDialogElement;
  private invoker: HTMLElement | null = null;
  onCreate: (input: FilamentInput) => Promise<void> = async () => {};
  onUpdate: (id: string, version: number, input: FilamentInput) => Promise<void> = async () => {};
  onArchive: (id: string, version: number) => Promise<void> = async () => {};
  onAssign: (
    updates: Array<{ target: VersionedModelTarget; filamentIds: string[] }>,
  ) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-filament-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
  }

  show(filaments: Filament[], models: LibraryModel[], invoker?: HTMLElement): void {
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = "Filaments";
    header.append(
      title,
      this.button("Close", () => this.close()),
    );
    const status = document.createElement("div");
    status.className = "library-editor-error";
    status.setAttribute("role", "alert");
    const assignment = document.createElement("section");
    const assignmentTitle = document.createElement("h3");
    assignmentTitle.textContent = `Assign to ${models.length} selected model${models.length === 1 ? "" : "s"}`;
    const selectableIds = new Set(
      filaments
        .filter((filament) => !filament.archived && !filament.source_missing)
        .map((filament) => filament.id),
    );
    const existing = models.length === 1 ? (models[0].filament_ids ?? []) : [];
    const unavailable = [
      ...new Set(
        models.flatMap((model) => model.filament_ids ?? []).filter((id) => !selectableIds.has(id)),
      ),
    ];
    const removedUnknown = new Set<string>();
    for (const id of unavailable) {
      const chip = this.button(`${filamentReferenceLabel(id, filaments)} · remove`, () => {
        removedUnknown.add(id);
        chip.disabled = true;
      });
      assignment.append(chip);
    }
    const checks: HTMLInputElement[] = [];
    for (const filament of filaments.filter((entry) => !entry.archived && !entry.source_missing)) {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = filament.id;
      checkbox.checked = existing.includes(filament.id);
      checks.push(checkbox);
      label.append(checkbox, document.createTextNode(filamentLabel(filament)));
      assignment.append(label);
    }
    const saveAssignment = this.button(
      "Save assignments",
      () =>
        void this.runBusy(saveAssignment, status, async () => {
          const selectedKnown = checks.filter((check) => check.checked).map((check) => check.value);
          const updates = models.map((model) => ({
            target: { model_id: model.id, expected_version: model.metadata_version },
            filamentIds: assignmentIdsPreservingUnknown(
              model.filament_ids ?? [],
              filaments,
              selectedKnown,
              removedUnknown,
            ),
          }));
          if (updates.some((update) => update.filamentIds.length > 5)) {
            throw new Error("Choose no more than five filament references per model.");
          }
          await this.onAssign(updates);
        }),
    );
    assignment.append(assignmentTitle, saveAssignment);

    const catalog = document.createElement("section");
    const catalogTitle = document.createElement("h3");
    catalogTitle.textContent = "Local filament catalog";
    catalog.append(catalogTitle, this.filamentForm(null, status));
    for (const filament of filaments) catalog.append(this.filamentForm(filament, status));
    this.dialog.append(header, status, assignment, catalog);
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
  }

  private filamentForm(filament: Filament | null, status: HTMLElement): HTMLElement {
    const row = document.createElement("div");
    row.className = "library-filament-row";
    const fields = ["name", "brand", "material", "color", "notes"] as const;
    const inputs = Object.fromEntries(
      fields.map((field) => {
        const input = document.createElement("input");
        input.value = filament?.[field] ?? "";
        input.placeholder = field;
        input.setAttribute("aria-label", `${filament ? "Edit" : "New"} filament ${field}`);
        input.disabled = Boolean(filament && filament.origin !== "manual");
        row.append(input);
        return [field, input];
      }),
    ) as Record<(typeof fields)[number], HTMLInputElement>;
    const save = this.button(
      filament ? "Save" : "Create",
      () =>
        void this.runBusy(save, status, async () => {
          const input: FilamentInput = Object.fromEntries(
            fields.map((field) => [field, inputs[field].value]),
          ) as unknown as FilamentInput;
          if (filament) await this.onUpdate(filament.id, filament.version, input);
          else await this.onCreate(input);
        }),
    );
    save.disabled = Boolean(filament && filament.origin !== "manual");
    row.append(save);
    if (filament?.origin === "manual") {
      row.append(
        this.button(
          "Archive",
          () => void this.run(status, () => this.onArchive(filament.id, filament.version)),
        ),
      );
    }
    return row;
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private async run(status: HTMLElement, action: () => Promise<void>): Promise<void> {
    status.textContent = "";
    try {
      await action();
    } catch (error) {
      status.textContent = String((error as { message?: unknown })?.message ?? error);
    }
  }

  private async runBusy(
    button: HTMLButtonElement,
    status: HTMLElement,
    action: () => Promise<void>,
  ): Promise<void> {
    button.disabled = true;
    try {
      await this.run(status, action);
    } finally {
      button.disabled = false;
    }
  }

  private close(): void {
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  dismiss(): void {
    this.close();
  }
}
