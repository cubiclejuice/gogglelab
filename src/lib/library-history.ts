import type {
  Filament,
  LibraryModel,
  PartStock,
  PartStockInput,
  PrintAttempt,
  PrintAttemptInput,
  PrintAttemptPart,
  PrintAttemptPartInput,
} from "./ipc";
import { filamentLabel, filamentReferenceLabel } from "./library-filaments";

export function validatePrintAttempt(input: PrintAttemptInput): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(input.printed_at)) return "Choose a valid print date.";
  if (!Number.isInteger(input.quantity) || input.quantity < 1 || input.quantity > 10_000)
    return "Quantity must be a whole number from 1 to 10,000.";
  if (input.filament_ids.length > 5) return "Use no more than five filament IDs.";
  if (input.parts !== undefined) {
    if (input.parts.length > 256) return "Use no more than 256 parts per print event.";
    const seen = new Set<string>();
    for (const part of input.parts) {
      if (
        !part.id.trim() ||
        !Number.isInteger(part.quantity) ||
        part.quantity < 1 ||
        part.quantity > 9_999
      )
        return "Each part needs a selected stock item and a whole per-copy quantity from 1 to 9,999.";
      if (seen.has(part.id)) return "Choose each stock part only once.";
      seen.add(part.id);
    }
  }
  return null;
}

export interface PartUsageEstimate {
  id: string;
  name: string;
  category: string | null;
  unit: string;
  quantityPerCopy: number;
  quantity: number;
  available: number;
  sufficient: boolean;
}

export function calculatePartUsage(
  parts: readonly PrintAttemptPartInput[],
  copies: number,
  stock: readonly PartStock[],
  releasing: readonly PrintAttemptPart[] = [],
): PartUsageEstimate[] {
  const stockById = new Map(stock.map((part) => [part.id, part]));
  const releasedById = new Map<string, number>();
  for (const part of releasing) {
    if (part.id) releasedById.set(part.id, (releasedById.get(part.id) ?? 0) + part.quantity);
  }
  return parts.flatMap((part) => {
    const record = stockById.get(part.id);
    if (!record || !Number.isSafeInteger(copies) || copies < 1) return [];
    const total = part.quantity * copies;
    const available = record.quantity + (releasedById.get(part.id) ?? 0);
    return [
      {
        id: part.id,
        name: record.name,
        category: record.category,
        unit: record.unit,
        quantityPerCopy: part.quantity,
        quantity: total,
        available,
        sufficient: total <= available,
      },
    ];
  });
}

export function partStockValidationError(input: PartStockInput): string | null {
  const boundedText = (value: string, maxLength: number): boolean =>
    Array.from(value.trim()).length <= maxLength;
  if (!input.name.trim() || !boundedText(input.name, 128))
    return "Part name is required and must be 128 characters or fewer.";
  if (!input.unit.trim() || !boundedText(input.unit, 128))
    return "Part unit is required and must be 128 characters or fewer.";
  if (input.category !== null && !boundedText(input.category, 128))
    return "Part category must be 128 characters or fewer.";
  if (!Number.isInteger(input.quantity) || input.quantity < 0 || input.quantity > 1_000_000)
    return "Stock quantity must be a whole number from 0 to 1,000,000.";
  if (!Number.isInteger(input.lowStock) || input.lowStock < 0 || input.lowStock > 1_000_000)
    return "Low-stock threshold must be a whole number from 0 to 1,000,000.";
  if (new TextEncoder().encode(input.notes).length > 20_000)
    return "Part notes must be 20,000 bytes or fewer.";
  return null;
}

export function describePartStockError(reason: unknown): string {
  const message = String((reason as { message?: unknown })?.message ?? reason);
  const normalized = message.toLocaleLowerCase();
  if (normalized.includes("insufficient stock") || normalized.includes("not enough stock")) {
    return `Not enough stock for this event. Review Parts stock or lower the per-copy quantity. ${message}`;
  }
  if (normalized.includes("version conflict") || normalized.includes("stock conflict")) {
    return `Part stock changed since it was loaded. Refresh the stock list and try again. ${message}`;
  }
  return message;
}

export function printAttemptSummary(attempt: PrintAttempt): string {
  const printer = attempt.printer_name ? ` · ${attempt.printer_name}` : "";
  const parts =
    attempt.parts && attempt.parts.length > 0
      ? ` · ${attempt.parts.map((part) => `${part.name}${part.category ? ` (${part.category})` : ""} ${part.quantity} ${part.unit}`).join(", ")}`
      : "";
  const stockWarning = printAttemptStockWarning(
    attempt.stock_adjusted,
    (attempt.parts?.length ?? 0) > 0,
  );
  return `${attempt.printed_at.slice(0, 10)} · ${attempt.quantity} · ${attempt.outcome}${printer}${parts}${stockWarning ? ` · ${stockWarning}` : ""}`;
}

