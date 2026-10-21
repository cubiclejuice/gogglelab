import type { DuplicateGroup, LibraryModel } from "./ipc";

export class LibraryDuplicatesDialog {
  private readonly dialog: HTMLDialogElement;
  private invoker: HTMLElement | null = null;
  onGetThumbnail: (modelId: string) => Promise<string | null> = async () => null;
  onRemove: (models: LibraryModel[]) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-duplicates-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
  }

  show(groups: DuplicateGroup[], scopeCount: number, invoker?: HTMLElement): void {
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = "Duplicate review";
    const scope = document.createElement("p");
    scope.textContent = `${groups.length} groups within exactly ${scopeCount} available models from the current filter.`;
    header.append(
      title,
      this.button("Close", () => this.close()),
    );
    const status = document.createElement("div");
    status.className = "library-editor-error";
    status.setAttribute("role", "alert");
    this.dialog.append(header, scope, status);
    for (const group of groups) this.dialog.append(this.group(group, status));
    if (!groups.length) {
      const empty = document.createElement("p");
      empty.textContent = "No content-hash duplicates were found in this scope.";
      this.dialog.append(empty);
    }
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
  }

  dismiss(): void {
    this.close();
  }

  private group(group: DuplicateGroup, status: HTMLElement): HTMLElement {
    const section = document.createElement("section");
    const heading = document.createElement("h3");
    heading.textContent = `${group.models.length} identical models`;
    const models = document.createElement("div");
    models.className = "library-duplicate-models";
    const selections: Array<{ model: LibraryModel; input: HTMLInputElement }> = [];
    for (const model of group.models) {
      const card = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.setAttribute("aria-label", `Remove ${model.display_name}`);
      const image = document.createElement("img");
      image.alt = "";
      void this.onGetThumbnail(model.id)
        .then((uri) => {
          if (!image.isConnected) return;
          if (uri) image.src = uri;
          else {
            image.classList.add("is-unavailable");
            image.alt = "Preview unavailable";
          }
        })
        .catch((error) => {
          if (!image.isConnected) return;
          image.classList.add("is-unavailable");
          image.alt = "Preview unavailable";
          status.textContent = `One or more previews could not load: ${String((error as { message?: unknown })?.message ?? error)}`;
        });
      const name = document.createElement("strong");
      name.textContent = model.display_name;
      card.append(checkbox, image, name);
      models.append(card);
      selections.push({ model, input: checkbox });
    }
    const remove = this.button("Remove checked records…", () => {
      const selected = selections.filter(({ input }) => input.checked).map(({ model }) => model);
      if (!selected.length) {
        status.textContent = "Check one or more duplicate records first.";
        return;
      }
      if (
        !window.confirm(
          `Remove ${selected.length} checked catalog records? Source files stay untouched.`,
        )
      )
        return;
      void (async () => {
        remove.disabled = true;
        status.textContent = "";
        try {
          await this.onRemove(selected);
        } catch (error) {
          status.textContent = `Could not remove the checked records: ${String((error as { message?: unknown })?.message ?? error)}. Reload or retry with the same checked models.`;
        } finally {
          remove.disabled = false;
        }
      })();
    });
    section.append(heading, models, remove);
    return section;
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private close(): void {
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }
}
