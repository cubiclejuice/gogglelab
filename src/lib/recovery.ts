import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  libraryCreatePrintAttempt,
  libraryCreatePartStock,
  libraryImportPrintAttempt,
  libraryImportPrintAttempts,
  libraryCreateTag,
  libraryListFilaments,
  libraryListModels,
  libraryListPartStock,
  libraryListPrinters,
  libraryListPrintAttempts,
  libraryListTags,
  libraryImportPrinterData,
  librarySetModelFilaments,
  libraryUpdateModelMetadata,
  libraryUpdatePartStock,
  importSlicerProfiles,
  recoveryExportColorways,
  recoveryExportLibrary,
  recoveryExportPrintventoryJson,
  recoveryListLibrary,
  type Filament,
  type LibraryModel,
  type LibraryPrinter,
  type LibraryTag,
  type PartStock,
  type PrinterDataImport,
  type PrinterDataImportResult,
} from "./ipc";
import {
  applyPrintventoryAssignments,
  applyPrintventoryPartAssignments,
  buildPrintventoryFieldMapping,
  buildPrintventoryImportPlan,
  canApplyPrintventoryImport,
  createPrintventoryTargetSearch,
  missingPrintventoryTagNames,
  printventoryTargetLabel,
  readPrintventoryExportFile,
  validatePrintventoryPartAssignments,
  validatePrintventoryAssignments,
  type PrintventoryAssignment,
  type PrintventoryExport,
  type PrintventoryImportRowResult,
  type PrintventoryImportPlanRow,
  type PrintventoryPartAssignment,
  type PrintventoryPartImportResult,
  type PrintventoryTargetSearch,
} from "./printventory-import";
import { COLORWAY_STORAGE_KEY } from "./storage-keys";
import { SlicerImportReview } from "./slicer-import";

export const PRINTER_IMPORT_REVIEW_PAGE_SIZE = 50;
export const MAX_LOCAL_PRINTER_MAPPING_OPTIONS = 20;
export const MAX_LOCAL_PART_MAPPING_OPTIONS = 20;

export function paginatePrinterImportProfiles<T>(
  profiles: readonly T[],
  requestedPage: number,
): { items: T[]; page: number; pageCount: number; start: number; end: number } {
  const pageCount = Math.ceil(profiles.length / PRINTER_IMPORT_REVIEW_PAGE_SIZE);
  const candidatePage = Number.isFinite(requestedPage) ? Math.trunc(requestedPage) : 0;
  const page = pageCount === 0 ? 0 : Math.min(Math.max(0, candidatePage), pageCount - 1);
  const start = page * PRINTER_IMPORT_REVIEW_PAGE_SIZE;
  const end = Math.min(profiles.length, start + PRINTER_IMPORT_REVIEW_PAGE_SIZE);
  return {
    items: profiles.slice(start, end),
    page,
    pageCount,
    start,
    end,
  };
}

export function createPrinterMappingOptions(
  printers: readonly LibraryPrinter[],
  query: string,
  selectedPrinterId: string | null = null,
): { options: LibraryPrinter[]; matchCount: number } {
  const normalizedQuery = query.trim().toLowerCase();
  const matches = printers.filter((printer) => {
    if (!normalizedQuery) return true;
    return [printer.nickname, printer.make ?? "", printer.model ?? ""]
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  });
  const options = matches.slice(0, MAX_LOCAL_PRINTER_MAPPING_OPTIONS);
  const selected = selectedPrinterId
    ? printers.find((printer) => printer.id === selectedPrinterId)
    : undefined;
  if (selected && !options.some((printer) => printer.id === selected.id)) {
    options.unshift(selected);
    if (options.length > MAX_LOCAL_PRINTER_MAPPING_OPTIONS) options.pop();
  }
  return { options, matchCount: matches.length };
}

export function createPartMappingOptions(
  parts: readonly PartStock[],
  query: string,
  selectedPartId: string | null = null,
): { options: PartStock[]; matchCount: number } {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matches = parts.filter((part) => {
    if (!normalizedQuery) return true;
    return [part.name, part.category ?? "", part.unit]
      .join(" ")
      .toLocaleLowerCase()
      .includes(normalizedQuery);
  });
  const options = matches.slice(0, MAX_LOCAL_PART_MAPPING_OPTIONS);
  const selected = selectedPartId ? parts.find((part) => part.id === selectedPartId) : undefined;
  if (selected && !options.some((part) => part.id === selected.id)) {
    options.unshift(selected);
    if (options.length > MAX_LOCAL_PART_MAPPING_OPTIONS) options.pop();
  }
  return { options, matchCount: matches.length };
}

export interface RawStringStorage {
  getItem(key: string): string | null;
}