export function printAttemptStockWarning(
  stockAdjusted: boolean | null | undefined,
  hasParts: boolean,
): string {
  return stockAdjusted === null && hasParts
    ? "Stock impact unknown; deleting this event will leave stock unchanged"
    : "";
}

export function localDateInputValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export interface PrintHistoryPrinter {
  id: string;
  name: string;
}

export const KEEP_HISTORICAL_PRINTER = "__keep_historical_printer__";

export function initialPrinterSelection(
  printers: readonly PrintHistoryPrinter[],
  activePrinterId: string | null,
  attempt: Pick<PrintAttempt, "printer_id" | "printer_name"> | null,
): string {
  if (attempt) {
    if (attempt.printer_id) return attempt.printer_id;
    return attempt.printer_name ? KEEP_HISTORICAL_PRINTER : "";
  }
  return printers.some((printer) => printer.id === activePrinterId) ? activePrinterId! : "";
}

/** `undefined` means preserve the stored snapshot; `null` explicitly clears it. */
export function printerIdChangeForAttempt(
  selected: string,
  attempt: Pick<PrintAttempt, "printer_id" | "printer_name"> | null,
): string | null | undefined {
  if (selected === KEEP_HISTORICAL_PRINTER) return undefined;
  const nextId = selected || null;
  if (!attempt) return nextId;
  const currentId = attempt.printer_id ?? null;
  if (nextId !== currentId) return nextId;
  if (nextId === null && attempt.printer_name) return null;
  return undefined;
}

export class LibraryPrintHistory {
  private readonly dialog: HTMLDialogElement;
  private model: LibraryModel | null = null;
  private attempts: PrintAttempt[] = [];
  private filaments: Filament[] = [];
  private partStock: PartStock[] = [];
  private printers: PrintHistoryPrinter[] = [];
  private activePrinterId: string | null = null;
  private invoker: HTMLElement | null = null;
  onCreate: (modelId: string, input: PrintAttemptInput, printerId: string | null) => Promise<void> =
    async () => {};
  onUpdate: (
    attemptId: string,
    input: PrintAttemptInput,
    printerId: string | null | undefined,
  ) => Promise<void> = async () => {};
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

  setPrinterOptions(
    printers: readonly PrintHistoryPrinter[],
    activePrinterId: string | null,
  ): void {
    this.printers = printers.map((printer) => ({ ...printer }));
    this.activePrinterId = activePrinterId;
  }

