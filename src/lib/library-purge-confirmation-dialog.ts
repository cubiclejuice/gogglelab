function purgeErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  )
    return error.message;
  return String(error);
}

/** Explicit native confirmation for the existing backup-and-purge operation. */
export class LibraryPurgeConfirmationDialog {
  private readonly dialog: HTMLDialogElement;
  private readonly destination: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly status: HTMLElement;
  private readonly submit: HTMLButtonElement;
  private readonly cancel: HTMLButtonElement;
  private invoker: HTMLElement | null = null;
  private pending = false;
  private revision = 0;
  private onConfirm: (token: string, destination: string) => Promise<void> = async () => {};
  private onCancel: () => void = () => {};
  private destinationPath = "";

  constructor(
    container: HTMLElement,
    private readonly focusFallback: () => void = () => {},
  ) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-editor-dialog library-purge-confirmation-dialog";
    this.dialog.setAttribute("aria-labelledby", "library-purge-confirmation-title");
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.id = "library-purge-confirmation-title";
    title.textContent = "Back up and purge catalog";
    header.append(title);
    const explanation = document.createElement("p");
    explanation.textContent =
      "A restorable backup will be written to the destination below before the catalog is purged. External model sources stay untouched; the previous Library is preserved.";
    const destinationLabel = document.createElement("strong");
    destinationLabel.textContent = "Backup destination";
    this.destination = document.createElement("p");
    this.destination.style.overflowWrap = "anywhere";
    const form = document.createElement("form");
    form.className = "library-editor-form";
    const label = document.createElement("label");
    label.className = "library-editor-field";
    label.textContent = "Type PURGE exactly";
    this.input = document.createElement("input");
    this.input.type = "text";
    this.input.autocomplete = "off";
    this.input.spellcheck = false;
    this.input.required = true;
    this.input.addEventListener("input", () => this.updateControls());
    label.append(this.input);
    this.status = document.createElement("p");
    this.status.className = "library-editor-error";
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");
    this.submit = document.createElement("button");
    this.submit.type = "submit";
    this.submit.className = "btn danger";
    this.submit.textContent = "Back up and purge catalog";
    this.cancel = document.createElement("button");
    this.cancel.type = "button";
    this.cancel.className = "btn";
    this.cancel.textContent = "Cancel";
    this.cancel.addEventListener("click", () => this.cancelConfirmation());
    const actions = document.createElement("div");
    actions.className = "settings-actions";
    actions.append(this.submit, this.cancel);
    form.append(label, this.status, actions);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.confirm();
    });
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      if (!this.pending) this.cancelConfirmation();
    });
    this.dialog.addEventListener("close", () => {
      this.revision += 1;
      if (this.invoker?.isConnected) this.invoker.focus();
      else this.focusFallback();
    });
    this.dialog.append(header, explanation, destinationLabel, this.destination, form);
    container.append(this.dialog);
    this.updateControls();
  }

  show(
    destination: string,
    onConfirm: (token: string, path: string) => Promise<void>,
    onCancel: () => void = () => {},
  ): boolean {
    if (this.pending) return false;
    this.revision += 1;
    this.invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.destinationPath = destination;
    this.destination.textContent = destination;
    this.onConfirm = onConfirm;
    this.onCancel = onCancel;
    this.input.value = "";
    this.status.textContent = "";
    this.updateControls();
    if (!this.dialog.open) this.dialog.showModal();
    this.input.focus();
    return true;
  }

  dismiss(): void {
    this.revision += 1;
    if (this.dialog.open) this.dialog.close();
  }
  private cancelConfirmation(): void {
    if (this.pending) return;
    this.onCancel();
    this.dialog.close();
  }
  private updateControls(): void {
    this.input.disabled = this.pending;
    this.submit.disabled = this.pending || this.input.value !== "PURGE";
    this.cancel.disabled = this.pending;
    this.dialog.setAttribute("aria-busy", String(this.pending));
  }
  private async confirm(): Promise<void> {
    if (this.pending || this.input.value !== "PURGE" || !this.dialog.open) return;
    const revision = this.revision;
    const destination = this.destinationPath;
    const operation = this.onConfirm;
    this.pending = true;
    this.status.textContent = "Creating the backup and purging the catalog…";
    this.updateControls();
    try {
      await operation("PURGE", destination);
      if (revision === this.revision && this.dialog.open) this.dialog.close();
    } catch (error) {
      if (revision === this.revision && this.dialog.open) {
        this.status.textContent = purgeErrorMessage(error);
      }
    } finally {
      this.pending = false;
      if (revision === this.revision) {
        this.updateControls();
        if (this.dialog.open) this.input.focus();
      }
    }
  }
}