export function readRawColorwayValue(
  storage: RawStringStorage | null = typeof globalThis === "undefined"
    ? null
    : ((globalThis as { localStorage?: RawStringStorage }).localStorage ?? null),
): string | null {
  if (!storage) return null;
  try {
    return storage.getItem(COLORWAY_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function buildPrinterDataImportPayload(
  documentData: Pick<PrintventoryExport, "printers" | "maintenanceLogs" | "reminders">,
  choices: ReadonlyMap<string, string>,
  now = new Date().toISOString(),
): {
  input: PrinterDataImport;
  pendingExternalRefs: string[];
  unlinkedMaintenanceLogs: number;
  unlinkedReminders: number;
} {
  const input: PrinterDataImport = { printers: [], maintenanceLogs: [], reminders: [] };
  const pendingExternalRefs: string[] = [];
  const importedRefs = new Set<string>();
  const includedRefs = new Set<string>();

  for (const printer of documentData.printers ?? []) {
    if (importedRefs.has(printer.externalRef)) {
      throw new Error("The Printventory file contains duplicate printer references.");
    }
    importedRefs.add(printer.externalRef);
    const choice = choices.get(printer.externalRef);
    if (choice === "skip") {
      continue;
    }
    if (choice === "create") {
      input.printers.push({
        externalRef: printer.externalRef,
        input: printer.input,
        ...(printer.createdAt ? { createdAt: printer.createdAt } : {}),
        ...(printer.updatedAt ? { updatedAt: printer.updatedAt } : {}),
      });
      includedRefs.add(printer.externalRef);
      continue;
    }
    if (choice?.startsWith("local:")) {
      const localPrinterId = choice.slice("local:".length);
      if (localPrinterId) {
        input.printers.push({
          externalRef: printer.externalRef,
          mapToLocalPrinterId: localPrinterId,
          input: printer.input,
        });
        includedRefs.add(printer.externalRef);
        continue;
      }
    }
    pendingExternalRefs.push(printer.externalRef);
  }

  const pendingRefs = new Set(pendingExternalRefs);
  const isUnlinked = (printerExternalRef: string | null): boolean =>
    printerExternalRef === null ||
    (!includedRefs.has(printerExternalRef) && !pendingRefs.has(printerExternalRef));

  let unlinkedMaintenanceLogs = 0;
  for (const log of documentData.maintenanceLogs ?? []) {
    if (isUnlinked(log.printerExternalRef)) unlinkedMaintenanceLogs += 1;
    input.maintenanceLogs.push({
      externalRef: log.externalRef,
      printerExternalRef: log.printerExternalRef,
      printerNickname: log.printerNickname,
      input: log.input,
      ...(log.createdAt ? { createdAt: log.createdAt } : {}),
    });
  }

  let unlinkedReminders = 0;
  for (const reminder of documentData.reminders ?? []) {
    if (isUnlinked(reminder.printerExternalRef)) unlinkedReminders += 1;
    input.reminders.push({
      externalRef: reminder.externalRef,
      printerExternalRef: reminder.printerExternalRef,
      printerNickname: reminder.printerNickname,
      input: reminder.input,
      status: reminder.status,
      ...(reminder.lastCompletedAt ? { lastCompletedAt: reminder.lastCompletedAt } : {}),
      createdAt: reminder.createdAt ?? now,
      updatedAt: reminder.updatedAt ?? now,
    });
  }

  return { input, pendingExternalRefs, unlinkedMaintenanceLogs, unlinkedReminders };
}

export function describeUnlinkedPrinterHistory(
  maintenanceLogCount: number,
  reminderCount: number,
): string {
  if (maintenanceLogCount <= 0 && reminderCount <= 0) return "";
  const records: string[] = [];
  if (maintenanceLogCount > 0) {
    records.push(
      `${maintenanceLogCount.toLocaleString()} maintenance log${maintenanceLogCount === 1 ? "" : "s"}`,
    );
  }
  if (reminderCount > 0) {
    records.push(`${reminderCount.toLocaleString()} reminder${reminderCount === 1 ? "" : "s"}`);
  }
  return `${records.join(" and ")} will be retained without a linked printer.`;
}

export function describeSlicerImportReview(documentData: PrintventoryExport): string {
  return documentData.slicers?.length
    ? ` ${documentData.slicers.length} slicer profile(s) require local app selection and trust, or an explicit skip.`
    : "";
}

export function mountRecoveryControl(
  container: HTMLElement,
  addFooterControl: (element: HTMLElement) => void,
  onImportCompleted?: () => void | Promise<void>,
): void {
  const launcher = document.createElement("button");
  launcher.type = "button";
  launcher.className = "btn";
  launcher.textContent = "Recover data…";
  launcher.setAttribute("aria-label", "Recover previously saved Library data or color assignments");

  const dialog = document.createElement("dialog");
  dialog.className = "sheet recovery-panel";
  dialog.setAttribute("aria-labelledby", "recovery-title");
  const title = document.createElement("h2");
  title.id = "recovery-title";
  title.textContent = "Recover GoggleLab data";
  const description = document.createElement("p");
  description.textContent =
    "Recover a managed Library backup, import reviewed Printventory metadata and printer history, or export compatible JSON and saved color assignments.";
  const status = document.createElement("p");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  const error = document.createElement("p");
  error.className = "settings-error";
  error.setAttribute("role", "alert");
  const actions = document.createElement("div");
  actions.className = "settings-actions";
  const exportLibrary = document.createElement("button");
  exportLibrary.type = "button";
  exportLibrary.className = "btn primary";
  exportLibrary.textContent = "Export Library backup…";
  const exportPrintventoryJson = document.createElement("button");
  exportPrintventoryJson.type = "button";
  exportPrintventoryJson.className = "btn";
  exportPrintventoryJson.textContent = "Export Printventory JSON…";
  const exportColorways = document.createElement("button");
  exportColorways.type = "button";
  exportColorways.className = "btn";
  exportColorways.textContent = "Export saved color assignments…";
  const importPrintventory = document.createElement("button");
  importPrintventory.type = "button";
  importPrintventory.className = "btn";
  importPrintventory.textContent = "Import Printventory JSON…";
  const filePicker = document.createElement("input");
  filePicker.type = "file";
  filePicker.accept = ".json,application/json";
  filePicker.hidden = true;
  const importPreview = document.createElement("section");
  importPreview.className = "recovery-import-preview";
  importPreview.hidden = true;
  const importPreviewTitle = document.createElement("h3");
  importPreviewTitle.textContent = "Printventory import preview";
  const importPreviewHint = document.createElement("p");
  importPreviewHint.textContent =
    "Choose a GoggleLab indexed model for each row. Paths are match hints only. Printer IDs are mapping hints; choose an existing local printer or create a separate imported profile. Imported endpoints remain unapproved and credentials are ignored. Model files are never opened or copied.";
  const printerImportSection = document.createElement("section");
  printerImportSection.className = "recovery-printer-import";
  const printerImportTitle = document.createElement("h4");
  printerImportTitle.textContent = "Printer data";
  const printerImportSummary = document.createElement("p");
  printerImportSummary.setAttribute("role", "status");
  const printerImportPagination = document.createElement("nav");
  printerImportPagination.className = "recovery-import-pagination";
  printerImportPagination.setAttribute("aria-label", "Printer profile import pages");
  const printerImportRows = document.createElement("div");
  printerImportSection.append(
    printerImportTitle,
    printerImportSummary,
    printerImportPagination,
    printerImportRows,
  );
  const partImportSection = document.createElement("section");
  partImportSection.className = "recovery-part-import";
  const partImportTitle = document.createElement("h4");
  partImportTitle.textContent = "Parts stock and print-event usage";
  const partImportSummary = document.createElement("p");
  partImportSummary.setAttribute("role", "status");
  const partImportPagination = document.createElement("nav");
  partImportPagination.className = "recovery-import-pagination";
  partImportPagination.setAttribute("aria-label", "Part stock import pages");
  const partImportRows = document.createElement("div");
  partImportSection.append(
    partImportTitle,
    partImportSummary,
    partImportPagination,
    partImportRows,
  );
  const importPagination = document.createElement("div");
  importPagination.className = "recovery-import-pagination";
  const importPreviewRows = document.createElement("div");
  const importResult = document.createElement("div");
  importResult.setAttribute("aria-live", "polite");
  const slicerReview = new SlicerImportReview(() => updateAssignmentState());
  const applyImport = document.createElement("button");
  applyImport.type = "button";
  applyImport.className = "btn primary";
  applyImport.textContent = "Apply reviewed import";
  applyImport.disabled = true;
  importPreview.append(
    importPreviewTitle,
    importPreviewHint,
    printerImportSection,
    partImportSection,
    slicerReview.element,
    importPagination,
    importPreviewRows,
    importResult,
    applyImport,
  );
  exportLibrary.disabled = true;
  exportColorways.disabled = true;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "btn";
  close.textContent = "Close";
  actions.append(exportLibrary, exportPrintventoryJson, exportColorways, importPrintventory, close);
  dialog.style.width = "min(900px, calc(100% - 32px))";
  dialog.style.maxHeight = "calc(100vh - 32px)";
  dialog.style.overflow = "auto";
  dialog.append(title, description, status, error, importPreview, filePicker, actions);
  container.appendChild(dialog);

  let importDocument: PrintventoryExport | null = null;
  let importPlan: PrintventoryImportPlanRow[] = [];
  let previewModels: LibraryModel[] = [];
  let previewPrinters: LibraryPrinter[] = [];
  let targetSearch: PrintventoryTargetSearch = createPrintventoryTargetSearch([]);
  let previewTags: LibraryTag[] = [];
  let previewFilaments: Filament[] = [];
  let previewPartStock: PartStock[] = [];
  let selectedTargetByEntry = new Map<number, string | null>();
  let reviewedTargetByEntry = new Map<number, string>();
  let selectedPrinterTargetByExternalRef = new Map<string, string>();
  let selectedPartTargetByExternalRef = new Map<string, string>();
  let printerImportResult: PrinterDataImportResult | null = null;
  let partImportResult: PrintventoryPartImportResult | null = null;
  let completedResultByEntry = new Map<number, PrintventoryImportRowResult>();
  let completedResultSummary = "";
  let currentImportPage = 0;
  let currentPrinterImportPage = 0;
  let currentPartImportPage = 0;
  const importReviewPageSize = PRINTER_IMPORT_REVIEW_PAGE_SIZE;
  let importCompleted = false;
  let importApplying = false;

  const updateStatus = async () => {
    error.textContent = "";
    const colorways = readRawColorwayValue();
    exportColorways.disabled = colorways === null;
    const parts: string[] = [];
    try {
      const manifest = await recoveryListLibrary();
      exportLibrary.disabled = false;
      exportPrintventoryJson.disabled = false;
      parts.push(`Library found: ${manifest.models.length} cataloged model(s).`);
    } catch {
      exportLibrary.disabled = true;
      exportPrintventoryJson.disabled = true;
      parts.push("No supported Library database was found on this device.");
    }
    parts.push(
      colorways === null
        ? "No saved color assignments were found in this app profile."
        : `Saved color assignment data is available (${colorways.length.toLocaleString()} characters).`,
    );
    status.textContent = parts.join(" ");
  };

  launcher.addEventListener("click", () => {
    if (!dialog.open) dialog.showModal();
    void updateStatus();
  });
  importPrintventory.addEventListener("click", () => filePicker.click());
  dialog.addEventListener("cancel", (event) => {
    if (importApplying) event.preventDefault();
  });
  filePicker.addEventListener("change", () => {
    const file = filePicker.files?.[0];
    filePicker.value = "";
    if (!file) return;
    void loadPrintventoryPreview(file);
  });

  async function loadPrintventoryPreview(file: File): Promise<void> {
    error.textContent = "";
    status.textContent = "Reading the selected Printventory JSON and loading the local Library…";
    applyImport.disabled = true;
    importPrintventory.disabled = true;
    importPagination.replaceChildren();
    printerImportPagination.replaceChildren();
    partImportPagination.replaceChildren();
    importPreviewRows.replaceChildren();
    printerImportRows.replaceChildren();
    importResult.replaceChildren();
    completedResultByEntry = new Map();
    completedResultSummary = "";
    try {
      const documentData = await readPrintventoryExportFile(file);
      const [allModels, tags, filaments, printers, partStock] = await Promise.all([
        libraryListModels(),
        libraryListTags(),
        libraryListFilaments(true).then((items) => items.filter((filament) => !filament.archived)),
        libraryListPrinters(),
        libraryListPartStock(),
      ]);
      const models = allModels;
      targetSearch = createPrintventoryTargetSearch(models);
      importDocument = documentData;
      slicerReview.reset(documentData.slicers ?? []);
      importPlan = buildPrintventoryImportPlan(documentData, models);
      selectedTargetByEntry = new Map(
        importPlan.map((row) => [row.entryIndex, row.suggestedTargetId]),
      );
      reviewedTargetByEntry = new Map();
      selectedPrinterTargetByExternalRef = new Map();
      selectedPartTargetByExternalRef = new Map();
      printerImportResult = null;
      partImportResult = null;
      completedResultByEntry = new Map();
      completedResultSummary = "";
      previewModels = models;
      previewPrinters = printers;
      previewTags = tags;
      previewFilaments = filaments;
      previewPartStock = partStock;
      importCompleted = false;
      currentImportPage = 0;
      currentPrinterImportPage = 0;
      currentPartImportPage = 0;
      renderPrintventoryPreview();
      importPreview.hidden = false;
      const slicerNotice = describeSlicerImportReview(documentData);
      const printerDataNotice =
        (documentData.printers?.length ?? 0) +
          (documentData.maintenanceLogs?.length ?? 0) +
          (documentData.reminders?.length ?? 0) >
        0
          ? ` ${documentData.printers?.length ?? 0} printer profile(s), ${documentData.maintenanceLogs?.length ?? 0} maintenance log(s), and ${documentData.reminders?.length ?? 0} reminder(s) are available for review.`
          : "";
      const partDataNotice = documentData.parts?.length
        ? ` ${documentData.parts.length} stock part(s) are available for reviewed create, update, or skip choices; event use is historical and will not be deducted again.`
        : "";
      status.textContent = `Preview ready: ${documentData.models.length} model metadata entr${documentData.models.length === 1 ? "y" : "ies"}.${printerDataNotice}${partDataNotice}${slicerNotice}`;
    } catch (reason) {
      importDocument = null;
      importPlan = [];
      selectedTargetByEntry = new Map();
      reviewedTargetByEntry = new Map();
      selectedPrinterTargetByExternalRef = new Map();
      selectedPartTargetByExternalRef = new Map();
      printerImportResult = null;
      partImportResult = null;
      completedResultByEntry = new Map();
      completedResultSummary = "";
      importPreview.hidden = true;
      error.textContent = `Could not preview Printventory JSON: ${String(reason)}`;
      status.textContent = "Choose a valid Printventory JSON export to preview metadata.";
    } finally {
      importPrintventory.disabled = false;
    }
  }

  function renderPrintventoryPreview(): void {
    importPagination.replaceChildren();
    importPreviewRows.replaceChildren();
    renderPrinterImportReview();
    renderPartStockImportReview();
    const pageCount = Math.max(1, Math.ceil(importPlan.length / importReviewPageSize));
    currentImportPage = Math.min(currentImportPage, pageCount - 1);
    if (pageCount > 1) {
      const previous = document.createElement("button");
      previous.type = "button";
      previous.className = "btn";
      previous.textContent = "Previous rows";
      previous.disabled = importApplying || currentImportPage === 0;
      previous.addEventListener("click", () => {
        currentImportPage -= 1;
        renderPrintventoryPreview();
      });
      const pageLabel = document.createElement("span");
      const first = currentImportPage * importReviewPageSize + 1;
      const last = Math.min(importPlan.length, first + importReviewPageSize - 1);
      pageLabel.textContent = `Rows ${first}–${last} of ${importPlan.length} · page ${currentImportPage + 1} of ${pageCount}`;
      const next = document.createElement("button");
      next.type = "button";
      next.className = "btn";
      next.textContent = "Next rows";
      next.disabled = importApplying || currentImportPage + 1 >= pageCount;
      next.addEventListener("click", () => {
        currentImportPage += 1;
        renderPrintventoryPreview();
      });
      importPagination.append(previous, pageLabel, next);
    }
    const pageStart = currentImportPage * importReviewPageSize;
    const pageRows = importPlan.slice(pageStart, pageStart + importReviewPageSize);
    for (const row of pageRows) {
      const entry = row.entry;
      const fieldset = document.createElement("fieldset");
      fieldset.className = "recovery-import-row";
      fieldset.disabled = importApplying;
      const legend = document.createElement("legend");
      const fileName =
        typeof entry.fileName === "string" ? entry.fileName : `Entry ${row.entryIndex + 1}`;
      legend.textContent = `${fileName} — ${row.suggestionMessage}`;
      const preview = document.createElement("p");
      preview.textContent = describeImportedFields(entry);
      const searchLabel = document.createElement("label");
      searchLabel.textContent = "Search Library models";
      const search = document.createElement("input");
      search.type = "search";
      search.placeholder = "Search name or filename";
      search.setAttribute("aria-label", `Search Library models for ${fileName}`);
      searchLabel.append(search);
      const targetLabel = document.createElement("label");
      targetLabel.textContent = "Library target";
      const select = document.createElement("select");
      select.setAttribute("aria-label", `GoggleLab target for ${fileName}`);
      targetLabel.append(select);
      const reviewInput = document.createElement("input");
      reviewInput.type = "checkbox";
      reviewInput.setAttribute("aria-label", `Confirm reviewed Printventory row for ${fileName}`);
      const reviewLabel = document.createElement("label");
      reviewLabel.append(
        reviewInput,
        document.createTextNode(" I reviewed this target and its supported fields."),
      );
      const refreshTargetOptions = () => {
        const selectedId = selectedTargetByEntry.get(row.entryIndex) ?? null;
        const candidates = targetSearch(search.value, selectedId);
        select.replaceChildren();
        const skip = document.createElement("option");
        skip.value = "";
        skip.textContent = "Skip this entry";
        select.append(skip);
        for (const model of candidates) {
          const option = document.createElement("option");
          option.value = model.id;
          option.textContent = printventoryTargetLabel(model);
          select.append(option);
        }
        select.value = selectedId ?? "";
        reviewInput.disabled = !selectedId;
        reviewInput.checked =
          selectedId !== null && reviewedTargetByEntry.get(row.entryIndex) === selectedId;
      };
      refreshTargetOptions();
      const mappingSummary = document.createElement("p");
      const mappingWarnings = document.createElement("div");
      const refreshFieldMapping = () => {
        const previewPartIds = new Map<string, string>();
        for (const importedPart of importDocument?.parts ?? []) {
          const choice = selectedPartTargetByExternalRef.get(importedPart.externalRef);
          if (choice === "create") {
            previewPartIds.set(importedPart.externalRef, `preview:${importedPart.externalRef}`);
          } else if (choice?.startsWith("local:")) {
            previewPartIds.set(importedPart.externalRef, choice.slice("local:".length));
          }
        }
        const mapping = buildPrintventoryFieldMapping(
          entry,
          select.value,
          previewModels,
          previewTags,
          previewFilaments,
          importDocument?.parts ?? [],
          previewPartIds,
        );
        mappingSummary.textContent = mapping.mappedFields.length
          ? `Supported fields: ${mapping.mappedFields.join(", ")}.`
          : "No supported fields are ready to apply for this target.";
        mappingWarnings.replaceChildren();
        const newTags = missingPrintventoryTagNames(mapping.tagNames ?? [], previewTags);
        if (newTags.length > 0) {
          const tagNotice = document.createElement("p");
          const tagAssignments = mapping.tagAssignments ?? [];
          const describedTags = newTags.map((name) => {
            const assignment = tagAssignments.find(
              (item) => item.name.trim().toLocaleLowerCase() === name.trim().toLocaleLowerCase(),
            );
            return assignment?.category ? name + " (" + assignment.category + ")" : name;
          });
          tagNotice.textContent = "New local tags to create: " + describedTags.join(", ") + ".";
          mappingWarnings.append(tagNotice);
        }
        for (const warning of mapping.warnings) {
          const warningLine = document.createElement("p");
          warningLine.textContent = warning;
          mappingWarnings.append(warningLine);
        }
      };
      select.addEventListener("change", () => {
        selectedTargetByEntry.set(row.entryIndex, select.value || null);
        reviewedTargetByEntry.delete(row.entryIndex);
        refreshTargetOptions();
        importCompleted = false;
        completedResultByEntry = new Map();
        completedResultSummary = "";
        refreshFieldMapping();
        updateAssignmentState();
      });
      reviewInput.addEventListener("change", () => {
        const selectedId = selectedTargetByEntry.get(row.entryIndex) ?? null;
        if (reviewInput.checked && selectedId) {
          reviewedTargetByEntry.set(row.entryIndex, selectedId);
        } else {
          reviewedTargetByEntry.delete(row.entryIndex);
        }
        updateAssignmentState();
      });
      search.addEventListener("input", refreshTargetOptions);
      refreshFieldMapping();
      fieldset.append(
        legend,
        preview,
        searchLabel,
        targetLabel,
        reviewLabel,
        mappingSummary,
        mappingWarnings,
      );
      importPreviewRows.append(fieldset);
    }
    if (importCompleted) renderCompletedImportPage();
    updateAssignmentState();
  }

  function renderPrinterImportReview(): void {
    const printers = importDocument?.printers ?? [];
    const logs = importDocument?.maintenanceLogs ?? [];
    const reminders = importDocument?.reminders ?? [];
    const hasPrinterData = printers.length + logs.length + reminders.length > 0;
    printerImportSection.hidden = !hasPrinterData;
    printerImportPagination.replaceChildren();
    printerImportRows.replaceChildren();
    if (!hasPrinterData) {
      printerImportSummary.textContent = "No printer profiles or printer history are in this file.";
      return;
    }

    const page = paginatePrinterImportProfiles(printers, currentPrinterImportPage);
    currentPrinterImportPage = page.page;
    if (page.pageCount > 1) {
      const previous = document.createElement("button");
      previous.type = "button";
      previous.className = "btn";
      previous.textContent = "Previous printer profiles";
      previous.disabled = page.page === 0 || importApplying || printerImportResult !== null;
      previous.addEventListener("click", () => {
        currentPrinterImportPage = page.page - 1;
        renderPrinterImportReview();
      });
      const pageStatus = document.createElement("span");
      pageStatus.setAttribute("role", "status");
      pageStatus.textContent = `Profiles ${page.start + 1}–${page.end} of ${printers.length.toLocaleString()}`;
      const next = document.createElement("button");
      next.type = "button";
      next.className = "btn";
      next.textContent = "Next printer profiles";
      next.disabled =
        page.page + 1 >= page.pageCount || importApplying || printerImportResult !== null;
      next.addEventListener("click", () => {
        currentPrinterImportPage = page.page + 1;
        renderPrinterImportReview();
      });
      printerImportPagination.append(previous, pageStatus, next);
    }

    for (const printer of page.items) {
      const fieldset = document.createElement("fieldset");
      fieldset.className = "recovery-import-row recovery-printer-import-row";
      fieldset.disabled = importApplying || printerImportResult !== null;
      const legend = document.createElement("legend");
      const makeModel = [printer.input.make, printer.input.model].filter(Boolean).join(" ");
      legend.textContent = makeModel
        ? `${printer.input.nickname} — ${makeModel}`
        : printer.input.nickname;
      const searchLabel = document.createElement("label");
      searchLabel.textContent = "Search local printers";
      const search = document.createElement("input");
      search.type = "search";
      search.setAttribute("aria-label", `Search local printers for ${printer.input.nickname}`);
      searchLabel.append(search);
      const mappingLabel = document.createElement("label");
      mappingLabel.textContent = "Local printer mapping";
      const select = document.createElement("select");
      select.setAttribute("aria-label", `Local printer mapping for ${printer.input.nickname}`);
      const chooseOption = document.createElement("option");
      chooseOption.value = "";
      chooseOption.textContent = "Choose a mapping";
      select.append(chooseOption);
      const createOption = document.createElement("option");
      createOption.value = "create";
      createOption.textContent = "Create a separate imported profile";
      select.append(createOption);
      const skipOption = document.createElement("option");
      skipOption.value = "skip";
      skipOption.textContent = "Skip this printer profile";
      const mappingHint = document.createElement("p");
      mappingHint.className = "hint";
      const refreshMappingOptions = () => {
        const choice = selectedPrinterTargetByExternalRef.get(printer.externalRef) ?? "";
        const selectedLocalId = choice.startsWith("local:") ? choice.slice("local:".length) : null;
        const { options, matchCount } = createPrinterMappingOptions(
          previewPrinters,
          search.value,
          selectedLocalId,
        );
        select.replaceChildren(chooseOption, createOption, skipOption);
        for (const localPrinter of options) {
          const option = document.createElement("option");
          option.value = `local:${localPrinter.id}`;
          const localMakeModel = [localPrinter.make, localPrinter.model].filter(Boolean).join(" ");
          option.textContent = localMakeModel
            ? `${localPrinter.nickname} — ${localMakeModel}`
            : localPrinter.nickname;
          select.append(option);
        }
        select.value = choice;
        if (previewPrinters.length === 0) {
          mappingHint.textContent = "No local printer profiles are available to map.";
        } else if (matchCount === 0) {
          mappingHint.textContent = "No local printers match this search.";
        } else if (matchCount > MAX_LOCAL_PRINTER_MAPPING_OPTIONS) {
          mappingHint.textContent = `Showing ${MAX_LOCAL_PRINTER_MAPPING_OPTIONS} of ${matchCount.toLocaleString()} matches. Refine the search.`;
        } else if (!search.value.trim() && previewPrinters.length > options.length) {
          mappingHint.textContent = `Showing ${options.length} of ${previewPrinters.length.toLocaleString()} local printers. Search to find a different printer.`;
        } else {
          mappingHint.textContent = `${matchCount.toLocaleString()} local printer(s) match.`;
        }
      };
      refreshMappingOptions();
      search.addEventListener("input", refreshMappingOptions);
      mappingLabel.append(select);
      const details = document.createElement("p");
      const updateDetails = () => {
        const choice = selectedPrinterTargetByExternalRef.get(printer.externalRef);
        if (choice === "skip") {
          details.textContent =
            "This profile will be skipped. Its maintenance history will be retained without a linked printer.";
        } else if (choice?.startsWith("local:")) {
          details.textContent =
            "The selected local profile will not be changed. Imported endpoints are ignored; printer history is linked to this local profile after review.";
        } else if (printer.endpointReviewRequired) {
          details.textContent =
            "The new local profile will retain its endpoint as unapproved metadata. GoggleLab will not connect until you review it in Printer Fleet; credentials are ignored.";
        } else {
          details.textContent =
            "The new local profile has no endpoint. Its external ID is only a mapping hint and will not become a local ID.";
        }
      };
      updateDetails();
      select.addEventListener("change", () => {
        if (select.value) selectedPrinterTargetByExternalRef.set(printer.externalRef, select.value);
        else selectedPrinterTargetByExternalRef.delete(printer.externalRef);
        updateDetails();
        importCompleted = false;
        completedResultByEntry = new Map();
        completedResultSummary = "";
        updateAssignmentState();
      });
      fieldset.append(legend, searchLabel, mappingLabel, mappingHint, details);
      printerImportRows.append(fieldset);
    }
    updateAssignmentState();
  }

  function renderPartStockImportReview(): void {
    const parts = importDocument?.parts ?? [];
    const hasPartData = parts.length > 0;
    partImportSection.hidden = !hasPartData;
    partImportPagination.replaceChildren();
    partImportRows.replaceChildren();
    if (!hasPartData) {
      partImportSummary.textContent = "No top-level stock parts are in this file.";
      return;
    }
    partImportSummary.textContent = `${parts.length.toLocaleString()} stock row(s). Imported quantities are current-stock snapshots. Historical event usage is linked after review without deducting it again.`;

    const pageCount = Math.ceil(parts.length / importReviewPageSize);
    currentPartImportPage = Math.min(currentPartImportPage, pageCount - 1);
    if (pageCount > 1) {
      const previous = document.createElement("button");
      previous.type = "button";
      previous.className = "btn";
      previous.textContent = "Previous parts";
      previous.disabled =
        currentPartImportPage === 0 || importApplying || partImportResult !== null;
      previous.addEventListener("click", () => {
        currentPartImportPage -= 1;
        renderPartStockImportReview();
      });
      const pageStatus = document.createElement("span");
      pageStatus.setAttribute("role", "status");
      const first = currentPartImportPage * importReviewPageSize + 1;
      const last = Math.min(parts.length, first + importReviewPageSize - 1);
      pageStatus.textContent = `Parts ${first}–${last} of ${parts.length.toLocaleString()}`;
      const next = document.createElement("button");
      next.type = "button";
      next.className = "btn";
      next.textContent = "Next parts";
      next.disabled =
        currentPartImportPage + 1 >= pageCount || importApplying || partImportResult !== null;
      next.addEventListener("click", () => {
        currentPartImportPage += 1;
        renderPartStockImportReview();
      });
      partImportPagination.append(previous, pageStatus, next);
    }

    const pageStart = currentPartImportPage * importReviewPageSize;
    for (const part of parts.slice(pageStart, pageStart + importReviewPageSize)) {
      const fieldset = document.createElement("fieldset");
      fieldset.className = "recovery-import-row recovery-part-import-row";
      fieldset.disabled = importApplying || partImportResult !== null;
      const legend = document.createElement("legend");
      legend.textContent = `${part.input.name} — ${part.input.quantity.toLocaleString()} ${part.input.unit}`;
      const searchLabel = document.createElement("label");
      searchLabel.textContent = "Search local stock parts";
      const search = document.createElement("input");
      search.type = "search";
      search.setAttribute("aria-label", `Search local stock for ${part.input.name}`);
      searchLabel.append(search);
      const mappingLabel = document.createElement("label");
      mappingLabel.textContent = "Local stock choice";
      const select = document.createElement("select");
      select.setAttribute("aria-label", `Stock mapping for ${part.input.name}`);
      mappingLabel.append(select);
      const mappingHint = document.createElement("p");
      mappingHint.className = "hint";
      const details = document.createElement("p");
      const refreshOptions = () => {
        const choice = selectedPartTargetByExternalRef.get(part.externalRef) ?? "";
        const selectedId = choice.startsWith("local:") ? choice.slice("local:".length) : null;
        const { options, matchCount } = createPartMappingOptions(
          previewPartStock,
          search.value,
          selectedId,
        );
        const choose = new Option("Choose a reviewed action", "");
        const create = new Option("Create a separate imported stock part", "create");
        const skip = new Option("Skip this stock part", "skip");
        select.replaceChildren(choose, create, skip);
        for (const localPart of options) {
          const option = new Option(
            `${localPart.name} · ${localPart.quantity.toLocaleString()} ${localPart.unit}`,
            `local:${localPart.id}`,
          );
          select.add(option);
        }
        select.value = choice;
        mappingHint.textContent =
          matchCount > MAX_LOCAL_PART_MAPPING_OPTIONS
            ? `Showing ${MAX_LOCAL_PART_MAPPING_OPTIONS} of ${matchCount.toLocaleString()} matches. Refine the search.`
            : `${matchCount.toLocaleString()} local part(s) match.`;
      };
      const updateDetails = () => {
        const choice = selectedPartTargetByExternalRef.get(part.externalRef);
        if (choice === "skip") {
          details.textContent =
            "This stock row will be skipped. Event usage that refers to it will also be skipped.";
        } else if (choice?.startsWith("local:")) {
          details.textContent =
            "The selected stock record will be version-checked and updated with all imported fields and quantity. Historical event usage will link to it without another stock deduction.";
        } else if (choice === "create") {
          details.textContent =
            "A separate stock record will be created from the imported current quantity. Historical event usage will link to it without another stock deduction.";
        } else {
          details.textContent = "Choose whether to create, update, or skip this stock row.";
        }
      };
      refreshOptions();
      updateDetails();
      search.addEventListener("input", refreshOptions);
      select.addEventListener("change", () => {
        if (select.value) selectedPartTargetByExternalRef.set(part.externalRef, select.value);
        else selectedPartTargetByExternalRef.delete(part.externalRef);
        updateDetails();
        importCompleted = false;
        partImportResult = null;
        completedResultByEntry = new Map();
        completedResultSummary = "";
        renderPrintventoryPreview();
      });
      fieldset.append(legend, searchLabel, mappingLabel, mappingHint, details);
      partImportRows.append(fieldset);
    }
  }

  function renderCompletedImportPage(): void {
    importResult.replaceChildren();
    const summary = document.createElement("p");
    summary.textContent = completedResultSummary;
    importResult.append(summary);
    const start = currentImportPage * importReviewPageSize;
    const end = Math.min(importPlan.length, start + importReviewPageSize);
    for (const planRow of importPlan.slice(start, end)) {
      const row = completedResultByEntry.get(planRow.entryIndex);
      if (!row) continue;
      const line = document.createElement("p");
      const name =
        row.targetModelId === null
          ? `Entry ${row.entryIndex + 1}`
          : (previewModels.find((model) => model.id === row.targetModelId)?.display_name ??
            `Entry ${row.entryIndex + 1}`);
      line.textContent = `${name}: ${row.status}${row.mappedFields.length ? `; applied ${row.mappedFields.join(", ")}` : ""}${row.warnings.length ? `; ${row.warnings.join(" ")}` : ""}`;
      importResult.append(line);
    }
  }

  function updateAssignmentState(): void {
    if (!importDocument) {
      applyImport.disabled = true;
      return;
    }
    const assignments = currentAssignments();
    const check = validatePrintventoryAssignments(assignments);
    let printerPayload: ReturnType<typeof buildPrinterDataImportPayload>;
    try {
      printerPayload = buildPrinterDataImportPayload(
        importDocument,
        selectedPrinterTargetByExternalRef,
      );
    } catch (reason) {
      applyImport.disabled = true;
      importResult.textContent = `Printer data could not be prepared: ${String(reason)}`;
      return;
    }
    const printerChoicesReviewed = printerPayload.pendingExternalRefs.length === 0;
    const printers = importDocument.printers ?? [];
    const logs = importDocument.maintenanceLogs ?? [];
    const reminders = importDocument.reminders ?? [];
    const endpointCount = printers.filter((printer) => printer.endpointReviewRequired).length;
    const endpointNotice =
      endpointCount > 0
        ? `${endpointCount.toLocaleString()} profile(s) include an endpoint; imported endpoints stay unapproved and credentials are ignored.`
        : "External IDs are used only to connect records within this import.";
    const unlinkedHistoryNotice = describeUnlinkedPrinterHistory(
      printerPayload.unlinkedMaintenanceLogs,
      printerPayload.unlinkedReminders,
    );
    printerImportSummary.textContent = [
      `${printers.length.toLocaleString()} profile(s), ${logs.length.toLocaleString()} maintenance log(s), and ${reminders.length.toLocaleString()} reminder(s).`,
      unlinkedHistoryNotice,
      endpointNotice,
    ]
      .filter(Boolean)
      .join(" ");
    const importedPartRows = importDocument.parts ?? [];
    const partAssignments: PrintventoryPartAssignment[] = importedPartRows.map((part) => ({
      externalRef: part.externalRef,
      target: selectedPartTargetByExternalRef.get(part.externalRef) ?? "",
    }));
    const partChoices = validatePrintventoryPartAssignments(importedPartRows, partAssignments);
    const partChoicesReviewed = partChoices.valid;
    applyImport.disabled =
      !slicerReview.ready ||
      !printerChoicesReviewed ||
      !partChoicesReviewed ||
      !canApplyPrintventoryImport(
        importDocument !== null,
        importCompleted,
        importApplying,
        assignments,
        reviewedTargetByEntry,
        { ...printerPayload.input, parts: partAssignments, slicers: slicerReview.reviewData },
      );
    if (!check.valid) {
      importCompleted = false;
      importResult.textContent =
        "Each GoggleLab model can be assigned to only one Printventory row. Choose a different target or skip a row.";
    } else if (!slicerReview.ready) {
      importResult.textContent =
        "Choose create or skip for every slicer. Created profiles need a locally selected app and explicit trust.";
    } else if (!printerChoicesReviewed) {
      importResult.textContent =
        "Choose create, skip, or a local mapping for every imported printer profile.";
    } else if (!partChoicesReviewed) {
      importResult.textContent =
        partChoices.reason ?? "Choose create, update, or skip for every imported stock part.";
    } else if (!importCompleted) {
      const needsReview = assignments.some(
        (assignment) =>
          assignment.targetModelId &&
          reviewedTargetByEntry.get(assignment.entryIndex) !== assignment.targetModelId,
      );
      importResult.textContent = needsReview
        ? "Confirm each assigned target and its supported fields before applying. Skipped rows need no confirmation."
        : "Model targets, printer mappings, and stock part choices are reviewed. Apply the selected data when ready.";
    }
  }

  function currentAssignments(): PrintventoryAssignment[] {
    return importPlan.map((row) => ({
      entryIndex: row.entryIndex,
      targetModelId: selectedTargetByEntry.get(row.entryIndex) ?? null,
    }));
  }

  function currentPartAssignments(): PrintventoryPartAssignment[] {
    return (importDocument?.parts ?? []).map((part) => ({
      externalRef: part.externalRef,
      target: selectedPartTargetByExternalRef.get(part.externalRef) ?? "",
    }));
  }

  applyImport.addEventListener("click", async () => {
    if (!importDocument || importApplying || applyImport.disabled) return;
    const assignments = currentAssignments();
    const printerPayload = buildPrinterDataImportPayload(
      importDocument,
      selectedPrinterTargetByExternalRef,
    );
    const partAssignments = currentPartAssignments();
    const selectedIds = new Set(
      assignments.flatMap((assignment) =>
        assignment.targetModelId ? [assignment.targetModelId] : [],
      ),
    );
    importApplying = true;
    slicerReview.setBusy(true);
    const exportLibraryWasDisabled = exportLibrary.disabled;
    const exportPrintventoryJsonWasDisabled = exportPrintventoryJson.disabled;
    applyImport.disabled = true;
    importPrintventory.disabled = true;
    close.disabled = true;
    exportLibrary.disabled = true;
    exportPrintventoryJson.disabled = true;
    for (const fieldset of importPreviewRows.querySelectorAll("fieldset")) {
      fieldset.disabled = true;
    }
    for (const fieldset of printerImportRows.querySelectorAll("fieldset")) {
      fieldset.disabled = true;
    }
    for (const fieldset of partImportRows.querySelectorAll("fieldset")) {
      fieldset.disabled = true;
    }
    for (const button of importPagination.querySelectorAll("button")) {
      button.disabled = true;
    }
    for (const button of printerImportPagination.querySelectorAll("button")) {
      button.disabled = true;
    }
    for (const button of partImportPagination.querySelectorAll("button")) {
      button.disabled = true;
    }
    status.textContent =
      "Checking the reviewed Library targets, printer mappings, and stock parts…";
    error.textContent = "";
    try {
      const [allCurrentModels, currentTags, currentFilaments, currentPrinters, currentPartStock] =
        await Promise.all([
          libraryListModels(),
          libraryListTags(),
          libraryListFilaments(true).then((items) =>
            items.filter((filament) => !filament.archived),
          ),
          libraryListPrinters(),
          libraryListPartStock(),
        ]);
      const currentModels = allCurrentModels;
      const changedModel = [...selectedIds].some((id) => {
        const before = previewModels.find((model) => model.id === id);
        const now = currentModels.find((model) => model.id === id);
        return (
          !before ||
          !now ||
          before.metadata_version !== now.metadata_version ||
          before.display_name !== now.display_name ||
          before.original_name !== now.original_name ||
          before.source_path !== now.source_path
        );
      });
      if (
        changedModel ||
        JSON.stringify(currentTags) !== JSON.stringify(previewTags) ||
        JSON.stringify(currentFilaments) !== JSON.stringify(previewFilaments) ||
        JSON.stringify(
          currentPartStock.map(
            ({ id, name, category, quantity, unit, notes, lowStock, metadataVersion }) => ({
              id,
              name,
              category,
              quantity,
              unit,
              notes,
              lowStock,
              metadataVersion,
            }),
          ),
        ) !==
          JSON.stringify(
            previewPartStock.map(
              ({ id, name, category, quantity, unit, notes, lowStock, metadataVersion }) => ({
                id,
                name,
                category,
                quantity,
                unit,
                notes,
                lowStock,
                metadataVersion,
              }),
            ),
          ) ||
        JSON.stringify(
          currentPrinters.map(
            ({ id, metadataVersion, nickname, make, model, host, webUrl, endpointApproved }) => ({
              id,
              metadataVersion,
              nickname,
              make,
              model,
              host,
              webUrl,
              endpointApproved,
            }),
          ),
        ) !==
          JSON.stringify(
            previewPrinters.map(
              ({ id, metadataVersion, nickname, make, model, host, webUrl, endpointApproved }) => ({
                id,
                metadataVersion,
                nickname,
                make,
                model,
                host,
                webUrl,
                endpointApproved,
              }),
            ),
          )
      ) {
        status.textContent =
          "The local Library, printer fleet, or stock parts changed after preview. Choose the JSON file again and review the updated preview.";
        return;
      }

      await slicerReview.apply(importSlicerProfiles);

      if (!partImportResult && (importDocument.parts?.length ?? 0) > 0) {
        partImportResult = await applyPrintventoryPartAssignments(
          importDocument.parts ?? [],
          partAssignments,
          currentPartStock,
          {
            createPartStock: libraryCreatePartStock,
            updatePartStock: libraryUpdatePartStock,
          },
        );
        previewPartStock = await libraryListPartStock();
      }

      if (
        !printerImportResult &&
        (printerPayload.input.printers.length > 0 ||
          printerPayload.input.maintenanceLogs.length > 0 ||
          printerPayload.input.reminders.length > 0)
      ) {
        printerImportResult = await libraryImportPrinterData(printerPayload.input);
      }
      const printerIdByExternalRef = new Map(
        (printerImportResult?.printerRefMap ?? []).map((entry) => [
          entry.externalRef,
          entry.localPrinterId,
        ]),
      );
      const result = await applyPrintventoryAssignments(
        importDocument.models,
        assignments,
        currentModels,
        currentTags,
        currentFilaments,
        {
          createTag: libraryCreateTag,
          updateMetadata: libraryUpdateModelMetadata,
          setModelFilaments: librarySetModelFilaments,
          listPrintAttempts: libraryListPrintAttempts,
          createPrintAttempt: libraryCreatePrintAttempt,
          importPrintAttempt: libraryImportPrintAttempt,
          importPrintAttempts: libraryImportPrintAttempts,
        },
        printerIdByExternalRef,
        importDocument.parts ?? [],
        partImportResult?.partIdByExternalRef ?? new Map(),
      );
      const skipped = result.rows.filter((row) => row.status === "skipped").length;
      const partial = result.rows.filter((row) => row.status === "partial").length;
      const failed = result.rows.filter((row) => row.status === "failed").length;
      const printerSummary = printerImportResult
        ? ` Printer data: ${printerImportResult.importedPrinterCount} new profile(s), ${printerImportResult.mappedPrinterCount} mapped, ${printerImportResult.importedMaintenanceLogCount} maintenance log(s), ${printerImportResult.importedReminderCount} reminder(s).`
        : "";
      const partSummary = partImportResult
        ? ` Stock parts: ${partImportResult.createdCount} created, ${partImportResult.updatedCount} updated, ${partImportResult.skippedCount} skipped${partImportResult.warnings.length ? `; ${partImportResult.warnings.join(" ")}` : ""}.`
        : "";
      const unlinkedHistoryNotice = describeUnlinkedPrinterHistory(
        printerPayload.unlinkedMaintenanceLogs,
        printerPayload.unlinkedReminders,
      );
      const unlinkedHistorySummary = unlinkedHistoryNotice ? ` ${unlinkedHistoryNotice}` : "";
      completedResultSummary = `Printventory import finished: ${result.updated} model(s) changed (${partial} partial), ${skipped} skipped, ${failed} failed.${partSummary}${printerSummary}${unlinkedHistorySummary}${slicerReview.summary}`;
      status.textContent = completedResultSummary;
      completedResultByEntry = new Map(result.rows.map((row) => [row.entryIndex, row]));
      importCompleted = true;
      await onImportCompleted?.();
      renderCompletedImportPage();
    } catch (reason) {
      error.textContent = `Could not apply Printventory data: ${String(reason)}`;
      status.textContent = "No further Printventory updates were attempted.";
    } finally {
      importApplying = false;
      slicerReview.setBusy(false);
      importPrintventory.disabled = false;
      close.disabled = false;
      exportLibrary.disabled = exportLibraryWasDisabled;
      exportPrintventoryJson.disabled = exportPrintventoryJsonWasDisabled;
      renderPrintventoryPreview();
    }
  });
  exportLibrary.addEventListener("click", async () => {
    error.textContent = "";
    const path = await saveDialog({
      defaultPath: "GoggleLab-Library-Recovery.zip",
      filters: [{ name: "Recovery archive", extensions: ["zip"] }],
    });
    if (!path) return;
    exportLibrary.disabled = true;
    status.textContent = "Reading the Library and verifying managed files…";
    try {
      const manifest = await recoveryExportLibrary([], path);
      status.textContent = `Recovery archive created with ${manifest.models.length} model(s). The Library was not changed.`;
    } catch (reason) {
      error.textContent = `Could not export the Library: ${String(reason)}`;
      await updateStatus();
    } finally {
      exportLibrary.disabled = false;
    }
  });
  exportPrintventoryJson.addEventListener("click", async () => {
    error.textContent = "";
    const path = await saveDialog({
      defaultPath: "GoggleLab-Printventory-Export.json",
      filters: [{ name: "Printventory JSON", extensions: ["json"] }],
    });
    if (!path) return;
    exportPrintventoryJson.disabled = true;
    status.textContent = "Preparing the compatible Library JSON export…";
    try {
      const models = await recoveryExportPrintventoryJson(path);
      status.textContent = `Printventory JSON export created with ${models.toLocaleString()} model(s). Printer endpoints are included as unapproved metadata; credentials and connection status are excluded.`;
    } catch (reason) {
      error.textContent = `Could not export Printventory JSON: ${String(reason)}`;
      await updateStatus();
    } finally {
      exportPrintventoryJson.disabled = false;
    }
  });
  exportColorways.addEventListener("click", async () => {
    error.textContent = "";
    const raw = readRawColorwayValue();
    if (raw === null) {
      exportColorways.disabled = true;
      status.textContent = "No saved color assignments were found in this app profile.";
      return;
    }
    const path = await saveDialog({
      defaultPath: "GoggleLab-Saved-Color-Assignments.json",
      filters: [{ name: "JSON recovery file", extensions: ["json"] }],
    });
    if (!path) return;
    exportColorways.disabled = true;
    try {
      const bytes = await recoveryExportColorways(raw, path);
      status.textContent = `Saved ${bytes.toLocaleString()} raw bytes. The original color assignments were not changed.`;
    } catch (reason) {
      error.textContent = `Could not export saved color assignments: ${String(reason)}`;
    } finally {
      exportColorways.disabled = false;
    }
  });
  close.addEventListener("click", () => dialog.close());
  addFooterControl(launcher);
  window.addEventListener(
    "beforeunload",
    () => {
      launcher.remove();
      dialog.remove();
    },
    { once: true },
  );
}

export function describeImportedFields(entry: Record<string, unknown>): string {
  const labels: Array<[string, string]> = [
    ["designer", "Designer"],
    ["source", "Source"],
    ["notes", "Notes"],
    ["printed", "Printed"],
    ["parentModel", "Parent"],
    ["gogglelabParentModelId", "Stable parent ID"],
    ["license", "License"],
    ["tags", "Tags"],
    ["gogglelabTags", "Tags and categories"],
    ["filaments", "Filaments"],
    ["print_status", "Print status"],
    ["printStatus", "Print status"],
    ["rating", "Rating"],
    ["favorite", "Favorite"],
    ["printEvents", "Print history"],
    ["print_events", "Print history"],
  ];
  const values = labels
    .filter(([key]) => Object.prototype.hasOwnProperty.call(entry, key))
    .map(([key, label]) => {
      const value = entry[key];
      const rendered =
        (key === "printEvents" || key === "print_events") && Array.isArray(value)
          ? `${value.length} event${value.length === 1 ? "" : "s"}`
          : key === "favorite" && typeof value === "boolean"
            ? value
              ? "Yes"
              : "No"
            : typeof value === "string"
              ? value
              : JSON.stringify(value);
      return `${label}: ${rendered ?? "unsupported value"}`;
    });
  if (typeof entry.printed === "boolean") {
    values.push(
      "The legacy printed flag maps current status only; print dates and counts are not inferred.",
    );
  }
  if (typeof entry.fileName === "string")
    values.push(`Filename used for matching only: ${entry.fileName}`);
  if (Object.prototype.hasOwnProperty.call(entry, "filePath")) {
    values.push("File path used only as an in-memory match hint; it is never opened or changed.");
  }
  const recognized = new Set([
    "filePath",
    "fileName",
    "designer",
    "source",
    "notes",
    "printed",
    "parentModel",
    "gogglelabParentModelId",
    "license",
    "tags",
    "gogglelabTags",
    "filaments",
    "print_status",
    "printStatus",
    "rating",
    "favorite",
    "printEvents",
    "print_events",
  ]);
  const ignored = Object.keys(entry).filter((key) => !recognized.has(key));
  if (ignored.length > 0) values.push(`Other model fields not imported: ${ignored.join(", ")}.`);
  return values.length
    ? values.join(" · ")
    : "This row contains no recognized Printventory metadata fields.";
}