  show(
    model: LibraryModel,
    attempts: PrintAttempt[],
    filaments: Filament[] = [],
    invoker?: HTMLElement,
    partStock: PartStock[] = [],
  ): void {
    this.model = model;
    this.attempts = attempts;
    this.filaments = filaments;
    this.partStock = partStock;
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

  setPartStock(parts: PartStock[]): void {
    this.partStock = parts;
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
    const printer = document.createElement("select");
    printer.setAttribute("aria-label", "Printer used");
    printer.add(new Option("No printer recorded", ""));
    for (const item of this.printers) printer.add(new Option(item.name, item.id));
    if (attempt?.printer_id && !this.printers.some((item) => item.id === attempt.printer_id)) {
      const removed = new Option(
        `${attempt.printer_name || "Removed printer"} (historical)`,
        attempt.printer_id,
      );
      removed.disabled = true;
      printer.add(removed);
    } else if (attempt && !attempt.printer_id && attempt.printer_name) {
      const removed = new Option(
        `${attempt.printer_name} (historical; removed)`,
        KEEP_HISTORICAL_PRINTER,
      );
      removed.disabled = true;
      printer.add(removed);
    }
    printer.value = initialPrinterSelection(this.printers, this.activePrinterId, attempt);
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
    outcome.add(new Option("Cancelled", "cancelled"));
    outcome.value = attempt?.outcome ?? "printed";
    const notes = document.createElement("textarea");
    notes.placeholder = "Print notes";
    notes.setAttribute("aria-label", "Print notes");
    notes.value = attempt?.notes ?? "";
    const partFieldset = document.createElement("fieldset");
    const partLegend = document.createElement("legend");
    partLegend.textContent = "Parts per copy";
    const partHelp = document.createElement("p");
    partHelp.textContent =
      "Printed, failed, and cancelled events all consume stock. Totals multiply per-copy use by print quantity. Unchanged historical snapshots stay attached; changing this parts list replaces the event's recorded use.";
    partFieldset.append(partLegend, partHelp);
    const selectedPartIds = new Set(
      attempt?.parts?.flatMap((part) => (part.id ? [part.id] : [])) ?? [],
    );
    const partQuantityInputs = new Map<string, HTMLInputElement>();
    const partUseInputs = new Map<string, HTMLInputElement>();
    let partSelectionChanged = false;
    const partSummary = document.createElement("div");
    partSummary.className = "library-part-usage-summary";
    partSummary.setAttribute("role", "status");
    partSummary.setAttribute("aria-live", "polite");
    for (const part of this.partStock) {
      const row = document.createElement("div");
      row.className = "library-part-usage-row";
      const label = document.createElement("label");
      const use = document.createElement("input");
      use.type = "checkbox";
      use.checked = selectedPartIds.has(part.id);
      use.setAttribute("aria-label", `Use ${part.name} for this print`);
      label.append(
        use,
        document.createTextNode(`${part.name} · ${part.quantity} ${part.unit} available`),
      );
      const perCopy = this.input(`Quantity of ${part.name} per copy`, "number", "1");
      perCopy.min = "1";
      perCopy.max = "9999";
      perCopy.disabled = !use.checked;
      const historical = attempt?.parts?.find((item) => item.id === part.id);
      if (historical) perCopy.value = String(historical.quantityPerCopy);
      partQuantityInputs.set(part.id, perCopy);
      partUseInputs.set(part.id, use);
      use.addEventListener("change", () => {
        partSelectionChanged = true;
        perCopy.disabled = !use.checked;
        updatePartSummary();
      });
      perCopy.addEventListener("input", () => {
        partSelectionChanged = true;
        updatePartSummary();
      });
      row.append(label, perCopy);
      partFieldset.append(row);
    }
    const availablePartIds = new Set(this.partStock.map((part) => part.id));
    for (const part of attempt?.parts ?? []) {
      if (part.id && availablePartIds.has(part.id)) continue;
      const snapshot = document.createElement("p");
      snapshot.textContent = `Historical use: ${part.name} · ${part.quantity} ${part.unit} total. This stock record is no longer available; it remains in the event history.`;
      partFieldset.append(snapshot);
    }
    const updatePartSummary = () => {
      const selected = this.partStock.flatMap((part) => {
        const checkbox = partUseInputs.get(part.id);
        const quantityInput = partQuantityInputs.get(part.id);
        if (!checkbox?.checked || !quantityInput) return [];
        return [{ id: part.id, quantity: Number(quantityInput.value) }];
      });
      const estimate = calculatePartUsage(
        selected,
        Number(quantity.value),
        this.partStock,
        attempt?.parts,
      );
      partSummary.replaceChildren();
      for (const item of estimate) {
        const line = document.createElement("p");
        line.textContent = item.sufficient
          ? `${item.name}: ${item.quantityPerCopy} per copy, ${item.quantity} ${item.unit} total; ${item.available} available after replacing this event.`
          : `${item.name}: ${item.quantity} ${item.unit} needed, ${item.available} available. Add stock or reduce the per-copy amount.`;
        partSummary.append(line);
      }
      if (estimate.length === 0 && !attempt?.parts?.length) {
        const empty = document.createElement("p");
        empty.textContent = "No parts selected.";
        partSummary.append(empty);
      }
      save.disabled = estimate.some((item) => !item.sufficient);
    };
    quantity.addEventListener("input", updatePartSummary);
    partFieldset.append(partSummary);
    const save = this.button(attempt ? "Save attempt" : "Log attempt", () => void submit());
    const submit = async () => {
      const input: PrintAttemptInput = {
        printed_at: date.value,
        quantity: Number(quantity.value),
        filament_ids: [...filaments.selectedOptions].map((option) => option.value),
        notes: notes.value,
        outcome: outcome.value as PrintAttemptInput["outcome"],
      };
      if (!attempt || partSelectionChanged) {
        input.parts = this.partStock.flatMap((part) => {
          const checkbox = partUseInputs.get(part.id);
          const quantityInput = partQuantityInputs.get(part.id);
          return checkbox?.checked && quantityInput
            ? [{ id: part.id, quantity: Number(quantityInput.value) }]
            : [];
        });
      }
      const problem = validatePrintAttempt(input);
      if (problem) {
        error.textContent = problem;
        return;
      }
      error.textContent = "";
      save.disabled = true;
      try {
        const nativeInput = { ...input, printed_at: `${input.printed_at}T00:00:00.000Z` };
        const printerId = printerIdChangeForAttempt(printer.value, attempt);
        if (attempt) await this.onUpdate(attempt.id, nativeInput, printerId);
        else if (this.model) await this.onCreate(this.model.id, nativeInput, printerId ?? null);
      } catch (reason) {
        error.textContent = describePartStockError(reason);
      } finally {
        save.disabled = false;
      }
    };
    form.append(date, quantity, printer, filaments, outcome, notes, partFieldset, save);
    updatePartSummary();
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
