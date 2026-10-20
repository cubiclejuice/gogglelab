/** Native dialog for collection names; WKWebView does not rely on window.prompt. */
export class LibraryCollectionNameDialog {
  private readonly dialog: HTMLDialogElement;
  private readonly title: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly message: HTMLElement;
  private readonly save: HTMLButtonElement;
  private readonly cancel: HTMLButtonElement;
  private onSave: (name: string) => Promise<void> = async () => {};
  private pending = false;
  private revision = 0;
  private invoker: HTMLElement | null = null;

  constructor(
    container: HTMLElement,
    private readonly focusFallback: () => void = () => {},
  ) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-editor-dialog";
    this.dialog.setAttribute("aria-labelledby", "library-collection-name-title");
    const header = document.createElement("header");
    this.title = document.createElement("h2");
    this.title.id = "library-collection-name-title";
    header.append(this.title);
    const form = document.createElement("form");
    form.className = "library-editor-form";
    const label = document.createElement("label");
    label.className = "library-editor-field";
    label.textContent = "Collection name";
    this.input = document.createElement("input");
    this.input.type = "text";
    this.input.required = true;
    label.append(this.input);
    this.message = document.createElement("p");
    this.message.setAttribute("role", "status");
    this.message.setAttribute("aria-live", "polite");
    this.save = document.createElement("button");
    this.save.type = "submit";
    this.save.className = "btn primary";
    this.cancel = document.createElement("button");
    this.cancel.type = "button";
    this.cancel.className = "btn";
    this.cancel.textContent = "Cancel";
    this.cancel.addEventListener("click", () => this.dialog.close());
    const actions = document.createElement("div");
    actions.className = "settings-actions";
    actions.append(this.save, this.cancel);
    form.append(label, this.message, actions);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.dialog.addEventListener("cancel", (event) => {
      if (this.pending) event.preventDefault();
    });
    this.dialog.addEventListener("close", () => {
      this.revision += 1;
      if (this.invoker?.isConnected) this.invoker.focus();
      else this.focusFallback();
    });
    this.dialog.append(header, form);
    container.append(this.dialog);
  }

  show(title: string, initialName: string, onSave: (name: string) => Promise<void>): void {
    this.revision += 1;
    this.invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.onSave = onSave;
    this.title.textContent = title;
    this.input.value = initialName;
    this.message.textContent = "";
    this.pending = false;
    this.input.disabled = false;
    this.save.disabled = false;
    this.cancel.disabled = false;
    this.save.textContent = initialName ? "Save name" : "Create collection";
    if (!this.dialog.open) this.dialog.showModal();
    this.input.focus();
    this.input.select();
  }

  dismiss(): void {
    if (this.dialog.open) this.dialog.close();
  }
  dispose(): void {
    this.dismiss();
    this.dialog.remove();
  }

  private async submit(): Promise<void> {
    if (this.pending) return;
    const name = this.input.value.trim();
    if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\\")) {
      this.message.textContent = "Enter a collection name without slashes or a dot-only name.";
      this.input.focus();
      return;
    }
    const revision = this.revision;
    this.pending = true;
    this.input.disabled = true;
    this.save.disabled = true;
    this.cancel.disabled = true;
    this.message.textContent = "Saving collection…";
    try {
      await this.onSave(name);
      if (revision === this.revision && this.dialog.open) this.dialog.close();
    } catch (error) {
      if (revision === this.revision && this.dialog.open) {
        this.message.textContent = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (revision === this.revision) {
        this.pending = false;
        this.input.disabled = false;
        this.save.disabled = false;
        this.cancel.disabled = false;
        if (this.dialog.open) this.input.focus();
      }
    }
  }
}
