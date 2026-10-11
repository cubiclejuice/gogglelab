import type { Filament, LibraryModel, PrintAttempt, PrintAttemptInput } from "./ipc";
import { filamentLabel, filamentReferenceLabel } from "./library-filaments";

export function validatePrintAttempt(input: PrintAttemptInput): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(input.printed_at)) return "Choose a valid print date.";
  if (!Number.isInteger(input.quantity) || input.quantity < 1)
    return "Quantity must be at least 1.";
  if (input.filament_ids.length > 5) return "Use no more than five filament IDs.";
  return null;
}

export function printAttemptSummary(attempt: PrintAttempt): string {
  return `${attempt.printed_at.slice(0, 10)} · ${attempt.quantity} · ${attempt.outcome}`;
}

export function localDateInputValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export class LibraryPrintHistory {
  private readonly dialog: HTMLDialogElement;
  private model: LibraryModel | null = null;
  private attempts: PrintAttempt[] = [];
  private filaments: Filament[] = [];
  private invoker: HTMLElement | null = null;
  onCreate: (modelId: string, input: PrintAttemptInput) => Promise<void> = async () => {};
  onUpdate: (attemptId: string, input: PrintAttemptInput) => Promise<void> = async () => {};
  onDelete: (attemptId: string) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-history-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    container.append(this.dialog);
  }

  show(
    model: LibraryModel,
    attempts: PrintAttempt[],
    filaments: Filament[] = [],
    invoker?: HTMLElement,
  ): void {
    this.model = model;
    this.attempts = attempts;
    this.filaments = filaments;
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.render();
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLInputElement>("input")?.focus();
  }

  update(attempts: PrintAttempt[]): void {
    this.attempts = attempts;
    if (this.dialog.open) this.render();
  }

  private render(): void {
    const model = this.model;
    if (!model) return;
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = `Print history · ${model.display_name}`;
    header.append(
      title,
      this.button("Close", () => this.close()),
    );
    const error = document.createElement("div");
    error.className = "library-editor-error";
    error.setAttribute("role", "alert");
    const create = this.attemptForm(null, error);
    const list = document.createElement("div");
    list.className = "library-history-list";
    if (!this.attempts.length) {
      const empty = document.createElement("p");
      empty.textContent = "No print attempts recorded.";
      list.append(empty);
    }
    for (const attempt of this.attempts) list.append(this.attemptForm(attempt, error));
    this.dialog.append(header, error, create, list);
  }

  private attemptForm(attempt: PrintAttempt | null, error: HTMLElement): HTMLFormElement {
    const form = document.createElement("form");
    form.className = "library-history-form";
    form.setAttribute("aria-label", attempt ? printAttemptSummary(attempt) : "Log print attempt");
    form.addEventListener("submit", (event) => event.preventDefault());
    const date = this.input(
      "Print date",
      "date",
      attempt?.printed_at.slice(0, 10) ?? localDateInputValue(new Date()),
    );
    const quantity = this.input("Quantity", "number", String(attempt?.quantity ?? 1));
    quantity.min = "1";
    const filaments = document.createElement("select");
    filaments.multiple = true;
    filaments.setAttribute("aria-label", "Filaments used, maximum five");
    const selectedIds = attempt?.filament_ids ?? [];
    for (const filament of this.filaments) {
      const unavailable = filament.archived || filament.source_missing;
      if (unavailable && !selectedIds.includes(filament.id)) continue;
      const option = new Option(
        unavailable
          ? `${filamentLabel(filament)} (unavailable; deselect to remove)`
          : filamentLabel(filament),
        filament.id,
      );
      option.selected = selectedIds.includes(filament.id);
      filaments.add(option);
    }
    const known = new Set(this.filaments.map((filament) => filament.id));
    for (const id of selectedIds.filter((value) => !known.has(value))) {
      const option = new Option(filamentReferenceLabel(id, this.filaments), id);
      option.selected = true;
      filaments.add(option);
    }
    const outcome = document.createElement("select");
    outcome.setAttribute("aria-label", "Print outcome");
    outcome.add(new Option("Printed", "printed"));
    outcome.add(new Option("Failed", "failed"));
    outcome.value = attempt?.outcome ?? "printed";
    const notes = document.createElement("textarea");
    notes.placeholder = "Print notes";
    notes.setAttribute("aria-label", "Print notes");
    notes.value = attempt?.notes ?? "";
    const save = this.button(attempt ? "Save attempt" : "Log attempt", () => void submit());
    const submit = async () => {
      const input: PrintAttemptInput = {
        printed_at: date.value,
        quantity: Number(quantity.value),
        filament_ids: [...filaments.selectedOptions].map((option) => option.value),
        notes: notes.value,
        outcome: outcome.value as "printed" | "failed",
      };
      const problem = validatePrintAttempt(input);
      if (problem) {
        error.textContent = problem;
        return;
      }
      error.textContent = "";
      save.disabled = true;
      try {
        const nativeInput = { ...input, printed_at: `${input.printed_at}T00:00:00.000Z` };
        if (attempt) await this.onUpdate(attempt.id, nativeInput);
        else if (this.model) await this.onCreate(this.model.id, nativeInput);
      } catch (reason) {
        error.textContent = String((reason as { message?: unknown })?.message ?? reason);
      } finally {
        save.disabled = false;
      }
    };
    form.append(date, quantity, filaments, outcome, notes, save);
    if (attempt) {
      form.append(
        this.button("Delete", () => {
          if (!window.confirm(`Delete the ${attempt.printed_at} print attempt?`)) return;
          void (async () => {
            error.textContent = "";
            try {
              await this.onDelete(attempt.id);
            } catch (reason) {
              error.textContent = String((reason as { message?: unknown })?.message ?? reason);
            }
          })();
        }),
      );
    }
    return form;
  }

  private input(label: string, type: string, value: string): HTMLInputElement {
    const input = document.createElement("input");
    input.type = type;
    input.value = value;
    input.placeholder = label;
    input.setAttribute("aria-label", label);
    return input;
  }

  dismiss(): void {
    this.close();
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
