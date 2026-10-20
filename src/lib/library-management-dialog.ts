import type { RemovedModel } from "./ipc";
import { LibraryPurgeConfirmationDialog } from "./library-purge-confirmation-dialog";

export class LibraryManagementDialog {
  private readonly dialog: HTMLDialogElement;
  private invoker: HTMLElement | null = null;
  private readonly purgeConfirmation: LibraryPurgeConfirmationDialog;
  private revision = 0;
  private choosingPurgeDestination = false;
  private purgePending = false;
  onRestoreRemoved: (snapshotId: string) => Promise<void> = async () => {};
  onBackup: () => Promise<void> = async () => {};
  onRestoreBackup: () => Promise<void> = async () => {};
  onChoosePurgeBackupDestination: () => Promise<string | null> = async () => null;
  onPurge: (confirmation: string, destinationPath: string) => Promise<void> = async () => {};
  onCleanup: () => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-management-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
    this.purgeConfirmation = new LibraryPurgeConfirmationDialog(container, () => {
      if (this.dialog.open) this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
    });
  }

  show(removedModels: RemovedModel[], invoker?: HTMLElement): void {
    this.revision += 1;
    this.purgeConfirmation.dismiss();
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = "Library management";
    header.append(
      title,
      this.button("Close", () => this.close()),
    );
    const status = document.createElement("div");
    status.className = "library-editor-error";
    status.setAttribute("role", "status");
    const maintenance = document.createElement("section");
    const maintenanceTitle = document.createElement("h3");
    maintenanceTitle.textContent = "Backup and maintenance";
    maintenance.append(
      maintenanceTitle,
      this.button("Create full backup…", () => void this.run(status, this.onBackup)),
      this.button("Restore full backup…", () => void this.run(status, this.onRestoreBackup)),
      this.button("Clean orphan cache", () => void this.run(status, this.onCleanup)),
      this.button("Back up and purge catalog…", () => void this.confirmPurge(status)),
    );
    const removed = document.createElement("section");
    const removedTitle = document.createElement("h3");
    removedTitle.textContent = `Removed items (${removedModels.length})`;
    removed.append(removedTitle);
    if (!removedModels.length) {
      const empty = document.createElement("p");
      empty.textContent = "No removed models are awaiting restore.";
      removed.append(empty);
    }
    for (const entry of removedModels) {
      const row = document.createElement("div");
      const description = document.createElement("span");
      description.textContent = `${entry.model.display_name} · removed ${entry.removedAt}`;
      row.append(
        description,
        this.button(
          "Restore",
          () => void this.run(status, () => this.onRestoreRemoved(entry.snapshotId)),
        ),
      );
      removed.append(row);
    }
    this.dialog.append(header, status, maintenance, removed);
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
  }

  private async confirmPurge(status: HTMLElement): Promise<void> {
    if (this.choosingPurgeDestination || this.purgePending) return;
    const revision = this.revision;
    const current = () => revision === this.revision && this.dialog.open && this.dialog.isConnected;
    this.choosingPurgeDestination = true;
    try {
      const destination = await this.onChoosePurgeBackupDestination();
      if (!current()) return;
      if (!destination) {
        status.textContent = "Catalog purge canceled before choosing a backup destination.";
        return;
      }
      const operation = this.onPurge;
      this.purgeConfirmation.show(
        destination,
        async (confirmation, destinationPath) => {
          if (!current() || this.purgePending) return;
          this.purgePending = true;
          try {
            await operation(confirmation, destinationPath);
          } finally {
            this.purgePending = false;
          }
        },
        () => {
          if (current()) status.textContent = "Catalog purge canceled.";
        },
      );
    } catch (error) {
      if (current()) status.textContent = error instanceof Error ? error.message : String(error);
    } finally {
      this.choosingPurgeDestination = false;
    }
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

  private async run(status: HTMLElement, action: () => Promise<void>): Promise<void> {
    status.textContent = "";
    try {
      await action();
    } catch (error) {
      status.textContent = String((error as { message?: unknown })?.message ?? error);
    }
  }

  private close(): void {
    this.revision += 1;
    this.purgeConfirmation.dismiss();
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    if (invoker?.isConnected) invoker.focus();
  }
}
