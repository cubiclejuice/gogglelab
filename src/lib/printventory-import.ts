import type {
  Filament,
  LibraryModel,
  LibraryTag,
  MetadataPatch,
  PartStock,
  PartStockInput,
  PrinterInput,
  PrinterMaintenanceInput,
  PrinterReminderInput,
  PrintAttempt,
  PrintAttemptImportItem,
  PrintAttemptImportResult,
  PrintAttemptInput,
  PrintAttemptPart,
  PrintAttemptPartInput,
} from "./ipc";

export const MAX_PRINTVENTORY_JSON_BYTES = 25 * 1024 * 1024;
export const MAX_PRINTVENTORY_MODEL_ENTRIES = 5_000;
export const MAX_PRINTVENTORY_PRINTER_ENTRIES = 5_000;
export const MAX_PRINTVENTORY_PART_ENTRIES = 10_000;
export const MAX_PRINTVENTORY_SLICER_ENTRIES = 64;
export const MAX_PRINTVENTORY_MAINTENANCE_LOG_ENTRIES = 50_000;
export const MAX_PRINTVENTORY_MAINTENANCE_REMINDER_ENTRIES = 10_000;
export const MAX_PRINTVENTORY_TARGET_OPTIONS = 20;
const MAX_PRINTVENTORY_SEARCH_CACHE = 64;
const MAX_PRINTVENTORY_EXTERNAL_REF_BYTES = 200;
const MAX_PRINTER_METADATA_TIMESTAMP_LENGTH = 64;
export const MAX_PRINTVENTORY_EVENT_PARTS = 256;
const MAX_PRINTVENTORY_PART_QUANTITY = 1_000_000;
const MAX_PRINTVENTORY_EVENT_PART_TOTAL = 99_990_000;

export interface PrintventoryPrinterImportRecord {
  externalRef: string;
  input: PrinterInput;
  endpointReviewRequired: boolean;
  createdAt?: string;
  updatedAt?: string;
}

export interface PrintventoryPartImportRecord {
  /** Stable source ID when present, otherwise a row-local review key. */
  externalRef: string;
  sourceId?: string;
  input: PartStockInput;
}

export interface PrintventorySlicerImportRecord {
  name: string;
  /** Display hint only. Never used to inspect or launch a local application. */
  path: string;
}

export interface PrintventoryPartAssignment {
  externalRef: string;
  /** `create`, `skip`, or `local:<part ID>`. */
  target: string;
}

export interface PrintventoryPartImportApi {
  createPartStock(input: PartStockInput): Promise<PartStock>;
  updatePartStock(id: string, expectedVersion: number, input: PartStockInput): Promise<PartStock>;
}

export interface PrintventoryPartImportResult {
  createdCount: number;
  updatedCount: number;
  skippedCount: number;
  partIdByExternalRef: Map<string, string>;
  rows: Array<{
    externalRef: string;
    status: "created" | "updated" | "unchanged" | "skipped" | "failed";
    localPartId: string | null;
    warning?: string;
  }>;
  warnings: string[];
}

export interface PrintventoryMaintenanceLogImportRecord {
  externalRef: string;
  printerExternalRef: string | null;
  printerNickname: string;
  input: PrinterMaintenanceInput;
  createdAt?: string;
}

export interface PrintventoryMaintenanceReminderImportRecord {
  externalRef: string;
  printerExternalRef: string | null;
  printerNickname?: string;
  input: PrinterReminderInput;
  status: "active" | "paused" | "completed";
  lastCompletedAt?: string;
  createdAt?: string;
  updatedAt?: string;
}

export interface PrintventoryModelRecord extends Record<string, unknown> {}

export interface PrintventoryExport extends Record<string, unknown> {
  models: PrintventoryModelRecord[];
  slicers?: PrintventorySlicerImportRecord[];
  parts?: PrintventoryPartImportRecord[];
  printers?: PrintventoryPrinterImportRecord[];
  maintenanceLogs?: PrintventoryMaintenanceLogImportRecord[];
  reminders?: PrintventoryMaintenanceReminderImportRecord[];
}

export interface PrintventoryReviewedDataCollections {
  slicers?: readonly unknown[];
  printers?: readonly unknown[];
  maintenanceLogs?: readonly unknown[];
  reminders?: readonly unknown[];
  parts?: readonly unknown[];
}

export interface PrintventoryImportPlanRow {
  entryIndex: number;
  entry: PrintventoryModelRecord;
  suggestedTargetId: string | null;
  matchKind: "source-path" | "unique-filename" | "ambiguous" | "unmatched";
  suggestionMessage: string;
}

export interface PrintventoryAssignment {
  entryIndex: number;
  targetModelId: string | null;
}

export interface PrintventoryTagAssignment {
  name: string;
  category?: string | null;
}

export interface PrintventoryFieldMapping {
  patch: MetadataPatch;
  tagNames?: string[];
  tagAssignments?: PrintventoryTagAssignment[];
  filamentIds?: string[];
  parts?: PrintAttemptInput["parts"];
  printAttempts?: ImportedPrintAttempt[];
  mappedFields: string[];
  warnings: string[];
}

export interface ImportedPrintAttempt {
  input: PrintAttemptInput;
  createdAt?: string;
  printerExternalRef?: string;
  printerSnapshot?: PrintventoryPrinterSnapshot;
}

export interface PrintventoryPrinterSnapshot {
  printer_nickname?: string;
  printer_manufacturer?: string;
  printer_model?: string;
  printer_type?: string;
}

export interface PrintventoryImportRowResult {
  entryIndex: number;
  targetModelId: string | null;
  status: "updated" | "partial" | "skipped" | "failed";
  mappedFields: string[];
  warnings: string[];
}

export interface PrintventoryImportResult {
  updated: number;
  rows: PrintventoryImportRowResult[];
}

export interface PrintventoryImportApi {
  createTag(name: string, category: string | null): Promise<LibraryTag>;
  updateMetadata(
    modelId: string,
    expectedVersion: number,
    patch: MetadataPatch,
  ): Promise<LibraryModel>;
  setModelFilaments(
    modelId: string,
    expectedVersion: number,
    filamentIds: string[],
  ): Promise<LibraryModel>;
  listPrintAttempts(modelId: string): Promise<PrintAttempt[]>;
  createPrintAttempt(modelId: string, input: PrintAttemptInput): Promise<PrintAttempt>;
  importPrintAttempt(
    modelId: string,
    input: PrintAttemptInput,
    createdAt: string | null,
    printerId: string | null,
    printerName: string | null,
  ): Promise<PrintAttempt>;
  importPrintAttempts?(
    modelId: string,
    attempts: PrintAttemptImportItem[],
  ): Promise<PrintAttemptImportResult>;
}

const hasOwn = (value: object, key: string): boolean =>
  Object.prototype.hasOwnProperty.call(value, key);

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function hasImportControlCharacters(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

function validateImportString(
  value: unknown,
  label: string,
  maxBytes: number,
  required = false,
): string | null | undefined {
  if (value === undefined) {
    if (required) throw new Error(`Printventory ${label} is required.`);
    return undefined;
  }
  if (value === null) {
    if (required) throw new Error(`Printventory ${label} is required.`);
    return null;
  }
  if (typeof value !== "string") {
    throw new Error(`Printventory ${label} must be text.`);
  }
  const normalized = value.trim();
  if (required && !normalized) throw new Error(`Printventory ${label} is required.`);
  if (
    new TextEncoder().encode(normalized).length > maxBytes ||
    hasImportControlCharacters(normalized)
  ) {
    throw new Error(`Printventory ${label} exceeds its text limits.`);
  }
  return normalized;
}

function readImportField(
  record: Record<string, unknown>,
  snakeCase: string,
  camelCase?: string,
): unknown {
  if (hasOwn(record, snakeCase)) return record[snakeCase];
  if (camelCase && hasOwn(record, camelCase)) return record[camelCase];
  return undefined;
}

function validateExternalRef(value: unknown, label: string): string {
  let normalizedValue = value;
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 1) {
      throw new Error(`Printventory ${label} must be a positive safe integer or text.`);
    }
    normalizedValue = String(value);
  }
  const ref = validateImportString(
    normalizedValue,
    label,
    MAX_PRINTVENTORY_EXTERNAL_REF_BYTES,
    true,
  );
  if (typeof ref !== "string") throw new Error(`Printventory ${label} is required.`);
  return ref;
}

