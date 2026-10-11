import type { LibraryTag } from "./ipc";

export class LibraryTagManager {
  private readonly dialog: HTMLDialogElement;
  private tags: LibraryTag[] = [];
  private invoker: HTMLElement | null = null;
  onCreate: (name: string, category: string | null) => Promise<void> = async () => {};
  onUpdate: (id: string, name: string, category: string | null) => Promise<void> = async () => {};
  onDelete: (id: string) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-tag-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
  }

  show(tags: LibraryTag[], invoker?: HTMLElement): void {
    this.tags = tags;
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.render();
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLInputElement>("input")?.focus();
  }

  update(tags: LibraryTag[]): void {
    this.tags = tags;
    if (this.dialog.open) this.render();
  }

  private render(): void {
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = "Manage tags";
    header.append(
      title,
      this.button("Close", () => this.close()),
    );
    const error = document.createElement("div");
    error.className = "library-editor-error";
    error.setAttribute("role", "alert");
    const createRow = document.createElement("div");
    createRow.className = "library-tag-create";
    const name = this.input("New tag name");
    const category = this.input("Category (optional)");
    createRow.append(
      name,
      category,
      this.button(
        "Create tag",
        () =>
          void this.run(error, async () => {
            await this.onCreate(name.value, category.value || null);
          }),
      ),
    );
    const list = document.createElement("div");
    list.className = "library-tag-list";
    for (const tag of this.tags) {
      const row = document.createElement("div");
      const tagName = this.input("Tag name", tag.name);
      const tagCategory = this.input("Category", tag.category ?? "");
      row.append(
        tagName,
        tagCategory,
        this.button(
          "Save",
          () =>
            void this.run(error, () =>
              this.onUpdate(tag.id, tagName.value, tagCategory.value || null),
            ),
        ),
        this.button("Delete", () => {
          if (!window.confirm(`Delete the tag “${tag.name}”? Models will keep their other tags.`))
            return;
          void this.run(error, () => this.onDelete(tag.id));
        }),
      );
      list.append(row);
    }
    this.dialog.append(header, error, createRow, list);
  }

  private input(label: string, value = ""): HTMLInputElement {
    const input = document.createElement("input");
    input.type = "text";
    input.value = value;
    input.placeholder = label;
    input.setAttribute("aria-label", label);
    return input;
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private async run(error: HTMLElement, action: () => Promise<void>): Promise<void> {
    error.textContent = "";
    try {
      await action();
    } catch (reason) {
      error.textContent = String((reason as { message?: unknown })?.message ?? reason);
    }
  }

  private close(): void {
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }
}