function endpointContainsCredentials(value: string): boolean {
  const authority =
    value.match(/^[a-z][a-z\d+.-]*:\/\/([^/?#]*)/i)?.[1] ?? value.split(/[/?#]/, 1)[0];
  if (authority.includes("@")) return true;
  return /(?:^|[?&#;\s])(?:user(?:name)?|pass(?:word|wd)?|token|access[_-]?token|api[_-]?key|secret|access[_-]?code|authorization)\s*[:=]\s*[^&#;\s]*/i.test(
    value,
  );
}

function normalizePrinterWebUrl(value: string): string | null {
  if (endpointContainsCredentials(value)) return null;
  try {
    const url = new URL(value);
    if (
      (url.protocol !== "http:" && url.protocol !== "https:") ||
      !url.hostname ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    ) {
      return null;
    }
    // Printer web URLs are inert metadata. Paths can contain bearer tokens, so
    // only the origin crosses the import boundary.
    return url.origin;
  } catch {
    return null;
  }
}

function validateRecordExternalRef(
  record: Record<string, unknown>,
  specificKeys: string | readonly string[],
  label: string,
): string {
  const keys = typeof specificKeys === "string" ? [specificKeys] : specificKeys;
  const specificValue = keys
    .map((key) => record[key])
    .find((value) => value !== undefined && value !== null);
  return validateExternalRef(specificValue ?? record.id, label);
}

function readPrinterExternalRef(
  record: Record<string, unknown>,
  required: boolean,
  label: string,
): string | null {
  const value = record.gogglelab_printer_id ?? record.printer_id;
  if (value === undefined || value === null) {
    return required ? validateExternalRef(undefined, label) : null;
  }
  return validateExternalRef(value, label);
}

function parseImportDate(
  value: unknown,
  label: string,
  required = false,
): string | null | undefined {
  if (value === undefined) {
    if (required) throw new Error(`Printventory ${label} is required.`);
    return undefined;
  }
  if (value === null && !required) return null;
  const normalized = normalizeImportedPrintTimestamp(value);
  if (!normalized) throw new Error(`Printventory ${label} must be a valid date.`);
  return normalized;
}

function parseOptionalPrinterImportTimestamp(
  record: Record<string, unknown>,
  snakeCaseKey: string,
  camelCaseKey: string,
  label: string,
): string | undefined {
  const value = readImportField(record, snakeCaseKey, camelCaseKey);
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string" || value.length > MAX_PRINTER_METADATA_TIMESTAMP_LENGTH) {
    throw new Error(`Printventory ${label} must be a valid date.`);
  }
  const parsed = normalizeImportedPrintTimestamp(value, false);
  if (!parsed) throw new Error(`Printventory ${label} must be a valid date.`);
  return parsed;
}

function parsePrintventoryPrinter(value: unknown, index: number): PrintventoryPrinterImportRecord {
  const record = asRecord(value);
  if (!record) throw new Error(`Printventory printer entry ${index + 1} must be an object.`);
  const externalRef = validateExternalRef(
    record.gogglelab_printer_id ?? record.id,
    `printer entry ${index + 1} ID`,
  );
  const getString = (field: string, maxBytes: number): string | null => {
    if (!hasOwn(record, field)) return null;
    return validateImportString(record[field], `printer ${field}`, maxBytes) ?? null;
  };
  const nickname = validateImportString(
    record.nickname,
    `printer entry ${index + 1} nickname`,
    120,
    true,
  );
  if (typeof nickname !== "string") {
    throw new Error(`Printventory printer entry ${index + 1} nickname is required.`);
  }
  const make = getString("manufacturer", 120);
  const model = getString("model", 160);
  const printerType = getString("printer_type", 80);
  const firmware = getString("firmware_type", 120);
  const importedHost = getString("host", 255);
  const importedWebUrl = getString("web_url", 2_048);
  const createdAt = parseOptionalPrinterImportTimestamp(
    record,
    "created_at",
    "createdAt",
    `printer entry ${index + 1} creation date`,
  );
  const updatedAt = parseOptionalPrinterImportTimestamp(
    record,
    "updated_at",
    "updatedAt",
    `printer entry ${index + 1} update date`,
  );
  const host = importedHost && !endpointContainsCredentials(importedHost) ? importedHost : null;
  const webUrl = importedWebUrl ? normalizePrinterWebUrl(importedWebUrl) : null;
  const notes = getString("notes", 20_000) ?? "";
  let bedMm: [number, number, number] | null = null;
  if (hasOwn(record, "bed_mm")) {
    const bed = record.bed_mm;
    if (bed === null) {
      bedMm = null;
    } else if (
      Array.isArray(bed) &&
      bed.length === 3 &&
      bed.every(
        (dimension) =>
          typeof dimension === "number" &&
          Number.isFinite(dimension) &&
          dimension > 0 &&
          dimension <= 10_000,
      )
    ) {
      bedMm = [bed[0], bed[1], bed[2]];
    } else {
      throw new Error(`Printventory printer bed_mm for entry ${index + 1} is invalid.`);
    }
  }
  return {
    externalRef,
    input: {
      nickname,
      make,
      model,
      printerType,
      firmware,
      host,
      webUrl,
      bedMm,
      notes,
    },
    endpointReviewRequired: Boolean(host || webUrl),
    ...(createdAt ? { createdAt } : {}),
    ...(updatedAt ? { updatedAt } : {}),
  };
}

function parsePrintventoryPart(value: unknown, index: number): PrintventoryPartImportRecord {
  const record = asRecord(value);
  if (!record) throw new Error(`Printventory part entry ${index + 1} must be an object.`);
  const stableIdValue =
    readImportField(record, "gogglelab_part_id", "gogglelabPartId") ?? record.id;
  let sourceId: string | undefined;
  if (stableIdValue !== undefined && stableIdValue !== null) {
    try {
      sourceId = validateExternalRef(stableIdValue, `part entry ${index + 1} ID`);
    } catch {
      throw new Error(`Printventory part entry ${index + 1} has an invalid ID.`);
    }
  }
  const name = validateImportString(record.name, `part entry ${index + 1} name`, 128, true);
  const unit = validateImportString(record.unit, `part entry ${index + 1} unit`, 128, true);
  if (typeof name !== "string" || typeof unit !== "string") {
    throw new Error(`Printventory part entry ${index + 1} needs a name and unit.`);
  }
  const rawCategory = readImportField(record, "category");
  const category =
    rawCategory === null || rawCategory === undefined
      ? null
      : (validateImportString(rawCategory, `part entry ${index + 1} category`, 128) ?? null);
  const quantity = record.quantity ?? 0;
  const rawLowStock = readImportField(record, "low_stock", "lowStock");
  const lowStock = rawLowStock ?? 0;
  if (
    typeof quantity !== "number" ||
    !Number.isSafeInteger(quantity) ||
    quantity < 0 ||
    quantity > MAX_PRINTVENTORY_PART_QUANTITY ||
    typeof lowStock !== "number" ||
    !Number.isSafeInteger(lowStock) ||
    lowStock < 0 ||
    lowStock > MAX_PRINTVENTORY_PART_QUANTITY
  ) {
    throw new Error(`Printventory part entry ${index + 1} has invalid stock quantities.`);
  }
  const notes = validateImportString(record.notes ?? "", `part entry ${index + 1} notes`, 20_000);
  return {
    externalRef: sourceId ?? `row:${index + 1}`,
    ...(sourceId !== undefined ? { sourceId } : {}),
    input: {
      name,
      category: typeof category === "string" ? category : null,
      quantity,
      unit,
      notes: typeof notes === "string" ? notes : "",
      lowStock,
    },
  };
}

function parsePrintventoryMaintenanceLog(
  value: unknown,
  index: number,
): PrintventoryMaintenanceLogImportRecord {
  const record = asRecord(value);
  if (!record) {
    throw new Error(`Printventory printer maintenance log ${index + 1} must be an object.`);
  }
  const title = validateImportString(record.title, `maintenance log ${index + 1} title`, 200, true);
  const performedAt = parseImportDate(
    readImportField(record, "performed_at", "performedAt"),
    `maintenance log ${index + 1} performed date`,
    true,
  );
  const notes = validateImportString(
    record.notes ?? "",
    `maintenance log ${index + 1} notes`,
    20_000,
  );
  const printerNickname = validateImportString(
    readImportField(record, "printer_nickname", "printerNickname"),
    `maintenance log ${index + 1} printer nickname`,
    120,
  );
  const createdAt = parseImportDate(
    readImportField(record, "created_at", "createdAt"),
    `maintenance log ${index + 1} creation date`,
  );
  if (typeof title !== "string" || typeof performedAt !== "string") {
    throw new Error(`Printventory printer maintenance log ${index + 1} is invalid.`);
  }
  const externalRef = validateRecordExternalRef(
    record,
    ["gogglelab_maintenance_log_id", "gogglelab_printer_maintenance_log_id"],
    `maintenance log ${index + 1} ID`,
  );
  return {
    externalRef,
    printerExternalRef: readPrinterExternalRef(
      record,
      false,
      `maintenance log ${index + 1} printer reference`,
    ),
    printerNickname: printerNickname ?? "",
    input: { title, performedAt, notes: notes ?? "" },
    ...(typeof createdAt === "string" ? { createdAt } : {}),
  };
}

function parsePrintventoryMaintenanceReminder(
  value: unknown,
  index: number,
): PrintventoryMaintenanceReminderImportRecord {
  const record = asRecord(value);
  if (!record) {
    throw new Error(`Printventory printer maintenance reminder ${index + 1} must be an object.`);
  }
  const title = validateImportString(
    record.title,
    `maintenance reminder ${index + 1} title`,
    200,
    true,
  );
  const notes = validateImportString(
    record.notes ?? "",
    `maintenance reminder ${index + 1} notes`,
    20_000,
  );
  const printerNickname = validateImportString(
    readImportField(record, "printer_nickname", "printerNickname"),
    `maintenance reminder ${index + 1} printer nickname`,
    120,
  );
  const dueAt = parseImportDate(
    readImportField(record, "due_at", "dueAt"),
    `maintenance reminder ${index + 1} due date`,
    true,
  );
  const intervalDaysValue = readImportField(record, "interval_days", "intervalDays");
  let intervalDays: number | null = null;
  if (intervalDaysValue !== undefined && intervalDaysValue !== null) {
    if (
      typeof intervalDaysValue !== "number" ||
      !Number.isSafeInteger(intervalDaysValue) ||
      intervalDaysValue < 1 ||
      intervalDaysValue > 36_500
    ) {
      throw new Error(`Printventory maintenance reminder ${index + 1} interval is invalid.`);
    }
    intervalDays = intervalDaysValue;
  }
  const statusValue = record.status === undefined ? "active" : record.status;
  if (statusValue !== "active" && statusValue !== "paused" && statusValue !== "completed") {
    throw new Error(`Printventory maintenance reminder ${index + 1} status is invalid.`);
  }
  const lastCompletedAt = parseImportDate(
    readImportField(record, "last_completed_at", "lastCompletedAt"),
    `maintenance reminder ${index + 1} completion date`,
  );
  const createdAt = parseImportDate(
    readImportField(record, "created_at", "createdAt"),
    `maintenance reminder ${index + 1} creation date`,
  );
  const updatedAt = parseImportDate(
    readImportField(record, "updated_at", "updatedAt"),
    `maintenance reminder ${index + 1} update date`,
  );
  if (typeof title !== "string" || typeof dueAt !== "string") {
    throw new Error(`Printventory printer maintenance reminder ${index + 1} is invalid.`);
  }
  const externalRef = validateRecordExternalRef(
    record,
    ["gogglelab_printer_reminder_id", "gogglelab_printer_maintenance_reminder_id"],
    `maintenance reminder ${index + 1} ID`,
  );
  return {
    externalRef,
    printerExternalRef: readPrinterExternalRef(
      record,
      false,
      `maintenance reminder ${index + 1} printer reference`,
    ),
    ...(typeof printerNickname === "string" ? { printerNickname } : {}),
    input: { title, notes: notes ?? "", dueAt, intervalDays },
    status: statusValue,
    ...(typeof lastCompletedAt === "string" ? { lastCompletedAt } : {}),
    ...(typeof createdAt === "string" ? { createdAt } : {}),
    ...(typeof updatedAt === "string" ? { updatedAt } : {}),
  };
}

function parseOptionalPrintventoryCollection<T>(
  root: Record<string, unknown>,
  field: string,
  maxEntries: number,
  parser: (value: unknown, index: number) => T,
): T[] | undefined {
  if (!hasOwn(root, field)) return undefined;
  const collection = root[field];
  if (!Array.isArray(collection)) {
    throw new Error(`The Printventory export ${field} field must be an array.`);
  }
  if (collection.length > maxEntries) {
    throw new Error(
      `The Printventory export exceeds the ${maxEntries.toLocaleString()} ${field} import limit.`,
    );
  }
  return collection.map(parser);
}

function parsePrintventorySlicer(value: unknown, index: number): PrintventorySlicerImportRecord {
  const row = asRecord(value);
  if (!row || typeof row.name !== "string" || typeof row.path !== "string") {
    throw new Error(`Printventory slicer ${index + 1} must contain a name and path.`);
  }
  const name = row.name.trim();
  const path = row.path.trim();
  if (!name || [...name].length > 128 || hasImportControlCharacters(row.name)) {
    throw new Error(`Printventory slicer ${index + 1} has an invalid name.`);
  }
  if (!path || [...path].length > 4096 || hasImportControlCharacters(row.path)) {
    throw new Error(`Printventory slicer ${index + 1} has an invalid path hint.`);
  }
  return { name, path };
}

export function assertPrintventoryExportBounds(byteLength: number, modelCount: number): void {
  if (!Number.isSafeInteger(byteLength) || byteLength < 0) {
    throw new Error("The selected Printventory JSON has an invalid file size.");
  }
  if (byteLength > MAX_PRINTVENTORY_JSON_BYTES) {
    throw new Error("The selected Printventory JSON exceeds the 25 MiB import limit.");
  }
  if (!Number.isSafeInteger(modelCount) || modelCount < 0) {
    throw new Error("The Printventory export has an invalid model-entry count.");
  }
  if (modelCount > MAX_PRINTVENTORY_MODEL_ENTRIES) {
    throw new Error("The Printventory export exceeds the 5,000 model entries import limit.");
  }
}

export function parsePrintventoryExport(json: string): PrintventoryExport {
  const byteLength = new TextEncoder().encode(json).length;
  assertPrintventoryExportBounds(byteLength, 0);
  let decoded: unknown;
  try {
    decoded = JSON.parse(json);
  } catch {
    throw new Error("The selected file is not valid JSON.");
  }
  const root = asRecord(decoded);
  if (!root) throw new Error("The Printventory export must be a JSON export object.");
  if (!Array.isArray(root.models)) {
    throw new Error("The Printventory export must contain a models array.");
  }
  assertPrintventoryExportBounds(byteLength, root.models.length);
  const models = root.models.map((model, index) => {
    const record = asRecord(model);
    if (!record) throw new Error(`Printventory model entry ${index + 1} must be an object.`);
    return record;
  });
  const importedPrinters = parseOptionalPrintventoryCollection(
    root,
    "printers",
    MAX_PRINTVENTORY_PRINTER_ENTRIES,
    parsePrintventoryPrinter,
  );
  const importedParts = parseOptionalPrintventoryCollection(
    root,
    "parts",
    MAX_PRINTVENTORY_PART_ENTRIES,
    parsePrintventoryPart,
  );
  const slicers = parseOptionalPrintventoryCollection(
    root,
    "slicers",
    MAX_PRINTVENTORY_SLICER_ENTRIES,
    parsePrintventorySlicer,
  );
  const partRefs = new Set<string>();
  for (const part of importedParts ?? []) {
    if (partRefs.has(part.externalRef)) {
      throw new Error("The Printventory export contains a duplicate part external reference.");
    }
    partRefs.add(part.externalRef);
  }
  const maintenanceLogs = parseOptionalPrintventoryCollection(
    root,
    "printer_maintenance_logs",
    MAX_PRINTVENTORY_MAINTENANCE_LOG_ENTRIES,
    parsePrintventoryMaintenanceLog,
  );
  const maintenanceReminders = parseOptionalPrintventoryCollection(
    root,
    "printer_maintenance_reminders",
    MAX_PRINTVENTORY_MAINTENANCE_REMINDER_ENTRIES,
    parsePrintventoryMaintenanceReminder,
  );
  const printerProfiles = [...(importedPrinters ?? [])];
  const printerRefs = new Set<string>();
  for (const printer of printerProfiles) {
    if (printerRefs.has(printer.externalRef)) {
      throw new Error("The Printventory export contains a duplicate printer external reference.");
    }
    printerRefs.add(printer.externalRef);
  }
  const addSnapshotCandidate = (externalRef: string | null, printerNickname: string): void => {
    const nickname = printerNickname.trim();
    if (!externalRef || !nickname || printerRefs.has(externalRef)) return;
    printerProfiles.push({
      externalRef,
      input: {
        nickname,
        make: null,
        model: null,
        printerType: null,
        firmware: null,
        host: null,
        webUrl: null,
        bedMm: null,
        notes: "",
      },
      endpointReviewRequired: false,
    });
    printerRefs.add(externalRef);
  };
  for (const log of maintenanceLogs ?? []) {
    addSnapshotCandidate(log.printerExternalRef, log.printerNickname);
  }
  for (const reminder of maintenanceReminders ?? []) {
    addSnapshotCandidate(reminder.printerExternalRef, reminder.printerNickname ?? "");
  }
  if (printerProfiles.length > MAX_PRINTVENTORY_PRINTER_ENTRIES) {
    throw new Error(
      `The Printventory export exceeds the ${MAX_PRINTVENTORY_PRINTER_ENTRIES.toLocaleString()} printer profile limit.`,
    );
  }
  const printers =
    importedPrinters !== undefined || printerProfiles.length > 0 ? printerProfiles : undefined;
  const normalizedRoot = { ...root };
  delete normalizedRoot.printer_maintenance_logs;
  delete normalizedRoot.printer_maintenance_reminders;
  return {
    ...normalizedRoot,
    models,
    ...(slicers !== undefined ? { slicers } : {}),
    ...(importedParts !== undefined ? { parts: importedParts } : {}),
    ...(printers !== undefined ? { printers } : {}),
    ...(maintenanceLogs !== undefined ? { maintenanceLogs } : {}),
    ...(maintenanceReminders !== undefined ? { reminders: maintenanceReminders } : {}),
  };
}

export async function readPrintventoryExportFile(
  file: Pick<Blob, "size" | "text">,
): Promise<PrintventoryExport> {
  assertPrintventoryExportBounds(file.size, 0);
  return parsePrintventoryExport(await file.text());
}

function normalizeFilename(value: string): string {
  return value.trim().toLocaleLowerCase();
}

function isLocalImportModel(model: LibraryModel): boolean {
  return model.source_kind === "indexed" || model.source_kind === "managed";
}

function pushIndex(map: Map<string, LibraryModel[]>, key: string, model: LibraryModel): void {
  const values = map.get(key);
  if (values) values.push(model);
  else map.set(key, [model]);
}

export type PrintventoryTargetSearch = (
  query: string,
  selectedModelId?: string | null,
) => LibraryModel[];

export function printventoryTargetLabel(
  model: Pick<LibraryModel, "display_name" | "original_name" | "id">,
): string {
  const filename = model.original_name ? ` — ${model.original_name}` : "";
  return `${model.display_name}${filename} · ${model.id}`;
}

export function createPrintventoryTargetSearch(
  inventory: LibraryModel[],
): PrintventoryTargetSearch {
  const indexedModels = inventory.filter(isLocalImportModel);
  indexedModels.sort((left, right) => left.display_name.localeCompare(right.display_name));
  const indexedById = new Map(indexedModels.map((model) => [model.id, model]));
  const cache = new Map<string, LibraryModel[]>();
  return (query, selectedModelId) => {
    const normalizedQuery = normalizeFilename(query);
    let candidates = cache.get(normalizedQuery);
    if (!candidates) {
      candidates = indexedModels
        .filter((model) => {
          if (!normalizedQuery) return true;
          return [model.display_name, model.original_name ?? "", model.id].some((value) =>
            normalizeFilename(value).includes(normalizedQuery),
          );
        })
        .slice(0, MAX_PRINTVENTORY_TARGET_OPTIONS);
      if (cache.size >= MAX_PRINTVENTORY_SEARCH_CACHE) {
        const oldestQuery = cache.keys().next().value;
        if (oldestQuery !== undefined) cache.delete(oldestQuery);
      }
      cache.set(normalizedQuery, candidates);
    }
    if (!selectedModelId || candidates.some((model) => model.id === selectedModelId)) {
      return candidates;
    }
    const selected = indexedById.get(selectedModelId);
    if (!selected) return candidates;
    return [...candidates.slice(0, MAX_PRINTVENTORY_TARGET_OPTIONS - 1), selected];
  };
}

export function buildPrintventoryImportPlan(
  document: Pick<PrintventoryExport, "models">,
  inventory: LibraryModel[],
): PrintventoryImportPlanRow[] {
  const indexedModels = inventory.filter(isLocalImportModel);
  const modelsBySourcePath = new Map<string, LibraryModel[]>();
  const modelsByFilename = new Map<string, LibraryModel[]>();
  for (const model of indexedModels) {
    if (typeof model.source_path === "string") {
      pushIndex(modelsBySourcePath, model.source_path, model);
    }
    const names = new Set(
      [model.original_name, model.display_name]
        .filter((name): name is string => Boolean(name))
        .map(normalizeFilename),
    );
    for (const name of names) pushIndex(modelsByFilename, name, model);
  }
  return document.models.map((entry, entryIndex) => {
    const pathHint = typeof entry.filePath === "string" ? entry.filePath : null;
    const pathMatches = pathHint ? (modelsBySourcePath.get(pathHint) ?? []) : [];
    if (pathMatches.length === 1) {
      return {
        entryIndex,
        entry,
        suggestedTargetId: pathMatches[0].id,
        matchKind: "source-path",
        suggestionMessage: "Suggested by an exact in-memory source-path match.",
      };
    }
    if (pathMatches.length > 1) {
      return {
        entryIndex,
        entry,
        suggestedTargetId: null,
        matchKind: "ambiguous",
        suggestionMessage:
          "This source-path hint matches multiple Library models; choose a target.",
      };
    }

    const filename = typeof entry.fileName === "string" ? normalizeFilename(entry.fileName) : "";
    const filenameMatchesForEntry = filename ? (modelsByFilename.get(filename) ?? []) : [];
    if (filenameMatchesForEntry.length === 1) {
      return {
        entryIndex,
        entry,
        suggestedTargetId: filenameMatchesForEntry[0].id,
        matchKind: "unique-filename",
        suggestionMessage:
          "Suggested by a unique filename match. Review the target before applying.",
      };
    }
    if (filenameMatchesForEntry.length > 1) {
      return {
        entryIndex,
        entry,
        suggestedTargetId: null,
        matchKind: "ambiguous",
        suggestionMessage:
          "This filename match is ambiguous across multiple Library models; choose a target.",
      };
    }
    return {
      entryIndex,
      entry,
      suggestedTargetId: null,
      matchKind: "unmatched",
      suggestionMessage: "No match was found; choose a target or leave this entry skipped.",
    };
  });
}

export function validatePrintventoryAssignments(assignments: readonly PrintventoryAssignment[]): {
  valid: boolean;
  duplicateTargetIds: string[];
} {
  const counts = new Map<string, number>();
  for (const assignment of assignments) {
    if (!assignment.targetModelId) continue;
    counts.set(assignment.targetModelId, (counts.get(assignment.targetModelId) ?? 0) + 1);
  }
  const duplicateTargetIds = [...counts].filter(([, count]) => count > 1).map(([id]) => id);
  return { valid: duplicateTargetIds.length === 0, duplicateTargetIds };
}

export function canApplyPrintventoryImport(
  hasDocument: boolean,
  completed: boolean,
  applying: boolean,
  assignments: readonly PrintventoryAssignment[],
  reviewedTargets: ReadonlyMap<number, string> = new Map(),
  reviewedPrinterData?: PrintventoryReviewedDataCollections,
): boolean {
  if (!hasDocument || completed || applying) return false;
  const check = validatePrintventoryAssignments(assignments);
  const hasAssignedModels = assignments.some((assignment) => Boolean(assignment.targetModelId));
  const hasPrinterData = Boolean(
    reviewedPrinterData?.printers?.length ||
    reviewedPrinterData?.maintenanceLogs?.length ||
    reviewedPrinterData?.reminders?.length ||
    reviewedPrinterData?.slicers?.length ||
    reviewedPrinterData?.parts?.length,
  );
  return (
    check.valid &&
    (hasAssignedModels || hasPrinterData) &&
    assignments.every(
      (assignment) =>
        !assignment.targetModelId ||
        reviewedTargets.get(assignment.entryIndex) === assignment.targetModelId,
    )
  );
}

function explicitTagName(value: unknown): string | null {
  if (typeof value === "string" && value.trim().length > 0) return value.trim();
  const record = asRecord(value);
  if (record && typeof record.name === "string" && record.name.trim().length > 0) {
    return record.name.trim();
  }
  return null;
}

function normalizeTagName(name: string): string {
  return name.trim().toLocaleLowerCase();
}

function isSafeLocalTagName(name: string): boolean {
  return (
    name.length > 0 &&
    name !== "." &&
    name !== ".." &&
    !name.includes("/") &&
    !name.includes("\\") &&
    !Array.from(name).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
    }) &&
    new TextEncoder().encode(name).length <= 256
  );
}

export function missingPrintventoryTagNames(
  names: readonly string[],
  tags: readonly LibraryTag[],
): string[] {
  return names.filter(
    (name) => !tags.some((tag) => normalizeTagName(tag.name) === normalizeTagName(name)),
  );
}

function hasSourceControlCharacters(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

function isSafeSourceUrl(value: string): boolean {
  if (hasSourceControlCharacters(value)) return false;
  const trimmed = value.trim();
  const schemeEnd = trimmed.indexOf("://");
  if (schemeEnd < 0) return false;
  const authority = trimmed.slice(schemeEnd + 3).split(/[/?#]/, 1)[0] ?? "";
  if (authority.includes("@")) return false;
  try {
    const url = new URL(trimmed);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.username.length === 0 &&
      url.password.length === 0 &&
      !authority.includes("@")
    );
  } catch {
    return false;
  }
}

function isSafeSourceOrigin(value: string): boolean {
  return value.trim().length > 0 && !/[:/\\?#]/.test(value) && !hasSourceControlCharacters(value);
}

function normalizeImportedPrintTimestamp(value: unknown, allowDateOnly = true): string | null {
  if (typeof value !== "string") return null;
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    if (!allowDateOnly) return null;
    const parsed = new Date(value + "T00:00:00.000Z");
    return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value
      ? parsed.toISOString()
      : null;
  }
  const match = value.match(
    /^(\d{4}-\d{2}-\d{2})[Tt](\d{2}:\d{2}:\d{2})(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/,
  );
  if (!match) return null;
  const dateStart = new Date(match[1] + "T00:00:00.000Z");
  const [hour, minute, second] = match[2].split(":").map(Number);
  if (
    !Number.isFinite(dateStart.getTime()) ||
    dateStart.toISOString().slice(0, 10) !== match[1] ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return null;
  }
  const seconds = Date.parse(match[1] + "T" + match[2] + match[4]);
  if (!Number.isFinite(seconds)) return null;
  const fractionalDigits = (match[3] ?? "").slice(1).replace(/0+$/, "");
  const fraction = fractionalDigits ? "." + fractionalDigits : "";
  return new Date(seconds).toISOString().slice(0, 19) + fraction + "Z";
}

function printAttemptIdentity(
  input: Omit<PrintAttemptInput, "parts"> & {
    parts?: Array<PrintAttemptPartInput | PrintAttemptPart>;
  },
  printerId: string | null,
  printerName: string | null,
): string {
  return JSON.stringify({
    printed_at: normalizeImportedPrintTimestamp(input.printed_at) ?? input.printed_at,
    quantity: input.quantity,
    notes: input.notes,
    filament_ids: input.filament_ids,
    outcome: input.outcome,
    parts: (input.parts ?? [])
      .map((part) => ({
        id: part.id,
        quantityPerCopy: "quantityPerCopy" in part ? part.quantityPerCopy : part.quantity,
      }))
      .sort((left, right) => String(left.id).localeCompare(String(right.id))),
    printer_id: printerId,
    printer_name: printerName,
  });
}

function readPrintEventPrinterSnapshot(
  event: Record<string, unknown>,
  eventIndex: number,
  warnings: string[],
): PrintventoryPrinterSnapshot | undefined {
  const fields = [
    ["printer_nickname", 120],
    ["printer_manufacturer", 120],
    ["printer_model", 160],
    ["printer_type", 80],
  ] as const;
  const snapshot: PrintventoryPrinterSnapshot = {};
  for (const [field, maxBytes] of fields) {
    const raw = event[field];
    if (raw === undefined || raw === null || raw === "") continue;
    try {
      const normalized = validateImportString(
        raw,
        `print event ${eventIndex + 1} ${field}`,
        maxBytes,
      );
      if (typeof normalized === "string") Object.assign(snapshot, { [field]: normalized });
    } catch {
      warnings.push(`Print event ${eventIndex + 1} ${field} was skipped because it is invalid.`);
    }
  }
  return Object.keys(snapshot).length > 0 ? snapshot : undefined;
}

function mapFilamentReference(
  value: unknown,
  filaments: Filament[],
  allowArchived = false,
): string | null {
  const record = asRecord(value);
  if (!record) return null;
  const brand =
    typeof record.brand === "string"
      ? record.brand
      : typeof record.vendor === "string"
        ? record.vendor
        : null;
  const material = typeof record.material === "string" ? record.material : null;
  const color =
    typeof record.color === "string"
      ? record.color
      : typeof record.color_hex === "string"
        ? record.color_hex
        : null;
  const name = typeof record.name === "string" ? record.name : null;
  if (brand === null || material === null || color === null || name === null) return null;

  const source = typeof record.source === "string" ? record.source.toLocaleLowerCase() : null;
  const spoolmanId =
    typeof record.spoolman_id === "string"
      ? record.spoolman_id
      : typeof record.external_id === "string"
        ? record.external_id
        : null;
  if (source !== null && source !== "manual" && source !== "spoolman") return null;
  if (source === "manual" && spoolmanId !== null) return null;
  if (source === "spoolman" && !spoolmanId) return null;
  const requiresSpoolmanIdentity = source === "spoolman" || spoolmanId !== null;
  if (requiresSpoolmanIdentity && !spoolmanId) return null;

  const matches = filaments.filter(
    (filament) =>
      (allowArchived || !filament.archived) &&
      filament.brand === brand &&
      filament.material === material &&
      filament.color === color &&
      filament.name === name &&
      (requiresSpoolmanIdentity
        ? filament.origin === "spoolman" &&
          filament.resource_kind === "filament" &&
          filament.external_id === spoolmanId
        : filament.origin === "manual" && filament.resource_kind === "manual"),
  );
  return matches.length === 1 ? matches[0].id : null;
}

function normalizePartSnapshot(
  name: string | null | undefined,
  category: string | null | undefined,
  unit: string | null | undefined,
): string | null {
  if (!name?.trim() || !unit?.trim()) return null;
  return JSON.stringify([
    name.trim().toLocaleLowerCase(),
    category?.trim().toLocaleLowerCase() ?? "",
    unit.trim().toLocaleLowerCase(),
  ]);
}

export function validatePrintventoryPartAssignments(
  imported: readonly PrintventoryPartImportRecord[],
  assignments: readonly PrintventoryPartAssignment[],
): { valid: boolean; reason?: string } {
  if (assignments.length !== imported.length) {
    return {
      valid: false,
      reason: "Choose create, skip, or a local part for every imported part.",
    };
  }
  const importedRefs = new Set(imported.map((part) => part.externalRef));
  const seenRefs = new Set<string>();
  const seenTargets = new Set<string>();
  for (const assignment of assignments) {
    if (!importedRefs.has(assignment.externalRef) || seenRefs.has(assignment.externalRef)) {
      return { valid: false, reason: "Each imported part must have one reviewed choice." };
    }
    seenRefs.add(assignment.externalRef);
    if (!assignment.target || assignment.target === "skip" || assignment.target === "create") {
      if (!assignment.target) {
        return { valid: false, reason: "Choose a reviewed action for every imported part." };
      }
      continue;
    }
    if (!assignment.target.startsWith("local:") || assignment.target.length <= "local:".length) {
      return { valid: false, reason: "An imported part has an invalid local stock mapping." };
    }
    if (seenTargets.has(assignment.target)) {
      return { valid: false, reason: "Each local stock part can be mapped only once." };
    }
    seenTargets.add(assignment.target);
  }
  return { valid: true };
}

export async function applyPrintventoryPartAssignments(
  imported: readonly PrintventoryPartImportRecord[],
  assignments: readonly PrintventoryPartAssignment[],
  localParts: readonly PartStock[],
  api: PrintventoryPartImportApi,
): Promise<PrintventoryPartImportResult> {
  const check = validatePrintventoryPartAssignments(imported, assignments);
  if (!check.valid) throw new Error(check.reason ?? "Part assignments are invalid.");
  const assignmentByRef = new Map(
    assignments.map((assignment) => [assignment.externalRef, assignment]),
  );
  const partIdByExternalRef = new Map<string, string>();
  const rows: PrintventoryPartImportResult["rows"] = [];
  const warnings: string[] = [];
  let createdCount = 0;
  let updatedCount = 0;
  let skippedCount = 0;
  for (const part of imported) {
    const assignment = assignmentByRef.get(part.externalRef)!;
    if (assignment.target === "skip") {
      skippedCount += 1;
      rows.push({ externalRef: part.externalRef, status: "skipped", localPartId: null });
      continue;
    }
    try {
      if (assignment.target === "create") {
        const created = await api.createPartStock(part.input);
        partIdByExternalRef.set(part.externalRef, created.id);
        createdCount += 1;
        rows.push({ externalRef: part.externalRef, status: "created", localPartId: created.id });
        continue;
      }
      const localPartId = assignment.target.slice("local:".length);
      const current = localParts.find((candidate) => candidate.id === localPartId);
      if (!current) throw new Error("The selected local stock part is no longer available.");
      const matches =
        current.name === part.input.name &&
        current.category === part.input.category &&
        current.quantity === part.input.quantity &&
        current.unit === part.input.unit &&
        current.notes === part.input.notes &&
        current.lowStock === part.input.lowStock;
      if (matches) {
        partIdByExternalRef.set(part.externalRef, current.id);
        rows.push({ externalRef: part.externalRef, status: "unchanged", localPartId: current.id });
        continue;
      }
      const updated = await api.updatePartStock(current.id, current.metadataVersion, part.input);
      partIdByExternalRef.set(part.externalRef, updated.id);
      updatedCount += 1;
      rows.push({ externalRef: part.externalRef, status: "updated", localPartId: updated.id });
    } catch (reason) {
      const warning = `Part “${part.input.name}” was not imported: ${String((reason as { message?: unknown })?.message ?? reason)}`;
      warnings.push(warning);
      rows.push({
        externalRef: part.externalRef,
        status: "failed",
        localPartId: null,
        warning,
      });
    }
  }
  return { createdCount, updatedCount, skippedCount, partIdByExternalRef, rows, warnings };
}

const PRINTVENTORY_ORPHAN_PART_PREFIX = "printventory_orphan_";

function printventoryOrphanPartId(seed: string): string {
  if (
    seed.startsWith(PRINTVENTORY_ORPHAN_PART_PREFIX) &&
    seed.length <= 128 &&
    !hasImportControlCharacters(seed)
  ) {
    return seed;
  }
  let hash = 14_695_981_039_346_656_037n;
  const mask = (1n << 64n) - 1n;
  for (const byte of new TextEncoder().encode(seed)) {
    hash = ((hash ^ BigInt(byte)) * 1_099_511_628_211n) & mask;
  }
  return PRINTVENTORY_ORPHAN_PART_PREFIX + hash.toString(16).padStart(16, "0");
}

function mapPrintventoryEventPartUsage(
  value: unknown,
  eventIndex: number,
  copies: number,
  importedParts: readonly PrintventoryPartImportRecord[],
  partIdByExternalRef: ReadonlyMap<string, string>,
  warnings: string[],
): NonNullable<PrintAttemptInput["parts"]> | undefined {
  if (!Array.isArray(value)) {
    warnings.push(`Print event ${eventIndex + 1} part usage was skipped because it is not a list.`);
    return undefined;
  }
  if (value.length > MAX_PRINTVENTORY_EVENT_PARTS) {
    warnings.push(
      `Print event ${eventIndex + 1} part usage exceeds the ${MAX_PRINTVENTORY_EVENT_PARTS}-part import limit.`,
    );
    return undefined;
  }
  const usageById = new Map<
    string,
    { quantity: number; snapshot?: { name: string; category: string | null; unit: string } }
  >();
  for (const [partIndex, rawPart] of value.entries()) {
    const part = asRecord(rawPart);
    if (!part) {
      warnings.push(
        `Print event ${eventIndex + 1} part ${partIndex + 1} was skipped because it is invalid.`,
      );
      continue;
    }
    const refValue =
      readImportField(part, "gogglelab_part_id", "gogglelabPartId") ??
      readImportField(part, "part_id", "partId") ??
      part.id;
    let sourceId: string | null = null;
    if (refValue !== undefined && refValue !== null) {
      try {
        sourceId = validateExternalRef(refValue, `print event ${eventIndex + 1} part ID`);
      } catch {
        warnings.push(
          `Print event ${eventIndex + 1} part ${partIndex + 1} has an invalid reference.`,
        );
        continue;
      }
    }
    let snapshotName: string | undefined;
    let snapshotCategory: string | null = null;
    let snapshotUnit: string | undefined;
    try {
      const normalizedName = validateImportString(
        part.name,
        `print event ${eventIndex + 1} part ${partIndex + 1} name`,
        128,
      );
      const normalizedCategory = validateImportString(
        part.category,
        `print event ${eventIndex + 1} part ${partIndex + 1} category`,
        128,
      );
      const normalizedUnit = validateImportString(
        part.unit,
        `print event ${eventIndex + 1} part ${partIndex + 1} unit`,
        128,
      );
      snapshotName = typeof normalizedName === "string" ? normalizedName : undefined;
      snapshotCategory = typeof normalizedCategory === "string" ? normalizedCategory : null;
      snapshotUnit = typeof normalizedUnit === "string" ? normalizedUnit : undefined;
    } catch {
      warnings.push(
        `Print event ${eventIndex + 1} part ${partIndex + 1} was skipped because its snapshot text is invalid.`,
      );
      continue;
    }
    const explicitPerCopy = readImportField(part, "quantity_per_copy", "quantityPerCopy");
    let perCopy: number;
    if (explicitPerCopy !== undefined) {
      if (
        typeof explicitPerCopy !== "number" ||
        !Number.isSafeInteger(explicitPerCopy) ||
        explicitPerCopy < 1 ||
        explicitPerCopy > 9_999
      ) {
        warnings.push(
          `Print event ${eventIndex + 1} part ${partIndex + 1} was skipped because its per-copy quantity is invalid.`,
        );
        continue;
      }
      perCopy = explicitPerCopy;
    } else {
      const total = readImportField(part, "quantity", "totalQuantity");
      if (
        typeof total !== "number" ||
        !Number.isSafeInteger(total) ||
        total < 1 ||
        total > MAX_PRINTVENTORY_EVENT_PART_TOTAL ||
        total % copies !== 0
      ) {
        warnings.push(
          `Print event ${eventIndex + 1} part ${partIndex + 1} was skipped because total use cannot be converted to a whole per-copy amount.`,
        );
        continue;
      }
      perCopy = total / copies;
    }
    if (perCopy < 1 || perCopy > 9_999) {
      warnings.push(
        `Print event ${eventIndex + 1} part ${partIndex + 1} exceeds the per-copy quantity limit.`,
      );
      continue;
    }

    const sourceIsOrphan = Boolean(sourceId?.startsWith(PRINTVENTORY_ORPHAN_PART_PREFIX));
    let candidates = sourceId
      ? importedParts.filter((candidate) => candidate.sourceId === sourceId)
      : [];
    if (!sourceId) {
      const snapshotKey = normalizePartSnapshot(snapshotName, snapshotCategory, snapshotUnit);
      if (snapshotKey !== null) {
        candidates = importedParts.filter(
          (candidate) =>
            normalizePartSnapshot(
              candidate.input.name,
              candidate.input.category,
              candidate.input.unit,
            ) === snapshotKey,
        );
      }
    }

    const matchedPart = candidates.length === 1 ? candidates[0] : null;
    const localId = matchedPart ? (partIdByExternalRef.get(matchedPart.externalRef) ?? null) : null;
    const needsOrphan =
      sourceIsOrphan || candidates.length !== 1 || !matchedPart || localId === null;
    const id = needsOrphan
      ? sourceIsOrphan && sourceId
        ? printventoryOrphanPartId(sourceId)
        : printventoryOrphanPartId(
            sourceId !== null
              ? `source:${sourceId}`
              : matchedPart
                ? `row:${matchedPart.externalRef}`
                : `event:${eventIndex + 1}:${partIndex + 1}:${JSON.stringify([
                    snapshotName ?? null,
                    snapshotCategory,
                    snapshotUnit ?? null,
                  ])}`,
          )
      : localId!;
    const snapshot = needsOrphan
      ? {
          name: snapshotName ?? (sourceId ? `Imported part ${sourceId}` : "Imported part"),
          category: snapshotCategory,
          unit: snapshotUnit ?? "unit",
        }
      : undefined;
    if (needsOrphan) {
      const reason = sourceIsOrphan
        ? "already marked as an unlinked historical part"
        : sourceId !== null && candidates.length === 0
          ? "its source ID has no top-level stock row"
          : candidates.length > 1
            ? "its stock row match is ambiguous"
            : matchedPart && localId === null
              ? "its top-level stock row was skipped or failed review"
              : "no unique reviewed stock row matches it";
      warnings.push(
        `Print event ${eventIndex + 1} part “${snapshot?.name ?? "Imported part"}”: historical usage is preserved but is not linked to stock because ${reason}.`,
      );
    }
    const previous = usageById.get(id);
    const aggregate = (previous?.quantity ?? 0) + perCopy;
    if (aggregate > 9_999) {
      warnings.push(
        `Print event ${eventIndex + 1} part ${partIndex + 1} exceeds the per-copy quantity limit after duplicate references were combined.`,
      );
      continue;
    }
    usageById.set(id, {
      quantity: aggregate,
      snapshot: previous?.snapshot ?? snapshot,
    });
  }
  return usageById.size > 0
    ? [...usageById].map(([id, usage]) => ({
        id,
        quantity: usage.quantity,
        ...(usage.snapshot ? { snapshot: usage.snapshot } : {}),
      }))
    : undefined;
}

export function buildPrintventoryFieldMapping(
  entry: PrintventoryModelRecord,
  targetModelId: string,
  inventory: LibraryModel[],
  tags: LibraryTag[],
  filaments: Filament[],
  importedParts: readonly PrintventoryPartImportRecord[] = [],
  partIdByExternalRef: ReadonlyMap<string, string> = new Map(),
): PrintventoryFieldMapping {
  const patch: MetadataPatch = {};
  const mappedFields: string[] = [];
  const warnings: string[] = [];

  const textFields = [
    ["designer", "designer", "Designer"],
    ["source", "source_url", "Source URL"],
    ["notes", "notes", "Notes"],
    ["license", "model_license", "License"],
  ] as const;
  for (const [sourceField, targetField, label] of textFields) {
    if (!hasOwn(entry, sourceField)) continue;
    const value = entry[sourceField];
    if (value === null || (typeof value === "string" && value.trim().length === 0)) {
      Object.assign(patch, { [targetField]: "" });
      mappedFields.push(label);
      continue;
    }
    if (typeof value !== "string") {
      warnings.push(label + " was skipped because the imported value is not text.");
      continue;
    }
    if (sourceField === "source" && !isSafeSourceUrl(value) && !isSafeSourceOrigin(value)) {
      warnings.push(
        "Source was skipped because it is not a credential-free HTTP or HTTPS URL or plain origin label.",
      );
      continue;
    }
    Object.assign(patch, { [targetField]: value });
    mappedFields.push(label);
  }

  const statusKey = hasOwn(entry, "print_status")
    ? "print_status"
    : hasOwn(entry, "printStatus")
      ? "printStatus"
      : null;
  const validPrintStatuses = new Set([
    "unprinted",
    "want",
    "queued",
    "printing",
    "printed",
    "failed",
  ]);
  if (statusKey !== null) {
    const value = entry[statusKey];
    if (typeof value === "string" && validPrintStatuses.has(value)) {
      patch.print_status = value as MetadataPatch["print_status"];
      mappedFields.push("Print status");
    } else {
      warnings.push("Print status was skipped because it is not a supported status.");
    }
  } else if (hasOwn(entry, "printed")) {
    if (typeof entry.printed === "boolean") {
      patch.print_status = entry.printed ? "printed" : "unprinted";
      mappedFields.push("Printed status");
    } else {
      warnings.push("Printed status was skipped because the imported value is not a boolean.");
    }
  }

  if (hasOwn(entry, "rating")) {
    if (
      typeof entry.rating === "number" &&
      Number.isInteger(entry.rating) &&
      entry.rating >= 0 &&
      entry.rating <= 5
    ) {
      patch.rating = entry.rating;
      mappedFields.push("Rating");
    } else {
      warnings.push("Rating was skipped because it must be an integer from 0 to 5.");
    }
  }
  if (hasOwn(entry, "favorite")) {
    if (typeof entry.favorite === "boolean") {
      patch.favorite = entry.favorite;
      mappedFields.push("Favorite");
    } else {
      warnings.push("Favorite was skipped because the imported value is not a boolean.");
    }
  }

  if (hasOwn(entry, "parentModel") || hasOwn(entry, "gogglelabParentModelId")) {
    const hasStableParentId = hasOwn(entry, "gogglelabParentModelId");
    const stableParentId = entry.gogglelabParentModelId;
    const parentName = entry.parentModel;
    const explicitlyCleared =
      hasStableParentId &&
      (stableParentId === null || stableParentId === "") &&
      (!hasOwn(entry, "parentModel") || parentName === null || parentName === "");
    if (explicitlyCleared || (!hasStableParentId && (parentName === null || parentName === ""))) {
      patch.parent_model_id = "";
      mappedFields.push("Parent model");
    } else if (hasStableParentId && stableParentId === targetModelId) {
      warnings.push("Parent model cannot reference the selected model itself and was skipped.");
    } else {
      let matches: LibraryModel[] = [];
      let warningValue: string | null = null;
      if (hasStableParentId) {
        const stableMatches =
          typeof stableParentId === "string" && stableParentId.length > 0
            ? inventory.filter((model) => model.id !== targetModelId && model.id === stableParentId)
            : [];
        if (stableMatches.length === 1) {
          matches = stableMatches;
          warningValue = stableParentId as string;
        } else if (typeof parentName === "string" && parentName.length > 0) {
          matches = inventory.filter(
            (model) => model.id !== targetModelId && model.display_name === parentName,
          );
          warningValue = parentName;
        } else {
          matches = stableMatches;
          warningValue = typeof stableParentId === "string" ? stableParentId : null;
        }
      } else if (typeof parentName === "string") {
        matches = inventory.filter(
          (model) =>
            model.id !== targetModelId &&
            (model.id === parentName || model.display_name === parentName),
        );
        warningValue = parentName;
      }

      if (matches.length === 1) {
        patch.parent_model_id = matches[0].id;
        mappedFields.push("Parent model");
      } else if (warningValue !== null) {
        warnings.push(
          matches.length > 1
            ? "Parent model “" + warningValue + "” is ambiguous and was skipped."
            : "Parent model “" +
                warningValue +
                "” does not uniquely match a local Library model and was skipped.",
        );
      } else {
        warnings.push(
          "Parent model was skipped because the imported value is not a recognized reference.",
        );
      }
    }
  }

  let tagNames: string[] | undefined;
  let tagAssignments: PrintventoryTagAssignment[] | undefined;
  const tagKey = hasOwn(entry, "gogglelabTags")
    ? "gogglelabTags"
    : hasOwn(entry, "tags")
      ? "tags"
      : null;
  if (tagKey !== null) {
    const rawTags = entry[tagKey];
    if (Array.isArray(rawTags)) {
      const parsed = rawTags.map((value) => {
        const name = explicitTagName(value);
        if (name === null) return null;
        const record = asRecord(value);
        let category: string | null | undefined;
        if (record && hasOwn(record, "category")) {
          const rawCategory = record.category;
          if (rawCategory === null) {
            category = null;
          } else if (typeof rawCategory === "string") {
            const normalized = rawCategory.trim();
            const safe =
              new TextEncoder().encode(normalized).length <= 120 &&
              !Array.from(normalized).some((character) => {
                const codePoint = character.codePointAt(0) ?? 0;
                return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
              });
            if (safe) category = normalized || null;
            else
              warnings.push(
                "A tag category was skipped because it is too long or contains control characters.",
              );
          } else {
            warnings.push("A tag category was skipped because the imported value is not text.");
          }
        }
        if (record && Object.keys(record).some((key) => key !== "name" && key !== "category")) {
          warnings.push(
            "Foreign tag properties such as IDs are ignored; local tags are matched by name.",
          );
        }
        return category === undefined ? { name } : { name, category };
      });
      if (parsed.some((value) => value === null)) {
        warnings.push("Tags were skipped because one or more values have no explicit name.");
      } else {
        const byName = new Map<string, PrintventoryTagAssignment>();
        let conflictingCategory = false;
        for (const value of parsed as Array<PrintventoryTagAssignment>) {
          const normalizedName = normalizeTagName(value.name);
          const previous = byName.get(normalizedName);
          if (
            previous &&
            hasOwn(previous, "category") &&
            hasOwn(value, "category") &&
            previous.category !== value.category
          ) {
            conflictingCategory = true;
            break;
          }
          if (previous && hasOwn(previous, "category") && !hasOwn(value, "category")) {
            continue;
          }
          byName.set(normalizedName, value);
        }
        const uniqueAssignments = [...byName.values()];
        const names = uniqueAssignments.map((assignment) => assignment.name);
        const invalidName = names.find((name) => !isSafeLocalTagName(name));
        const ambiguousName = names.find(
          (name) =>
            tags.filter((tag) => normalizeTagName(tag.name) === normalizeTagName(name)).length > 1,
        );
        if (conflictingCategory) {
          warnings.push(
            "The same tag has conflicting categories in the imported row; the tag list was skipped.",
          );
        } else if (invalidName) {
          warnings.push(
            "Tag “" + invalidName + "” is not a valid local tag name; the tag list was skipped.",
          );
        } else if (uniqueAssignments.length > 200) {
          warnings.push("The tag list exceeds the local Library limit of 200 and was skipped.");
        } else if (ambiguousName) {
          warnings.push(
            "Tag “" + ambiguousName + "” matches multiple local tags; the tag list was skipped.",
          );
        } else {
          tagAssignments = uniqueAssignments;
          tagNames = uniqueAssignments.map((assignment) => assignment.name);
          mappedFields.push("Tags");
          if (uniqueAssignments.some((assignment) => hasOwn(assignment, "category"))) {
            mappedFields.push("Tag categories");
          }
          for (const assignment of uniqueAssignments) {
            if (!hasOwn(assignment, "category")) continue;
            const existing = tags.find(
              (tag) => normalizeTagName(tag.name) === normalizeTagName(assignment.name),
            );
            if (existing && (existing.category ?? null) !== assignment.category) {
              warnings.push(
                "Category for existing tag “" +
                  assignment.name +
                  "” differs; its existing global category will be retained.",
              );
            }
          }
        }
      }
    } else {
      warnings.push("Tags were skipped because the imported value is not a collection.");
    }
  }

  let filamentIds: string[] | undefined;
  if (hasOwn(entry, "filaments")) {
    if (Array.isArray(entry.filaments)) {
      const parsedIds = entry.filaments.map((value) => mapFilamentReference(value, filaments));
      if (parsedIds.every((id): id is string => id !== null)) {
        filamentIds = [...new Set(parsedIds)];
        mappedFields.push("Filaments");
      } else {
        const hasNumericId = entry.filaments.some((value) => typeof value === "number");
        warnings.push(
          hasNumericId
            ? "Filaments were skipped because foreign numeric IDs cannot identify local filament records."
            : "Filaments were skipped because every item must uniquely match a local filament by stable fields.",
        );
      }
    } else {
      warnings.push("Filaments were skipped because the imported value is not a collection.");
    }
  }

  let printAttempts: ImportedPrintAttempt[] | undefined;
  const historyKey = hasOwn(entry, "printEvents")
    ? "printEvents"
    : hasOwn(entry, "print_events")
      ? "print_events"
      : null;
  if (historyKey !== null) {
    const rawEvents = entry[historyKey];
    if (!Array.isArray(rawEvents)) {
      warnings.push("Print history was skipped because the imported value is not a collection.");
    } else if (rawEvents.length > 1_000) {
      warnings.push("Print history exceeds the local import limit of 1,000 events per model.");
    } else {
      printAttempts = [];
      for (const [index, rawEvent] of rawEvents.entries()) {
        const event = asRecord(rawEvent);
        if (!event) {
          warnings.push("Print event " + (index + 1) + " was skipped because it is not an object.");
          continue;
        }
        const printedAt = normalizeImportedPrintTimestamp(event.printed_at ?? event.printedAt);
        const createdAtKey = hasOwn(event, "created_at")
          ? "created_at"
          : hasOwn(event, "createdAt")
            ? "createdAt"
            : null;
        const createdAt =
          createdAtKey === null
            ? undefined
            : normalizeImportedPrintTimestamp(event[createdAtKey], false);
        const rawOutcome = event.outcome ?? event.status;
        const outcome =
          rawOutcome === "printed" || rawOutcome === "failed" || rawOutcome === "cancelled"
            ? rawOutcome
            : null;
        const quantity = event.quantity === undefined ? 1 : event.quantity;
        const notes = event.notes === undefined ? "" : event.notes;
        if (createdAtKey !== null && createdAt === null) {
          warnings.push(
            "Print event " +
              (index + 1) +
              " was skipped because its creation timestamp is invalid.",
          );
          continue;
        }
        if (
          !printedAt ||
          !outcome ||
          typeof quantity !== "number" ||
          !Number.isInteger(quantity) ||
          quantity < 1 ||
          quantity > 10_000 ||
          typeof notes !== "string" ||
          new TextEncoder().encode(notes).length > 20_000
        ) {
          warnings.push(
            "Print event " +
              (index + 1) +
              " was skipped because its date, outcome, quantity, or notes are invalid.",
          );
          continue;
        }
        const rawFilaments = event.filaments ?? event.filament_ids ?? event.filamentIds ?? [];
        if (!Array.isArray(rawFilaments)) {
          warnings.push(
            "Print event " + (index + 1) + " was skipped because its filaments are invalid.",
          );
          continue;
        }
        const filamentIds = rawFilaments.map((filament) =>
          typeof filament === "string"
            ? filaments.some((candidate) => candidate.id === filament)
              ? filament
              : null
            : mapFilamentReference(filament, filaments, true),
        );
        if (!filamentIds.every((id): id is string => id !== null) || filamentIds.length > 5) {
          warnings.push(
            "Print event " +
              (index + 1) +
              " was skipped because each filament must match one local record.",
          );
          continue;
        }
        const partUsage = hasOwn(event, "parts")
          ? mapPrintventoryEventPartUsage(
              event.parts,
              index,
              quantity,
              importedParts,
              partIdByExternalRef,
              warnings,
            )
          : undefined;
        const printerRefValue = event.gogglelab_printer_id ?? event.printer_id;
        let printerExternalRef: string | undefined;
        if (printerRefValue !== undefined && printerRefValue !== null) {
          try {
            printerExternalRef = validateExternalRef(
              printerRefValue,
              `print event ${index + 1} printer reference`,
            );
          } catch {
            warnings.push(
              `Print event ${index + 1} printer reference was skipped because it is invalid.`,
            );
          }
        }
        const printerSnapshot = readPrintEventPrinterSnapshot(event, index, warnings);
        const printAttempt: ImportedPrintAttempt = {
          input: {
            printed_at: printedAt,
            outcome,
            quantity,
            notes,
            filament_ids: filamentIds,
            ...(partUsage !== undefined ? { parts: partUsage } : {}),
          },
        };
        if (partUsage !== undefined && !mappedFields.includes("Parts")) mappedFields.push("Parts");
        if (typeof createdAt === "string") printAttempt.createdAt = createdAt;
        if (printerExternalRef !== undefined) {
          printAttempt.printerExternalRef = printerExternalRef;
        }
        if (printerSnapshot !== undefined) printAttempt.printerSnapshot = printerSnapshot;
        printAttempts.push(printAttempt);
      }
      if (printAttempts.length > 0) mappedFields.push("Print history");
    }
  } else if (hasOwn(entry, "print_count") || hasOwn(entry, "last_printed_at")) {
    warnings.push(
      "Print count and last-printed date are derived from printEvents and were not imported separately.",
    );
  }

  // Paths and foreign IDs are hints only; file I/O remains restricted to local catalog APIs.
  return { patch, tagNames, tagAssignments, filamentIds, printAttempts, mappedFields, warnings };
}

export async function applyPrintventoryAssignments(
  entries: PrintventoryModelRecord[],
  assignments: readonly PrintventoryAssignment[],
  inventory: LibraryModel[],
  tags: LibraryTag[],
  filaments: Filament[],
  api: PrintventoryImportApi,
  printerIdByExternalRef: ReadonlyMap<string, string> = new Map(),
  importedParts: readonly PrintventoryPartImportRecord[] = [],
  partIdByExternalRef: ReadonlyMap<string, string> = new Map(),
): Promise<PrintventoryImportResult> {
  const assignmentCheck = validatePrintventoryAssignments(assignments);
  if (!assignmentCheck.valid) {
    throw new Error("A Library model cannot be assigned more than once.");
  }
  const assignmentByEntry = new Map<number, string>();
  for (const assignment of assignments) {
    if (
      !Number.isInteger(assignment.entryIndex) ||
      assignment.entryIndex < 0 ||
      assignment.entryIndex >= entries.length
    ) {
      throw new Error("An import row assignment is out of range.");
    }
    if (assignmentByEntry.has(assignment.entryIndex)) {
      throw new Error("An import row cannot be assigned more than once.");
    }
    if (assignment.targetModelId)
      assignmentByEntry.set(assignment.entryIndex, assignment.targetModelId);
  }

  const rows: PrintventoryImportRowResult[] = [];
  const availableTags = [...tags];
  let updated = 0;
  for (let entryIndex = 0; entryIndex < entries.length; entryIndex += 1) {
    const targetModelId = assignmentByEntry.get(entryIndex) ?? null;
    if (!targetModelId) {
      rows.push({
        entryIndex,
        targetModelId: null,
        status: "skipped",
        mappedFields: [],
        warnings: ["No Library target was selected; this entry was skipped."],
      });
      continue;
    }
    const model = inventory.find((candidate) => candidate.id === targetModelId);
    if (!model || !isLocalImportModel(model)) {
      rows.push({
        entryIndex,
        targetModelId,
        status: "failed",
        mappedFields: [],
        warnings: ["The selected Library model is no longer available for metadata import."],
      });
      continue;
    }

    const mapping = buildPrintventoryFieldMapping(
      entries[entryIndex],
      targetModelId,
      inventory,
      availableTags,
      filaments,
      importedParts,
      partIdByExternalRef,
    );
    const patch: MetadataPatch = { ...mapping.patch };
    const rowWarnings = [...mapping.warnings];
    const mappedFields = [...mapping.mappedFields];
    const appliedFields: string[] = [];
    const createdTagNames: string[] = [];
    let currentVersion = model.metadata_version;
    let didWrite = false;
    try {
      if (mapping.tagNames !== undefined) {
        const tagIds: string[] = [];
        const assignments =
          mapping.tagAssignments ??
          mapping.tagNames.map((name) => ({ name }) as PrintventoryTagAssignment);
        let tagsSafe = true;
        for (const assignment of assignments) {
          const matches = availableTags.filter(
            (tag) => normalizeTagName(tag.name) === normalizeTagName(assignment.name),
          );
          if (matches.length > 1) {
            rowWarnings.push(
              "Tag “" + assignment.name + "” matches multiple local tags and was skipped.",
            );
            tagsSafe = false;
            break;
          }
          let tag = matches[0];
          if (!tag) {
            tag = await api.createTag(assignment.name, assignment.category ?? null);
            availableTags.push(tag);
            createdTagNames.push(assignment.name);
          }
          tagIds.push(tag.id);
        }
        if (tagsSafe) {
          patch.tags = tagIds;
          patch.tag_mode = "replace";
        } else {
          for (const field of ["Tags", "Tag categories"]) {
            const fieldIndex = mappedFields.indexOf(field);
            if (fieldIndex >= 0) mappedFields.splice(fieldIndex, 1);
          }
        }
      }

      if (Object.keys(patch).length > 0) {
        const updatedModel = await api.updateMetadata(model.id, currentVersion, patch);
        currentVersion = updatedModel.metadata_version;
        didWrite = true;
        appliedFields.push(
          ...mappedFields.filter((field) => field !== "Filaments" && field !== "Print history"),
        );
      }
      if (mapping.filamentIds !== undefined) {
        const updatedModel = await api.setModelFilaments(
          model.id,
          currentVersion,
          mapping.filamentIds,
        );
        currentVersion = updatedModel.metadata_version;
        didWrite = true;
        appliedFields.push("Filaments");
      }
      if (mapping.printAttempts && mapping.printAttempts.length > 0) {
        const priorAttempts = await api.listPrintAttempts(model.id);
        const priorCounts = new Map<string, number>();
        for (const attempt of priorAttempts) {
          const identity = printAttemptIdentity(
            attempt,
            attempt.printer_id ?? null,
            attempt.printer_name ?? null,
          );
          priorCounts.set(identity, (priorCounts.get(identity) ?? 0) + 1);
        }
        const matchedCounts = new Map<string, number>();
        let addedHistory = 0;
        let skippedHistory = 0;
        const attemptsToImport: PrintAttemptImportItem[] = [];
        for (const importedAttempt of mapping.printAttempts) {
          const mappedPrinterId = importedAttempt.printerExternalRef
            ? (printerIdByExternalRef.get(importedAttempt.printerExternalRef) ?? null)
            : null;
          const safePrinterId =
            typeof mappedPrinterId === "string" && mappedPrinterId.trim().length > 0
              ? mappedPrinterId
              : null;
          const printerNameSnapshot = importedAttempt.printerSnapshot?.printer_nickname ?? null;
          const identity = printAttemptIdentity(
            importedAttempt.input,
            safePrinterId,
            printerNameSnapshot,
          );
          const existingCount = priorCounts.get(identity) ?? 0;
          const matchedCount = matchedCounts.get(identity) ?? 0;
          if (matchedCount < existingCount) {
            matchedCounts.set(identity, matchedCount + 1);
            skippedHistory += 1;
            continue;
          }
          attemptsToImport.push({
            input: importedAttempt.input,
            createdAt: importedAttempt.createdAt ?? null,
            printerId: safePrinterId,
            printerName: printerNameSnapshot,
          });
        }
        if (attemptsToImport.length > 0) {
          if (!api.importPrintAttempts) {
            throw new Error("Atomic print history import is unavailable.");
          }
          const importResult = await api.importPrintAttempts(model.id, attemptsToImport);
          currentVersion = importResult.metadataVersion;
          addedHistory = importResult.addedCount;
          skippedHistory += importResult.skippedCount;
          didWrite = didWrite || addedHistory > 0;
          if (addedHistory > 0 && !appliedFields.includes("Print history")) {
            appliedFields.push("Print history");
          }
          if (
            addedHistory > 0 &&
            mapping.printAttempts.some((attempt) => (attempt.input.parts?.length ?? 0) > 0) &&
            !appliedFields.includes("Parts")
          ) {
            appliedFields.push("Parts");
          }
        }
        if (skippedHistory > 0 && !appliedFields.includes("Print history")) {
          appliedFields.push("Print history");
        }
        if ((addedHistory > 0 || skippedHistory > 0) && mapping.patch.print_status !== undefined) {
          const updatedModel = await api.updateMetadata(model.id, currentVersion, {
            print_status: mapping.patch.print_status,
          });
          currentVersion = updatedModel.metadata_version;
        }
        if (skippedHistory > 0) {
          rowWarnings.push(
            skippedHistory +
              " matching print event(s) already existed and were skipped to avoid duplicates.",
          );
        }
      }
      if (!didWrite && rowWarnings.length === 0) {
        rowWarnings.push("This entry has no supported fields to apply.");
      }
      if (didWrite) updated += 1;
      rows.push({
        entryIndex,
        targetModelId,
        status: didWrite ? (rowWarnings.length > 0 ? "partial" : "updated") : "skipped",
        mappedFields: didWrite ? appliedFields : [],
        warnings: rowWarnings,
      });
    } catch (reason) {
      if (createdTagNames.length > 0) {
        rowWarnings.push(
          `Local tags were created but may not be attached: ${createdTagNames.join(", ")}.`,
        );
      }
      rowWarnings.push(
        didWrite
          ? `Some metadata was saved, but a later update failed: ${String(reason)}`
          : `No metadata was saved: ${String(reason)}`,
      );
      if (didWrite) updated += 1;
      rows.push({
        entryIndex,
        targetModelId,
        status: didWrite ? "partial" : "failed",
        mappedFields: didWrite ? appliedFields : [],
        warnings: rowWarnings,
      });
    }
  }
  return { updated, rows };
}
