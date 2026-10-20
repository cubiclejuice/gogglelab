import type {
  LibraryPrinter,
  PrinterConnectionTestResult,
  PrinterDiscoverySuggestion as IpcPrinterDiscoverySuggestion,
  PrinterInput as IpcPrinterInput,
  PrinterReminder as IpcPrinterReminder,
  PrinterReminderCompletion,
  PrinterReminderInput as IpcPrinterReminderInput,
  PrinterMaintenanceLog as IpcPrinterMaintenanceLog,
  PrinterMaintenanceInput as IpcPrinterMaintenanceInput,
} from "./ipc";

export type PrinterConnection = { kind: "local" } | { kind: "network"; address: string };

export type ConnectionTestStatus = PrinterConnectionTestResult;
export type MaintenanceReminderStatus = "open" | "paused" | "completed";

export interface ManagedPrinter {
  id: string;
  metadataVersion: number;
  name: string;
  model: string;
  bedMm: [number, number, number];
  connection: PrinterConnection;
  endpointApproved: boolean;
  connectionStatus: LibraryPrinter["connectionStatus"];
  jobStatus: LibraryPrinter["jobStatus"];
  lastCheckedAt: string | null;
  connectionErrorCode: string | null;
  make: string | null;
  printerType: string | null;
  firmware: string | null;
  webUrl: string | null;
  notes: string;
  sourceKind: LibraryPrinter["sourceKind"];
  printCount: number;
  lastPrintedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface SlicerPrinterDetection {
  id: string;
  name: string;
  model: string;
  bedMm: [number, number, number];
  source: string;
}

export interface PrinterDiscoverySuggestion {
  key: string;
  name: string;
  model: string;
  bedMm: [number, number, number];
  connection: PrinterConnection;
  source: string;
  printerType: string;
}

export interface PrinterDraft {
  id: string | null;
  metadataVersion: number | null;
  name: string;
  make: string | null;
  model: string;
  printerType: string | null;
  firmware: string | null;
  webUrl: string | null;
  notes: string;
  bedMm: [number, number, number];
  connection: PrinterConnection;
  endpointApproved: boolean;
}

export interface MaintenanceLog {
  id: string;
  metadataVersion: number;
  printerId: string;
  task: string;
  performedAt: string;
  notes: string;
}

export interface MaintenanceLogDraft {
  id: string | null;
  metadataVersion: number | null;
  printerId: string;
  task: string;
  performedAt: string;
  notes: string;
}

export interface MaintenanceReminder {
  id: string;
  printerId: string;
  task: string;
  dueAt: string;
  repeatEveryDays: number | null;
  status: MaintenanceReminderStatus;
  completedAt: string | null;
  metadataVersion: number;
  notes: string;
  printerNickname: string;
  lastCompletedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface MaintenanceReminderDraft {
  id: string | null;
  printerId: string;
  task: string;
  dueAt: string;
  repeatEveryDays: number | null;
  metadataVersion: number | null;
  notes: string;
}

export function mapLibraryPrinter(printer: LibraryPrinter): ManagedPrinter {
  const safeHost = printer.host ?? printer.webUrl ?? "";
  const connection: PrinterConnection = safeHost
    ? { kind: "network", address: redactAddressCredentials(safeHost) }
    : { kind: "local" };
  return {
    id: printer.id,
    metadataVersion: printer.metadataVersion,
    name: printer.nickname,
    make: printer.make,
    model: printer.model ?? "",
    printerType: printer.printerType,
    firmware: printer.firmware,
    webUrl: printer.webUrl ? redactAddressCredentials(printer.webUrl) : null,
    notes: printer.notes,
    bedMm: printer.bedMm ? [...printer.bedMm] : [256, 256, 256],
    connection,
    endpointApproved: printer.endpointApproved,
    connectionStatus: printer.connectionStatus,
    jobStatus: printer.jobStatus,
    lastCheckedAt: printer.lastCheckedAt,
    connectionErrorCode: printer.connectionErrorCode,
    sourceKind: printer.sourceKind,
    printCount: printer.printCount,
    lastPrintedAt: printer.lastPrintedAt,
    createdAt: printer.createdAt,
    updatedAt: printer.updatedAt,
  };
}

export function mapPrinterDraftToInput(draft: PrinterDraft): IpcPrinterInput {
  return {
    nickname: draft.name.trim(),
    make: cleanOptional(draft.make),
    model: cleanOptional(draft.model),
    printerType: cleanOptional(draft.printerType),
    firmware: cleanOptional(draft.firmware),
    host:
      draft.connection.kind === "network"
        ? redactAddressCredentials(draft.connection.address).trim()
        : null,
    webUrl:
      draft.connection.kind === "network"
        ? redactAddressCredentials(draft.connection.address).trim()
        : null,
    bedMm: [...draft.bedMm],
    notes: draft.notes.slice(0, MAX_NOTES_LENGTH),
  };
}

export function mapDiscoverySuggestion(
  suggestion: IpcPrinterDiscoverySuggestion,
): PrinterDiscoverySuggestion {
  return {
    ...suggestion,
    bedMm: [...suggestion.bedMm],
    connection: {
      kind: "network",
      address: redactAddressCredentials(suggestion.connection.address),
    },
  };
}

export function mapMaintenanceLog(log: IpcPrinterMaintenanceLog): MaintenanceLog {
  return {
    id: log.id,
    metadataVersion: log.metadataVersion,
    printerId: log.printerId ?? "",
    task: log.title,
    performedAt: toDateInputValue(log.performedAt),
    notes: log.notes,
  };
}

export function mapMaintenanceReminder(reminder: IpcPrinterReminder): MaintenanceReminder {
  const lastCompletedAt = reminder.lastCompletedAt
    ? toDateInputValue(reminder.lastCompletedAt)
    : null;
  return {
    id: reminder.id,
    printerId: reminder.printerId ?? "",
    task: reminder.title,
    dueAt: toDateInputValue(reminder.dueAt),
    repeatEveryDays: reminder.intervalDays,
    status:
      reminder.status === "completed"
        ? "completed"
        : reminder.status === "paused"
          ? "paused"
          : "open",
    completedAt: lastCompletedAt,
    metadataVersion: reminder.metadataVersion,
    notes: reminder.notes,
    printerNickname: reminder.printerNickname,
    lastCompletedAt,
    createdAt: reminder.createdAt,
    updatedAt: reminder.updatedAt,
  };
}

export function mapMaintenanceLogDraftToInput(
  draft: MaintenanceLogDraft,
): IpcPrinterMaintenanceInput {
  return {
    title: draft.task.trim(),
    performedAt: toApiUtcTimestamp(draft.performedAt),
    notes: draft.notes,
  };
}

export function mapMaintenanceReminderDraftToInput(
  draft: MaintenanceReminderDraft,
): IpcPrinterReminderInput {
  return {
    title: draft.task.trim(),
    notes: draft.notes,
    dueAt: toApiUtcTimestamp(draft.dueAt),
    intervalDays: draft.repeatEveryDays,
  };
}

function toDateInputValue(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toISOString().slice(0, 10);
}

function toApiUtcTimestamp(value: string): string {
  if (isDateOnly(value)) return `${value}T00:00:00.000Z`;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toISOString();
}

export function mapReminderCompletion(completion: PrinterReminderCompletion): {
  reminder: MaintenanceReminder;
  maintenanceLog: MaintenanceLog;
} {
  return {
    reminder: mapMaintenanceReminder(completion.reminder),
    maintenanceLog: mapMaintenanceLog(completion.maintenanceLog),
  };
}

export interface PrinterFleetModel {
  managedPrinters: ManagedPrinter[];
  slicerDetections: SlicerPrinterDetection[];
  activePrinterId: string;
  maintenanceLogs: MaintenanceLog[];
  maintenanceReminders: MaintenanceReminder[];
}

export interface DiscoveredPrinterDraft {
  draft: PrinterDraft;
  endpointApproved: boolean;
}

export interface PrinterFleetCallbacks {
  /** Deletes by metadata version; caller reconciles state even after a partial failure. */
  removePrinter(id: string): Promise<void>;
  savePrinter(draft: PrinterDraft): Promise<ManagedPrinter>;
  setDefaultPrinter(id: string): Promise<void>;
  discoverPrinters(range: string): Promise<PrinterDiscoverySuggestion[]>;
  cancelDiscovery(): Promise<void>;
  /** Persist the complete selected batch atomically and return each saved printer. */
  addDiscoveredPrinters(drafts: DiscoveredPrinterDraft[]): Promise<ManagedPrinter[]>;
  testConnection(
    id: string,
    mainboardId?: string,
    discoverMainboardId?: boolean,
  ): Promise<{
    result: PrinterConnectionTestResult;
    printer: ManagedPrinter;
    statusEventObserved: boolean;
  }>;
  saveMaintenanceLog(draft: MaintenanceLogDraft): Promise<MaintenanceLog>;
  deleteMaintenanceLog(id: string, expectedVersion: number): Promise<void>;
  saveMaintenanceReminder(draft: MaintenanceReminderDraft): Promise<MaintenanceReminder>;
  /** Advances a repeat schedule or marks a one-off reminder complete atomically. */
  completeMaintenanceReminder(
    id: string,
    expectedVersion: number,
  ): Promise<{ reminder: MaintenanceReminder; maintenanceLog: MaintenanceLog }>;
  deleteMaintenanceReminder(id: string): Promise<void>;
}

export const MAX_MANAGED_PRINTERS = 5_000;
export const MAX_DISCOVERY_SUGGESTIONS = 32;
export const MAX_DISCOVERY_ADDRESSES = 1_022;
export const MAX_MAINTENANCE_LOGS_PER_PRINTER = 500;
export const MAX_REMINDERS_PER_PRINTER = 100;
export const GENERIC_PRINTER_ID = "generic-256";
export const CONNECTION_SNAPSHOT_STALE_AFTER_MS = 15 * 60 * 1_000;
const MAX_NAME_LENGTH = 100;
const MAX_NOTES_LENGTH = 2_000;

export function createPrinterDraft(printer?: ManagedPrinter): PrinterDraft {
  return printer
    ? {
        id: printer.id,
        metadataVersion: printer.metadataVersion,
        name: printer.name,
        make: printer.make,
        model: printer.model,
        printerType: printer.printerType,
        firmware: printer.firmware,
        webUrl: printer.webUrl,
        notes: printer.notes,
        bedMm: [...printer.bedMm],
        connection: safeConnectionCopy(printer.connection),
        endpointApproved: printer.endpointApproved,
      }
    : {
        id: null,
        metadataVersion: null,
        name: "",
        make: null,
        model: "",
        printerType: null,
        firmware: null,
        webUrl: null,
        notes: "",
        bedMm: [256, 256, 256],
        connection: { kind: "local" },
        endpointApproved: false,
      };
}

export function validatePrinterDraft(draft: PrinterDraft): string | null {
  if (!draft.name.trim() || draft.name.trim().length > MAX_NAME_LENGTH) {
    return `Enter a printer name up to ${MAX_NAME_LENGTH} characters.`;
  }
  if (draft.model.trim().length > MAX_NAME_LENGTH) {
    return `Keep the model name to ${MAX_NAME_LENGTH} characters or fewer.`;
  }
  if (
    draft.bedMm.length !== 3 ||
    draft.bedMm.some(
      (dimension) => !Number.isFinite(dimension) || dimension <= 0 || dimension > 2_000,
    )
  ) {
    return "Enter three bed dimensions between 0 and 2,000 mm.";
  }
  if (draft.connection.kind === "network" && !validNetworkAddress(draft.connection.address)) {
    return "Enter a valid HTTP or HTTPS printer address without a username or password.";
  }
  return null;
}

export function resolveEndpointApproval(
  network: boolean,
  address: string,
  originalAddress: string | null,
  originalApproval: boolean,
  explicitlyApproved: boolean,
): boolean {
  if (!network) return false;
  const unchanged =
    originalAddress !== null && normalizeAddress(address) === normalizeAddress(originalAddress);
  return explicitlyApproved || (unchanged && originalApproval);
}

export function validateDiscoveryRange(range: string): string | null {
  const value = range.trim();
  if (!value || value.length > 512) return "Enter private IPv4 addresses, CIDRs, or ranges.";
  const parsed = parseDiscoveryRanges(value);
  if (!parsed)
    return "Enter valid private IPv4 addresses, CIDRs, ranges, or comma-separated terms.";
  if (parsed.addressCount > MAX_DISCOVERY_ADDRESSES) {
    return `The total discovery scope must contain at most ${MAX_DISCOVERY_ADDRESSES.toLocaleString()} addresses.`;
  }
  return null;
}

export function mergeDiscoverySuggestions(
  suggestions: readonly PrinterDiscoverySuggestion[],
  existing: readonly ManagedPrinter[] = [],
  range?: string,
): PrinterDiscoverySuggestion[] {
  const seen = new Set<string>();
  const addresses = new Set(
    existing
      .filter((printer) => printer.connection.kind === "network")
      .map((printer) =>
        normalizeAddress(printer.connection.kind === "network" ? printer.connection.address : ""),
      ),
  );
  const merged: PrinterDiscoverySuggestion[] = [];
  for (const suggestion of suggestions) {
    const key = suggestion.key.trim().toLowerCase();
    const address =
      suggestion.connection.kind === "network"
        ? normalizeAddress(suggestion.connection.address)
        : "";
    if (!key || seen.has(key) || (address && addresses.has(address))) continue;
    if (range && !suggestionAddressWithinRange(suggestion, range)) continue;
    const draft = suggestionToDraft(suggestion);
    if (validatePrinterDraft(draft)) continue;
    seen.add(key);
    if (address) addresses.add(address);
    merged.push({
      ...suggestion,
      key,
      bedMm: [...suggestion.bedMm],
      connection: { ...suggestion.connection },
    });
    if (merged.length === MAX_DISCOVERY_SUGGESTIONS) break;
  }
  return merged;
}

export function sanitizeConnectionTestStatus(result: unknown): ConnectionTestStatus {
  const raw =
    typeof result === "string"
      ? result
      : result && typeof result === "object" && "result" in result
        ? String((result as { result?: unknown }).result)
        : "";
  switch (raw) {
    case "connected":
    case "unreachable":
    case "unsupported":
    case "authRequired":
    case "failed":
    case "cancelled":
      return raw;
    default:
      return "failed";
  }
}

export function connectionTestCopy(result: unknown): string {
  switch (sanitizeConnectionTestStatus(result)) {
    case "connected":
      return "Connection test succeeded.";
    case "unreachable":
      return "Printer did not respond. Check that it is powered on and try again.";
    case "unsupported":
      return "Connection testing is not available for this printer type.";
    case "authRequired":
      return "The printer requires authentication. GoggleLab did not collect or store credentials.";
    case "cancelled":
      return "Connection test cancelled.";
    default:
      return "Connection test failed. Check the printer details and try again.";
  }
}

export function validateMaintenanceLogDraft(draft: MaintenanceLogDraft): string | null {
  if (
    draft.id !== null &&
    (typeof draft.metadataVersion !== "number" ||
      !Number.isSafeInteger(draft.metadataVersion) ||
      draft.metadataVersion < 1)
  ) {
    return "Reload this maintenance log before editing it.";
  }
  if (!draft.printerId) return "Choose a printer for this maintenance log.";
  if (!draft.task.trim() || draft.task.trim().length > 120) {
    return "Enter a maintenance task up to 120 characters.";
  }
  if (!isDateOnly(draft.performedAt)) return "Choose a valid maintenance date.";
  if (draft.notes.length > MAX_NOTES_LENGTH) {
    return `Keep maintenance notes to ${MAX_NOTES_LENGTH} characters or fewer.`;
  }
  return null;
}

export function validateMaintenanceReminderDraft(draft: MaintenanceReminderDraft): string | null {
  if (!draft.printerId) return "Choose a printer for this reminder.";
  if (!draft.task.trim() || draft.task.trim().length > 120) {
    return "Enter a reminder task up to 120 characters.";
  }
  if (!isDateOnly(draft.dueAt)) return "Choose a valid due date.";
  if (
    draft.repeatEveryDays !== null &&
    (!Number.isInteger(draft.repeatEveryDays) ||
      draft.repeatEveryDays < 1 ||
      draft.repeatEveryDays > 3_650)
  ) {
    return "Repeat interval must be from 1 to 3,650 days.";
  }
  return null;
}

export function createMaintenanceLogDraft(
  printerId: string,
  log?: MaintenanceLog,
): MaintenanceLogDraft {
  return log
    ? { ...log }
    : {
        id: null,
        metadataVersion: null,
        printerId,
        task: "",
        performedAt: localDateInputValue(new Date()),
        notes: "",
      };
}

export function createMaintenanceReminderDraft(
  printerId: string,
  reminder?: MaintenanceReminder,
): MaintenanceReminderDraft {
  return reminder
    ? {
        id: reminder.id,
        printerId: reminder.printerId,
        task: reminder.task,
        dueAt: reminder.dueAt,
        repeatEveryDays: reminder.repeatEveryDays,
        metadataVersion: reminder.metadataVersion,
        notes: reminder.notes,
      }
    : {
        id: null,
        printerId,
        task: "",
        dueAt: localDateInputValue(new Date()),
        repeatEveryDays: null,
        metadataVersion: null,
        notes: "",
      };
}

export function localDateInputValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export class PrinterFleetState {
  model: PrinterFleetModel;
  readonly callbacks: PrinterFleetCallbacks;
  suggestions: PrinterDiscoverySuggestion[] = [];
  error = "";
  status = "";
  discoveryTruncated = false;
  discovering = false;
  cancellationRequested = false;
  readonly connectionTests = new Map<string, ConnectionTestStatus>();
  readonly busyTests = new Set<string>();
  private readonly selectedSuggestionKeys = new Set<string>();
  private activeDiscoveryRevision = 0;

  constructor(model: PrinterFleetModel, callbacks: PrinterFleetCallbacks) {
    this.model = cloneFleetModel(model);
    this.callbacks = callbacks;
  }

  get selectedSuggestions(): string[] {
    return [...this.selectedSuggestionKeys];
  }

  updateModel(model: PrinterFleetModel): void {
    this.model = cloneFleetModel(model);
  }

  async savePrinter(draft: PrinterDraft): Promise<boolean> {
    this.clearMessages();
    const problem = validatePrinterDraft(draft);
    if (problem) {
      this.error = problem;
      return false;
    }
    if (!draft.id && this.model.managedPrinters.length >= MAX_MANAGED_PRINTERS) {
      this.error = `You can manage up to ${MAX_MANAGED_PRINTERS} printers.`;
      return false;
    }
    this.status = "Saving printer…";
    try {
      const saved = await this.callbacks.savePrinter(clonePrinterDraft(draft));
      const existing = this.model.managedPrinters.some((printer) => printer.id === saved.id);
      this.model.managedPrinters = existing
        ? this.model.managedPrinters.map((printer) =>
            printer.id === saved.id ? cloneManagedPrinter(saved) : printer,
          )
        : [...this.model.managedPrinters, cloneManagedPrinter(saved)];
      this.status = "Printer saved.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not save printer: ${errorMessage(error)}`;
      return false;
    }
  }

  async removePrinter(id: string): Promise<boolean> {
    if (!this.model.managedPrinters.some((printer) => printer.id === id)) return false;
    this.clearMessages();
    this.status = "Removing printer…";
    try {
      await this.callbacks.removePrinter(id);
      this.model.managedPrinters = this.model.managedPrinters.filter(
        (printer) => printer.id !== id,
      );
      if (this.model.activePrinterId === id) this.model.activePrinterId = GENERIC_PRINTER_ID;
      this.connectionTests.delete(id);
      this.status = "Printer removed.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not remove printer: ${errorMessage(error)}`;
      return false;
    }
  }

  async setDefaultPrinter(id: string): Promise<boolean> {
    if (!isSelectablePrinterId(this.model, id)) {
      this.error = "Choose an available printer for bed fit.";
      this.status = "";
      return false;
    }
    this.clearMessages();
    this.status = "Saving default printer…";
    try {
      await this.callbacks.setDefaultPrinter(id);
      this.model.activePrinterId = id;
      this.status = "Active printer updated.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not save the default printer: ${errorMessage(error)}`;
      return false;
    }
  }

  async discover(range: string): Promise<boolean> {
    this.clearMessages();
    if (this.discovering) {
      this.error = "A printer search is already running.";
      return false;
    }
    const problem = validateDiscoveryRange(range);
    if (problem) {
      this.error = problem;
      return false;
    }
    const revision = ++this.activeDiscoveryRevision;
    this.discovering = true;
    this.cancellationRequested = false;
    this.status = `Searching ${range.trim()}…`;
    try {
      const raw = await this.callbacks.discoverPrinters(range.trim());
      if (revision !== this.activeDiscoveryRevision || this.cancellationRequested) {
        this.status = "Printer search cancelled. Previous suggestions remain available.";
        return false;
      }
      this.discoveryTruncated = raw.length > MAX_DISCOVERY_SUGGESTIONS;
      this.suggestions = mergeDiscoverySuggestions(raw, this.model.managedPrinters, range.trim());
      this.selectedSuggestionKeys.clear();
      this.status = this.suggestions.length
        ? `${this.suggestions.length} printer suggestion${this.suggestions.length === 1 ? "" : "s"} ready to review${this.discoveryTruncated ? `; showing the first ${MAX_DISCOVERY_SUGGESTIONS}` : ""}. Nothing has been added yet.`
        : "No printer suggestions found in that range.";
      return true;
    } catch {
      if (revision !== this.activeDiscoveryRevision || this.cancellationRequested) {
        this.status = "Printer search cancelled. Previous suggestions remain available.";
        return false;
      }
      this.status = "";
      this.error = "Could not discover printers in that range. Check the range and try again.";
      return false;
    } finally {
      if (revision === this.activeDiscoveryRevision) {
        this.discovering = false;
        this.cancellationRequested = false;
      }
    }
  }

  async cancelDiscovery(): Promise<boolean> {
    if (!this.discovering || this.cancellationRequested) return false;
    this.error = "";
    this.cancellationRequested = true;
    this.status = "Cancelling printer search…";
    try {
      await this.callbacks.cancelDiscovery();
      if (this.discovering) {
        this.status = "Cancellation requested. Previous suggestions remain available.";
      }
      return true;
    } catch {
      this.cancellationRequested = false;
      this.status = "";
      this.error = "Could not cancel the printer search. It may still be running.";
      return false;
    }
  }

  selectSuggestion(key: string, selected: boolean): void {
    if (!this.suggestions.some((suggestion) => suggestion.key === key)) return;
    if (selected) this.selectedSuggestionKeys.add(key);
    else this.selectedSuggestionKeys.delete(key);
  }

  isSuggestionSelected(key: string): boolean {
    return this.selectedSuggestionKeys.has(key);
  }

  async addSelectedSuggestions(endpointApproved = false): Promise<boolean> {
    this.clearMessages();
    const chosen = this.suggestions.filter((suggestion) =>
      this.selectedSuggestionKeys.has(suggestion.key),
    );
    if (!chosen.length) {
      this.error = "Select at least one printer suggestion to add.";
      return false;
    }
    if (this.model.managedPrinters.length + chosen.length > MAX_MANAGED_PRINTERS) {
      this.error = `You can manage up to ${MAX_MANAGED_PRINTERS} printers.`;
      return false;
    }
    this.status = `Adding ${chosen.length} selected printer${chosen.length === 1 ? "" : "s"}…`;
    try {
      const saved = await this.callbacks.addDiscoveredPrinters(
        chosen.map((suggestion) => ({
          draft: suggestionToDraft(suggestion),
          endpointApproved,
        })),
      );
      if (saved.length !== chosen.length) {
        throw new Error("The saved printer list did not match the reviewed selection.");
      }
      this.model.managedPrinters = [
        ...this.model.managedPrinters,
        ...saved.map(cloneManagedPrinter),
      ];
      const addedKeys = new Set(chosen.map((suggestion) => suggestion.key));
      this.suggestions = this.suggestions.filter((suggestion) => !addedKeys.has(suggestion.key));
      this.selectedSuggestionKeys.clear();
      this.status = `${saved.length} printer${saved.length === 1 ? "" : "s"} added to the fleet.`;
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not add the selected printers: ${errorMessage(error)}`;
      return false;
    }
  }

  async testConnection(
    id: string,
    mainboardId?: string,
    discoverMainboardId = false,
  ): Promise<boolean> {
    const printer = this.model.managedPrinters.find((item) => item.id === id);
    if (!printer || this.busyTests.has(id)) return false;
    const snapshot = cloneManagedPrinter(printer);
    const requestIsCurrent = () =>
      printerMatchesConnectionSnapshot(
        this.model.managedPrinters.find((item) => item.id === id),
        snapshot,
      );
    const ignoreStaleResult = () => {
      if (this.status === "Testing printer connection…") {
        this.status = "Printer details changed; the previous connection test result was ignored.";
      }
      return false;
    };
    this.error = "";
    this.status = "Testing printer connection…";
    this.busyTests.add(id);
    try {
      const response = await this.callbacks.testConnection(id, mainboardId, discoverMainboardId);
      if (!requestIsCurrent()) return ignoreStaleResult();
      const result = sanitizeConnectionTestStatus(response?.result);
      if (result === "cancelled") {
        this.status = connectionTestCopy(result);
        return false;
      }
      if (response?.printer?.id === id) {
        this.model.managedPrinters = this.model.managedPrinters.map((item) =>
          item.id === id ? cloneManagedPrinter(response.printer) : item,
        );
      }
      this.connectionTests.set(id, result);
      const reportedJobState =
        response?.printer?.id === id ? jobStatusLabel(response.printer.jobStatus) : "unknown";
      this.status =
        result === "connected" && response?.statusEventObserved === true
          ? `SDCP event observed during this test; reported job state: ${reportedJobState}.`
          : connectionTestCopy(result);
      return result === "connected";
    } catch {
      if (!requestIsCurrent()) return ignoreStaleResult();
      this.connectionTests.set(id, "failed");
      this.status = connectionTestCopy("failed");
      return false;
    } finally {
      this.busyTests.delete(id);
    }
  }

  async saveMaintenanceLog(draft: MaintenanceLogDraft): Promise<boolean> {
    this.clearMessages();
    const problem = validateMaintenanceLogDraft(draft);
    if (problem) {
      this.error = problem;
      return false;
    }
    if (!this.hasPrinter(draft.printerId)) {
      this.error = "This printer is no longer in the fleet.";
      return false;
    }
    const existingCount = this.model.maintenanceLogs.filter(
      (log) => log.printerId === draft.printerId && log.id !== draft.id,
    ).length;
    if (!draft.id && existingCount >= MAX_MAINTENANCE_LOGS_PER_PRINTER) {
      this.error = `Keep no more than ${MAX_MAINTENANCE_LOGS_PER_PRINTER} maintenance logs per printer.`;
      return false;
    }
    this.status = "Saving maintenance log…";
    try {
      const saved = await this.callbacks.saveMaintenanceLog({ ...draft });
      this.model.maintenanceLogs = upsertById(this.model.maintenanceLogs, saved);
      this.status = "Maintenance log saved.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not save maintenance log: ${errorMessage(error)}`;
      return false;
    }
  }

  async deleteMaintenanceLog(id: string, expectedVersion: number): Promise<boolean> {
    this.clearMessages();
    this.status = "Removing maintenance log…";
    try {
      await this.callbacks.deleteMaintenanceLog(id, expectedVersion);
      this.model.maintenanceLogs = this.model.maintenanceLogs.filter((log) => log.id !== id);
      this.status = "Maintenance log removed.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not remove maintenance log: ${errorMessage(error)}`;
      return false;
    }
  }

  async saveMaintenanceReminder(draft: MaintenanceReminderDraft): Promise<boolean> {
    this.clearMessages();
    const problem = validateMaintenanceReminderDraft(draft);
    if (problem) {
      this.error = problem;
      return false;
    }
    if (!this.hasPrinter(draft.printerId)) {
      this.error = "This printer is no longer in the fleet.";
      return false;
    }
    const existingCount = this.model.maintenanceReminders.filter(
      (reminder) => reminder.printerId === draft.printerId && reminder.id !== draft.id,
    ).length;
    if (!draft.id && existingCount >= MAX_REMINDERS_PER_PRINTER) {
      this.error = `Keep no more than ${MAX_REMINDERS_PER_PRINTER} reminders per printer.`;
      return false;
    }
    this.status = "Saving maintenance reminder…";
    try {
      const saved = await this.callbacks.saveMaintenanceReminder({ ...draft });
      this.model.maintenanceReminders = upsertById(this.model.maintenanceReminders, saved);
      this.status = "Maintenance reminder saved.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not save maintenance reminder: ${errorMessage(error)}`;
      return false;
    }
  }

  async completeMaintenanceReminder(id: string): Promise<boolean> {
    const reminder = this.model.maintenanceReminders.find((item) => item.id === id);
    if (!reminder || reminder.status === "completed") return false;
    this.clearMessages();
    this.status = "Completing maintenance reminder…";
    try {
      const completion = await this.callbacks.completeMaintenanceReminder(
        id,
        reminder.metadataVersion,
      );
      const completed = completion.reminder;
      this.model.maintenanceReminders = upsertById(this.model.maintenanceReminders, completed);
      this.model.maintenanceLogs = upsertById(
        this.model.maintenanceLogs,
        completion.maintenanceLog,
      );
      this.status =
        completed.status === "completed"
          ? "Maintenance reminder completed."
          : "Next maintenance reminder scheduled.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not complete maintenance reminder: ${errorMessage(error)}`;
      return false;
    }
  }

  async deleteMaintenanceReminder(id: string): Promise<boolean> {
    this.clearMessages();
    this.status = "Removing maintenance reminder…";
    try {
      await this.callbacks.deleteMaintenanceReminder(id);
      this.model.maintenanceReminders = this.model.maintenanceReminders.filter(
        (reminder) => reminder.id !== id,
      );
      this.status = "Maintenance reminder removed.";
      return true;
    } catch (error) {
      this.status = "";
      this.error = `Could not remove maintenance reminder: ${errorMessage(error)}`;
      return false;
    }
  }

  private hasPrinter(id: string): boolean {
    return this.model.managedPrinters.some((printer) => printer.id === id);
  }

  private clearMessages(): void {
    this.error = "";
    this.status = "";
  }
}

/** Standalone settings surface. All network work is exposed as explicit actions. */
export class PrinterFleetManager {
  readonly element: HTMLElement;
  readonly state: PrinterFleetState;

  private readonly content: HTMLElement;
  private readonly statusRegion: HTMLElement;
  private readonly errorRegion: HTMLElement;
  private readonly dialog: HTMLDialogElement;
  private discoveryRange = "";
  private dialogInvoker: HTMLElement | null = null;
  private nextDialogId = 0;
  private dialogRevision = 0;

  constructor(container: HTMLElement, model: PrinterFleetModel, callbacks: PrinterFleetCallbacks) {
    this.state = new PrinterFleetState(model, callbacks);
    this.element = document.createElement("section");
    this.element.className = "printer-fleet-manager";
    this.content = document.createElement("div");
    this.content.className = "printer-fleet-content";
    this.statusRegion = document.createElement("div");
    this.statusRegion.className = "settings-status";
    this.statusRegion.setAttribute("role", "status");
    this.statusRegion.setAttribute("aria-live", "polite");
    this.statusRegion.setAttribute("aria-atomic", "true");
    this.errorRegion = document.createElement("div");
    this.errorRegion.className = "settings-error";
    this.errorRegion.setAttribute("role", "alert");
    this.errorRegion.setAttribute("aria-live", "assertive");
    this.errorRegion.setAttribute("aria-atomic", "true");
    this.element.append(this.content, this.statusRegion, this.errorRegion);

    this.dialog = document.createElement("dialog");
    this.dialog.className = "printer-fleet-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.closeDialog();
    });
    container.append(this.element, this.dialog);
    this.render();
  }

  updateModel(model: PrinterFleetModel): void {
    this.state.updateModel(model);
    const ids = new Set(this.state.model.managedPrinters.map((printer) => printer.id));
    for (const id of this.state.connectionTests.keys()) {
      if (!ids.has(id)) this.state.connectionTests.delete(id);
    }
    this.render();
  }

  private render(): void {
    const active = this.content.contains(document.activeElement)
      ? (document.activeElement as HTMLElement)
      : null;
    const focusKey = active?.dataset.fleetFocus;
    const content: HTMLElement[] = [
      this.renderHeader(),
      this.renderDefaultPrinter(),
      this.renderManagedPrinters(),
      this.renderDiscovery(),
      this.renderSlicerDetections(),
    ];
    const archived = this.renderArchivedMaintenance();
    if (archived) content.push(archived);
    this.content.replaceChildren(...content);
    this.statusRegion.textContent = this.state.status;
    this.errorRegion.textContent = this.state.error;
    if (focusKey) findFleetFocusControl(this.content, focusKey)?.focus();
  }

  private renderHeader(): HTMLElement {
    const section = document.createElement("header");
    section.className = "printer-fleet-header";
    const title = document.createElement("h2");
    title.textContent = "Printer fleet";
    const add = fleetButton("Add printer…", () => this.openPrinterDialog());
    add.dataset.fleetFocus = "add-printer";
    add.disabled = this.state.model.managedPrinters.length >= MAX_MANAGED_PRINTERS;
    if (add.disabled) add.title = `Maximum of ${MAX_MANAGED_PRINTERS} managed printers reached.`;
    section.append(title, add);
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = add.disabled
      ? `You can manage up to ${MAX_MANAGED_PRINTERS} printers. Discovery and connection tests run only when you choose them.`
      : "Manage your printers here. Discovery and connection tests run only when you choose them.";
    section.append(hint);
    return section;
  }

  private renderDefaultPrinter(): HTMLFieldSetElement {
    const fieldset = document.createElement("fieldset");
    fieldset.className = "settings-section printer-fleet-default";
    const legend = document.createElement("legend");
    legend.textContent = "Active printer";
    const row = document.createElement("div");
    row.className = "settings-row";
    const label = document.createElement("label");
    label.className = "label";
    label.htmlFor = "printer-fleet-active-printer";
    label.textContent = "Printer used for bed fit";
    const select = document.createElement("select");
    select.id = label.htmlFor;
    select.className = "select";
    select.add(new Option("Generic 256 mm bed", GENERIC_PRINTER_ID));
    for (const detection of this.state.model.slicerDetections) {
      select.add(new Option(`${detection.name} · slicer detected`, detection.id));
    }
    for (const printer of this.state.model.managedPrinters) {
      select.add(
        new Option(`${printer.name}${printer.model ? ` · ${printer.model}` : ""}`, printer.id),
      );
    }
    if (![...select.options].some((option) => option.value === this.state.model.activePrinterId)) {
      const unavailable = new Option(
        `Unavailable printer (${this.state.model.activePrinterId})`,
        this.state.model.activePrinterId,
      );
      unavailable.disabled = true;
      select.add(unavailable);
    }
    select.value = this.state.model.activePrinterId;
    select.dataset.fleetFocus = "active-printer";
    select.addEventListener("change", () => {
      void this.changeDefaultPrinter(select.value, select);
    });
    row.append(label, select);
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent =
      "Managed printers, slicer detections, and Generic 256 are selectable. Slicer detections are read-only.";
    fieldset.append(legend, row, hint);
    return fieldset;
  }

  private renderManagedPrinters(): HTMLFieldSetElement {
    const fieldset = document.createElement("fieldset");
    fieldset.className = "settings-section printer-fleet-list";
    const legend = document.createElement("legend");
    legend.textContent = `Managed printers (${this.state.model.managedPrinters.length})`;
    const list = document.createElement("ul");
    list.className = "printer-fleet-printer-list";
    if (!this.state.model.managedPrinters.length) {
      const empty = document.createElement("p");
      empty.className = "hint";
      empty.textContent = "No managed printers yet. Add one or review discovery suggestions.";
      fieldset.append(legend, empty);
      return fieldset;
    }
    for (const printer of this.state.model.managedPrinters) {
      list.append(this.renderManagedPrinter(printer));
    }
    fieldset.append(legend, list);
    return fieldset;
  }

  private renderManagedPrinter(printer: ManagedPrinter): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "printer-fleet-printer";
    const heading = document.createElement("h3");
    heading.textContent = printer.name;
    const description = document.createElement("p");
    description.className = "printer-fleet-description";
    description.textContent = `${printer.model || "Printer"} · ${printer.bedMm.join(" × ")} mm`;
    const connection = document.createElement("p");
    connection.className = "printer-fleet-connection";
    connection.textContent =
      printer.connection.kind === "network"
        ? `Network · ${printer.connection.address}`
        : "Local connection";
    const snapshot = document.createElement("p");
    snapshot.className = "value printer-fleet-test-status";
    snapshot.textContent = connectionSnapshotCopy(printer);

    const actions = document.createElement("div");
    actions.className = "settings-actions printer-fleet-actions";
    const test = fleetButton("Test connection", () => void this.testConnection(printer.id, test));
    test.setAttribute("aria-label", `Test connection to ${printer.name}`);
    test.dataset.fleetFocus = `test:${printer.id}`;
    test.disabled =
      this.state.busyTests.has(printer.id) ||
      printer.connection.kind !== "network" ||
      !printer.endpointApproved;
    test.setAttribute("aria-busy", String(this.state.busyTests.has(printer.id)));
    if (!printer.endpointApproved) {
      test.title = "Edit this printer and approve its address before testing the connection.";
    } else if (printer.connection.kind !== "network") {
      test.title = "Connection tests require a network address.";
    }
    if (requiresElegooMainboardId(printer)) {
      const findAndTest = fleetButton(
        "Find ID & test",
        () => void this.findAndTestElegoo(printer.id, findAndTest),
      );
      findAndTest.setAttribute("aria-label", `Find MainboardID and test ${printer.name}`);
      findAndTest.dataset.fleetFocus = `find-mainboard:${printer.id}`;
      findAndTest.disabled = test.disabled;
      findAndTest.setAttribute("aria-busy", String(this.state.busyTests.has(printer.id)));
      findAndTest.title = test.disabled
        ? test.title
        : "Send one disclosed SDCP discovery broadcast, then run a read-only status check.";
      actions.append(test, findAndTest);
    } else {
      actions.append(test);
    }
    const edit = fleetButton("Edit…", () => this.openPrinterDialog(printer, edit));
    edit.setAttribute("aria-label", `Edit ${printer.name}`);
    edit.dataset.fleetFocus = `edit:${printer.id}`;
    const remove = fleetButton("Remove", () => void this.confirmRemovePrinter(printer, remove));
    remove.setAttribute("aria-label", `Remove ${printer.name}`);
    remove.dataset.fleetFocus = `remove:${printer.id}`;
    actions.append(edit, remove);

    const maintenance = document.createElement("details");
    maintenance.className = "printer-fleet-maintenance";
    const logs = this.state.model.maintenanceLogs.filter((entry) => entry.printerId === printer.id);
    const reminders = this.state.model.maintenanceReminders.filter(
      (entry) => entry.printerId === printer.id,
    );
    const summary = document.createElement("summary");
    summary.dataset.fleetFocus = `maintenance:${printer.id}`;
    summary.textContent = `Maintenance · ${logs.length} log${logs.length === 1 ? "" : "s"} · ${reminders.length} reminder${reminders.length === 1 ? "" : "s"}`;
    maintenance.append(summary, this.renderMaintenance(printer, logs, reminders));
    const approval = document.createElement("p");
    approval.className = "hint";
    approval.textContent = printer.endpointApproved
      ? "Connection tests are approved for this address."
      : "Connection tests are not approved for this address.";
    item.append(heading, description, connection, snapshot, approval, actions, maintenance);
    return item;
  }

  private renderMaintenance(
    printer: ManagedPrinter,
    logs: MaintenanceLog[],
    reminders: MaintenanceReminder[],
  ): HTMLElement {
    const section = document.createElement("div");
    section.className = "printer-fleet-maintenance-content";
    const actions = document.createElement("div");
    actions.className = "settings-actions";
    const addLog = fleetButton("Log maintenance…", () => this.openMaintenanceLogDialog(printer.id));
    addLog.dataset.fleetFocus = `add-log:${printer.id}`;
    const addReminder = fleetButton("Add reminder…", () => this.openReminderDialog(printer.id));
    addReminder.dataset.fleetFocus = `add-reminder:${printer.id}`;
    actions.append(addLog, addReminder);

    const logList = document.createElement("ul");
    logList.className = "printer-fleet-maintenance-log-list";
    if (!logs.length) {
      const empty = document.createElement("li");
      empty.textContent = "No maintenance logged.";
      logList.append(empty);
    }
    for (const log of logs) logList.append(this.renderMaintenanceLog(log));

    const reminderList = document.createElement("ul");
    reminderList.className = "printer-fleet-reminder-list";
    if (!reminders.length) {
      const empty = document.createElement("li");
      empty.textContent = "No maintenance reminders.";
      reminderList.append(empty);
    }
    for (const reminder of reminders) reminderList.append(this.renderReminder(reminder));
    section.append(actions, logList, reminderList);
    return section;
  }

  private renderMaintenanceLog(log: MaintenanceLog): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "printer-fleet-maintenance-entry";
    const text = document.createElement("span");
    text.textContent = `${log.performedAt} · ${log.task}${log.notes ? ` · ${log.notes}` : ""}`;
    const edit = fleetButton("Edit log…", () =>
      this.openMaintenanceLogDialog(log.printerId, log, edit),
    );
    edit.setAttribute("aria-label", `Edit maintenance log: ${log.task}`);
    edit.dataset.fleetFocus = `edit-log:${log.id}`;
    const remove = fleetButton("Remove", () => void this.confirmDeleteLog(log, remove));
    remove.setAttribute("aria-label", `Remove maintenance log: ${log.task}`);
    remove.dataset.fleetFocus = `remove-log:${log.id}`;
    item.append(text, edit, remove);
    return item;
  }

  private renderReminder(reminder: MaintenanceReminder): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "printer-fleet-reminder-entry";
    const text = document.createElement("span");
    const repeat = reminder.repeatEveryDays
      ? ` · repeats every ${reminder.repeatEveryDays} days`
      : "";
    text.textContent = `${reminder.task} · due ${reminder.dueAt} · ${reminder.status}${repeat}`;
    item.append(text);
    if (reminder.status === "open") {
      const complete = fleetButton(
        "Mark complete",
        () => void this.confirmCompleteReminder(reminder, complete),
      );
      complete.setAttribute("aria-label", `Mark ${reminder.task} reminder complete`);
      complete.dataset.fleetFocus = `complete-reminder:${reminder.id}`;
      item.append(complete);
    }
    const edit = fleetButton("Edit reminder…", () =>
      this.openReminderDialog(reminder.printerId, reminder, edit),
    );
    edit.setAttribute("aria-label", `Edit ${reminder.task} reminder`);
    edit.dataset.fleetFocus = `edit-reminder:${reminder.id}`;
    const remove = fleetButton("Remove", () => void this.confirmDeleteReminder(reminder, remove));
    remove.setAttribute("aria-label", `Remove ${reminder.task} reminder`);
    remove.dataset.fleetFocus = `remove-reminder:${reminder.id}`;
    item.append(edit, remove);
    return item;
  }

  private renderDiscovery(): HTMLFieldSetElement {
    const fieldset = document.createElement("fieldset");
    fieldset.className = "settings-section printer-fleet-discovery";
    const legend = document.createElement("legend");
    legend.textContent = "Discover printers";
    const label = document.createElement("label");
    label.className = "label";
    label.htmlFor = "printer-fleet-discovery-range";
    label.textContent = "IPv4 address or range to search";
    const range = document.createElement("input");
    range.type = "text";
    range.id = label.htmlFor;
    range.maxLength = 512;
    range.placeholder = "192.168.1.0/24, 10.0.0.12-10.0.0.20";
    range.value = this.discoveryRange;
    range.dataset.fleetFocus = "discovery-range";
    range.setAttribute("aria-describedby", "printer-fleet-discovery-hint");
    range.addEventListener("input", () => {
      this.discoveryRange = range.value;
    });
    const hint = document.createElement("p");
    hint.id = "printer-fleet-discovery-hint";
    hint.className = "hint";
    hint.textContent = `Use private IPv4 addresses, CIDRs, address ranges, or comma-separated terms, up to ${MAX_DISCOVERY_ADDRESSES.toLocaleString()} addresses total. Bambu discovery sends multicast on routed interfaces; devices outside this range may receive the request, but replies outside it are discarded. Nothing is scanned until you click Discover.`;
    const discover = fleetButton(
      this.state.discovering ? "Searching…" : "Discover in this range",
      () => void this.runDiscovery(),
    );
    discover.dataset.fleetFocus = "discover";
    discover.disabled = this.state.discovering;
    const row = document.createElement("div");
    row.className = "settings-row";
    row.append(label, range, discover);
    if (this.state.discovering) {
      const cancel = fleetButton(
        this.state.cancellationRequested ? "Cancelling…" : "Cancel search",
        () => void this.cancelDiscovery(cancel),
      );
      cancel.dataset.fleetFocus = "cancel-discovery";
      cancel.disabled = this.state.cancellationRequested;
      cancel.setAttribute("aria-busy", String(this.state.cancellationRequested));
      row.append(cancel);
    }
    fieldset.append(legend, row, hint);

    if (this.state.suggestions.length) {
      const review = document.createElement("section");
      review.className = "printer-fleet-suggestion-review";
      const heading = document.createElement("h3");
      heading.textContent = "Review discovery suggestions";
      const list = document.createElement("ul");
      for (const suggestion of this.state.suggestions)
        list.append(this.renderSuggestion(suggestion));
      const approvedLabel = document.createElement("label");
      const approved = document.createElement("input");
      approved.type = "checkbox";
      approved.dataset.fleetFocus = "discovery-approve";
      approvedLabel.append(
        approved,
        document.createTextNode(" Allow connection tests for these addresses"),
      );
      const add = fleetButton(
        "Add selected printers",
        () => void this.addSelectedSuggestions(add, approved.checked),
      );
      add.dataset.fleetFocus = "add-discovered";
      const totalAfterAdd =
        this.state.model.managedPrinters.length + this.state.selectedSuggestions.length;
      add.disabled = !this.state.selectedSuggestions.length || totalAfterAdd > MAX_MANAGED_PRINTERS;
      if (totalAfterAdd > MAX_MANAGED_PRINTERS) {
        const limit = document.createElement("p");
        limit.className = "hint";
        limit.textContent = `Select fewer printers to stay within the ${MAX_MANAGED_PRINTERS}-printer limit.`;
        review.append(heading, list, approvedLabel, limit, add);
      } else {
        review.append(heading, list, approvedLabel, add);
      }
      fieldset.append(review);
    }
    if (this.state.discoveryTruncated) {
      const bounded = document.createElement("p");
      bounded.className = "hint";
      bounded.textContent = `Only the first ${MAX_DISCOVERY_SUGGESTIONS} results are shown. Narrow the range to see more.`;
      fieldset.append(bounded);
    }
    return fieldset;
  }

  private renderSuggestion(suggestion: PrinterDiscoverySuggestion): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "printer-fleet-suggestion";
    const label = document.createElement("label");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = this.state.isSuggestionSelected(suggestion.key);
    checkbox.setAttribute("aria-label", `Select discovered printer ${suggestion.name}`);
    checkbox.dataset.fleetFocus = `suggestion:${suggestion.key}`;
    checkbox.addEventListener("change", () => {
      this.state.selectSuggestion(suggestion.key, checkbox.checked);
      this.render();
    });
    const name = document.createElement("strong");
    name.textContent = suggestion.name;
    label.append(checkbox, name);
    const details = document.createElement("span");
    details.textContent = `${suggestion.model || "Printer"} · ${suggestion.bedMm.join(" × ")} mm · ${suggestion.source} · ${suggestion.printerType} · ${suggestion.connection.kind === "network" ? suggestion.connection.address : "local"}`;
    item.append(label, details);
    return item;
  }

  private renderSlicerDetections(): HTMLFieldSetElement {
    const fieldset = document.createElement("fieldset");
    fieldset.className = "settings-section printer-fleet-slicer-detections";
    const legend = document.createElement("legend");
    legend.textContent = "Printer profiles detected in slicers · read-only";
    const list = document.createElement("ul");
    if (!this.state.model.slicerDetections.length) {
      const empty = document.createElement("li");
      empty.textContent = "No slicer printer profiles detected.";
      list.append(empty);
    }
    for (const detection of this.state.model.slicerDetections) {
      const row = document.createElement("li");
      const name = document.createElement("strong");
      name.textContent = detection.name;
      const description = document.createElement("span");
      description.textContent = `${detection.model || "Printer"} · ${detection.bedMm.join(" × ")} mm · ${detection.source}`;
      row.append(name, description);
      list.append(row);
    }
    fieldset.append(legend, list);
    return fieldset;
  }

  private renderArchivedMaintenance(): HTMLElement | null {
    const managedIds = new Set(this.state.model.managedPrinters.map((printer) => printer.id));
    const logs = this.state.model.maintenanceLogs.filter((log) => !managedIds.has(log.printerId));
    const reminders = this.state.model.maintenanceReminders.filter(
      (reminder) => !managedIds.has(reminder.printerId),
    );
    if (!logs.length && !reminders.length) return null;
    const section = document.createElement("section");
    const heading = document.createElement("h3");
    heading.textContent = "Maintenance history for removed printers";
    const list = document.createElement("ul");
    for (const log of logs) {
      const item = document.createElement("li");
      item.textContent = `${log.performedAt} · ${log.task} · printer ${log.printerId}${log.notes ? ` · ${log.notes}` : ""}`;
      list.append(item);
    }
    for (const reminder of reminders) {
      const item = document.createElement("li");
      item.textContent = `${reminder.task} · due ${reminder.dueAt} · ${reminder.status} · printer ${reminder.printerId}`;
      list.append(item);
    }
    section.append(heading, list);
    return section;
  }

  private async changeDefaultPrinter(id: string, control: HTMLSelectElement): Promise<void> {
    control.disabled = true;
    const saving = this.state.setDefaultPrinter(id);
    this.renderLiveRegions();
    await saving;
    this.render();
  }

  private async runDiscovery(): Promise<void> {
    const discovery = this.state.discover(this.discoveryRange);
    this.render();
    this.renderLiveRegions();
    if (this.state.discovering) {
      findFleetFocusControl(this.content, "cancel-discovery")?.focus();
    }
    await discovery;
    this.render();
    findFleetFocusControl(this.content, "discover")?.focus();
  }

  private async cancelDiscovery(button: HTMLButtonElement): Promise<void> {
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    const cancellation = this.state.cancelDiscovery();
    this.renderLiveRegions();
    await cancellation;
    this.render();
    const focusTarget = this.state.discovering ? "cancel-discovery" : "discover";
    findFleetFocusControl(this.content, focusTarget)?.focus();
  }

  private async addSelectedSuggestions(
    button: HTMLButtonElement,
    endpointApproved: boolean,
  ): Promise<void> {
    button.disabled = true;
    const adding = this.state.addSelectedSuggestions(endpointApproved);
    this.renderLiveRegions();
    await adding;
    this.render();
  }

  private async testConnection(id: string, button: HTMLButtonElement): Promise<void> {
    const printer = this.state.model.managedPrinters.find((item) => item.id === id);
    if (
      !printer ||
      button.disabled ||
      this.state.busyTests.has(id) ||
      printer.connection.kind !== "network" ||
      !printer.endpointApproved
    )
      return;
    if (requiresElegooMainboardId(printer)) {
      this.openMainboardIdDialog(printer, button);
      return;
    }
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    const test = this.state.testConnection(id);
    this.renderLiveRegions();
    await test;
    this.render();
  }

  private openMainboardIdDialog(printer: ManagedPrinter, invoker: HTMLElement): void {
    const snapshot = cloneManagedPrinter(printer);
    const formId = `printer-mainboard-${++this.nextDialogId}`;
    const { form, error } = this.beginDialog("Test Centauri Carbon connection", formId, invoker);
    const revision = this.dialogRevision;
    const disclosure = document.createElement("p");
    disclosure.textContent = `Enter the MainboardID for ${snapshot.name} (${snapshot.connection.kind === "network" ? snapshot.connection.address : ""}). This one-time read-only SDCP check sends it over an unencrypted local-network connection to this printer. GoggleLab will not save it. No discovery broadcast is sent.`;
    form.append(disclosure);
    const input = this.textField(form, formId, "mainboard-id", "MainboardID", "", 128);
    input.autocomplete = "off";
    input.autocapitalize = "off";
    input.spellcheck = false;
    const submit = document.createElement("button");
    submit.type = "submit";
    submit.className = "btn primary";
    submit.textContent = "Test connection";
    form.append(submit);
    let pending = false;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      if (pending || !this.dialog.open || revision !== this.dialogRevision) return;
      const current = this.state.model.managedPrinters.find((item) => item.id === snapshot.id);
      if (
        !current ||
        current.metadataVersion !== snapshot.metadataVersion ||
        current.connection.kind !== "network" ||
        snapshot.connection.kind !== "network" ||
        current.connection.address !== snapshot.connection.address ||
        !current.endpointApproved ||
        current.make !== snapshot.make ||
        current.model !== snapshot.model ||
        !requiresElegooMainboardId(current) ||
        this.state.busyTests.has(snapshot.id)
      ) {
        input.value = "";
        error.textContent =
          "Printer details changed. Close this dialog, review the printer, and try again.";
        return;
      }
      const mainboardId = input.value.trim();
      if (!/^[A-Za-z0-9._-]{1,128}$/.test(mainboardId)) {
        error.textContent =
          "Enter a MainboardID with 1–128 letters, numbers, dots, underscores, or hyphens.";
        input.focus();
        return;
      }
      pending = true;
      input.value = "";
      input.disabled = true;
      submit.disabled = true;
      error.textContent = "";
      const checking = this.state.testConnection(snapshot.id, mainboardId, false);
      this.renderLiveRegions();
      void (async () => {
        const success = await checking;
        if (revision !== this.dialogRevision || !this.dialog.open) return;
        this.render();
        if (success) this.closeDialog();
        else {
          error.textContent = this.state.error || this.state.status;
          pending = false;
          input.disabled = false;
          submit.disabled = false;
          input.focus();
        }
      })();
    });
    this.showDialog();
  }

  private async findAndTestElegoo(id: string, button: HTMLButtonElement): Promise<void> {
    const printer = this.state.model.managedPrinters.find((item) => item.id === id);
    if (!printer || !requiresElegooMainboardId(printer) || button.disabled) return;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    const test = this.state.testConnection(id, undefined, true);
    this.renderLiveRegions();
    await test;
    this.render();
  }

  private async confirmRemovePrinter(printer: ManagedPrinter, invoker: HTMLElement): Promise<void> {
    if (
      !window.confirm(
        `Remove ${printer.name} from the managed fleet? Maintenance history is retained.`,
      )
    ) {
      return;
    }
    if (invoker instanceof HTMLButtonElement) invoker.disabled = true;
    const removing = this.state.removePrinter(printer.id);
    this.renderLiveRegions();
    await removing;
    this.render();
    (
      findFleetFocusControl(this.content, invoker.dataset.fleetFocus ?? "") ??
      findFleetFocusControl(this.content, "active-printer") ??
      findFleetFocusControl(this.content, "add-printer")
    )?.focus();
  }

  private openPrinterDialog(printer?: ManagedPrinter, invoker?: HTMLElement): void {
    const draft = createPrinterDraft(printer);
    const formId = this.dialogId("printer");
    const { form, error } = this.beginDialog(
      printer ? `Edit ${printer.name}` : "Add printer",
      formId,
      invoker,
    );
    const name = this.textField(form, formId, "name", "Printer name", draft.name, 100);
    name.required = true;
    const make = this.textField(form, formId, "make", "Manufacturer", draft.make ?? "", 100);
    const model = this.textField(form, formId, "model", "Model", draft.model, 100);
    const printerType = this.textField(
      form,
      formId,
      "printer-type",
      "Printer type",
      draft.printerType ?? "",
      100,
    );
    const firmware = this.textField(
      form,
      formId,
      "firmware",
      "Firmware",
      draft.firmware ?? "",
      100,
    );
    const bed = document.createElement("fieldset");
    bed.className = "printer-fleet-bed-fields";
    const bedLegend = document.createElement("legend");
    bedLegend.textContent = "Printable volume (mm)";
    const width = this.numberField(
      bed,
      formId,
      "bed-width",
      "Width (mm)",
      draft.bedMm[0],
      1,
      2_000,
    );
    const depth = this.numberField(
      bed,
      formId,
      "bed-depth",
      "Depth (mm)",
      draft.bedMm[1],
      1,
      2_000,
    );
    const height = this.numberField(
      bed,
      formId,
      "bed-height",
      "Height (mm)",
      draft.bedMm[2],
      1,
      2_000,
    );
    bed.prepend(bedLegend);

    const connectionLabel = document.createElement("label");
    connectionLabel.htmlFor = `${formId}-connection-kind`;
    connectionLabel.textContent = "Connection type";
    const connectionKind = document.createElement("select");
    connectionKind.id = connectionLabel.htmlFor;
    connectionKind.add(new Option("Local", "local"));
    connectionKind.add(new Option("Network address", "network"));
    connectionKind.value = draft.connection.kind;
    connectionKind.dataset.fleetFocus = "printer-connection-kind";
    form.append(connectionLabel, connectionKind);

    const addressLabel = document.createElement("label");
    addressLabel.htmlFor = `${formId}-address`;
    addressLabel.textContent = "Printer address (no username or password)";
    const address = document.createElement("input");
    address.type = "text";
    address.inputMode = "url";
    address.id = addressLabel.htmlFor;
    address.maxLength = 250;
    address.placeholder = "http://printer.local:8080";
    address.value = draft.connection.kind === "network" ? draft.connection.address : "";
    address.dataset.fleetFocus = "printer-address";
    const addressHint = document.createElement("p");
    addressHint.id = `${formId}-address-hint`;
    addressHint.className = "hint";
    addressHint.textContent = "Credentials are not collected or saved by this form.";
    address.setAttribute("aria-describedby", addressHint.id);
    const addressGroup = document.createElement("div");
    addressGroup.append(addressLabel, address, addressHint);
    const approveLabel = document.createElement("label");
    const approve = document.createElement("input");
    approve.type = "checkbox";
    approve.checked = draft.endpointApproved;
    approve.dataset.fleetFocus = "printer-endpoint-approval";
    approveLabel.append(
      approve,
      document.createTextNode(" Allow connection tests for this address"),
    );
    addressGroup.append(approveLabel);
    const originalAddress =
      printer?.connection.kind === "network" ? normalizeAddress(printer.connection.address) : null;
    address.addEventListener("input", () => {
      if (originalAddress === null || normalizeAddress(address.value) !== originalAddress) {
        approve.checked = false;
      }
    });
    const updateAddressVisibility = () => {
      addressGroup.hidden = connectionKind.value !== "network";
      address.required = connectionKind.value === "network";
      approve.disabled = connectionKind.value !== "network";
    };
    connectionKind.addEventListener("change", updateAddressVisibility);
    updateAddressVisibility();
    form.append(bed, addressGroup);
    const notesLabel = document.createElement("label");
    notesLabel.htmlFor = `${formId}-notes`;
    notesLabel.textContent = "Notes";
    const notes = document.createElement("textarea");
    notes.id = notesLabel.htmlFor;
    notes.rows = 3;
    notes.maxLength = MAX_NOTES_LENGTH;
    notes.value = draft.notes;
    notes.dataset.fleetFocus = "printer-notes";
    form.append(notesLabel, notes);

    const save = fleetButton(printer ? "Save printer" : "Add printer");
    save.type = "submit";
    form.append(save);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void (async () => {
        const next: PrinterDraft = {
          id: draft.id,
          metadataVersion: draft.metadataVersion,
          name: name.value,
          make: make.value || null,
          model: model.value,
          printerType: printerType.value || null,
          firmware: firmware.value || null,
          webUrl: draft.webUrl,
          notes: notes.value,
          bedMm: [Number(width.value), Number(depth.value), Number(height.value)],
          connection:
            connectionKind.value === "network"
              ? { kind: "network", address: address.value }
              : { kind: "local" },
          endpointApproved: resolveEndpointApproval(
            connectionKind.value === "network",
            address.value,
            originalAddress,
            draft.endpointApproved,
            approve.checked,
          ),
        };
        const problem = validatePrinterDraft(next);
        if (problem) {
          error.textContent = problem;
          return;
        }
        save.disabled = true;
        const saving = this.state.savePrinter(next);
        this.renderLiveRegions();
        if (await saving) {
          this.closeDialog();
          this.render();
        } else {
          error.textContent = this.state.error;
          save.disabled = false;
          this.renderLiveRegions();
        }
      })();
    });
    this.showDialog();
  }

  private openMaintenanceLogDialog(
    printerId: string,
    existing?: MaintenanceLog,
    invoker?: HTMLElement,
  ): void {
    const draft = createMaintenanceLogDraft(printerId, existing);
    const formId = this.dialogId("maintenance-log");
    const { form, error } = this.beginDialog(
      existing ? "Edit maintenance log" : "Log maintenance",
      formId,
      invoker,
    );
    const task = this.textField(form, formId, "task", "Maintenance task", draft.task, 120);
    task.required = true;
    const date = this.textField(
      form,
      formId,
      "performed-at",
      "Date performed",
      draft.performedAt,
      10,
      "date",
    );
    date.required = true;
    const notesLabel = document.createElement("label");
    notesLabel.htmlFor = `${formId}-notes`;
    notesLabel.textContent = "Notes";
    const notes = document.createElement("textarea");
    notes.id = notesLabel.htmlFor;
    notes.maxLength = MAX_NOTES_LENGTH;
    notes.rows = 4;
    notes.value = draft.notes;
    notes.dataset.fleetFocus = "maintenance-notes";
    form.append(notesLabel, notes);
    const save = fleetButton(existing ? "Save log" : "Save maintenance log");
    save.type = "submit";
    form.append(save);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void (async () => {
        const next = { ...draft, task: task.value, performedAt: date.value, notes: notes.value };
        const problem = validateMaintenanceLogDraft(next);
        if (problem) {
          error.textContent = problem;
          return;
        }
        save.disabled = true;
        const saving = this.state.saveMaintenanceLog(next);
        this.renderLiveRegions();
        if (await saving) {
          this.closeDialog();
          this.render();
        } else {
          error.textContent = this.state.error;
          save.disabled = false;
          this.renderLiveRegions();
        }
      })();
    });
    this.showDialog();
  }

  private openReminderDialog(
    printerId: string,
    existing?: MaintenanceReminder,
    invoker?: HTMLElement,
  ): void {
    const draft = createMaintenanceReminderDraft(printerId, existing);
    const formId = this.dialogId("maintenance-reminder");
    const { form, error } = this.beginDialog(
      existing ? "Edit maintenance reminder" : "Add maintenance reminder",
      formId,
      invoker,
    );
    const task = this.textField(form, formId, "task", "Reminder task", draft.task, 120);
    task.required = true;
    const dueAt = this.textField(form, formId, "due-at", "Due date", draft.dueAt, 10, "date");
    dueAt.required = true;
    const repeatLabel = document.createElement("label");
    repeatLabel.htmlFor = `${formId}-repeat-days`;
    repeatLabel.textContent = "Repeat every (days, optional)";
    const repeat = document.createElement("input");
    repeat.id = repeatLabel.htmlFor;
    repeat.type = "number";
    repeat.min = "1";
    repeat.max = "3650";
    repeat.step = "1";
    repeat.value = draft.repeatEveryDays === null ? "" : String(draft.repeatEveryDays);
    repeat.dataset.fleetFocus = "reminder-repeat";
    form.append(repeatLabel, repeat);
    const save = fleetButton(existing ? "Save reminder" : "Save reminder");
    save.type = "submit";
    form.append(save);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void (async () => {
        const next = {
          ...draft,
          task: task.value,
          dueAt: dueAt.value,
          repeatEveryDays: repeat.value ? Number(repeat.value) : null,
        };
        const problem = validateMaintenanceReminderDraft(next);
        if (problem) {
          error.textContent = problem;
          return;
        }
        save.disabled = true;
        const saving = this.state.saveMaintenanceReminder(next);
        this.renderLiveRegions();
        if (await saving) {
          this.closeDialog();
          this.render();
        } else {
          error.textContent = this.state.error;
          save.disabled = false;
          this.renderLiveRegions();
        }
      })();
    });
    this.showDialog();
  }

  private beginDialog(
    titleText: string,
    formId: string,
    invoker?: HTMLElement,
  ): { form: HTMLFormElement; error: HTMLElement } {
    this.dialogRevision += 1;
    for (const input of this.dialog.querySelectorAll("input")) input.value = "";
    this.dialog.replaceChildren();
    this.dialog.setAttribute("aria-labelledby", `${formId}-title`);
    this.dialogInvoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.id = `${formId}-title`;
    title.textContent = titleText;
    const close = fleetButton("Cancel", () => this.closeDialog());
    header.append(title, close);
    const form = document.createElement("form");
    form.id = formId;
    form.addEventListener("submit", (event) => event.preventDefault());
    const error = document.createElement("div");
    error.className = "settings-error";
    error.id = `${formId}-error`;
    error.setAttribute("role", "alert");
    form.setAttribute("aria-describedby", error.id);
    this.dialog.append(header, form, error);
    return { form, error };
  }

  private showDialog(): void {
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLInputElement>("input")?.focus();
  }

  private closeDialog(): void {
    this.dialogRevision += 1;
    for (const input of this.dialog.querySelectorAll("input")) input.value = "";
    if (this.dialog.open) this.dialog.close();
    this.dialog.replaceChildren();
    const invoker = this.dialogInvoker;
    this.dialogInvoker = null;
    const focusTarget = invoker?.isConnected
      ? invoker
      : invoker?.dataset.fleetFocus
        ? findFleetFocusControl(this.content, invoker.dataset.fleetFocus)
        : null;
    focusTarget?.focus();
  }

  private textField(
    form: HTMLFormElement,
    formId: string,
    key: string,
    labelText: string,
    value: string,
    maxLength: number,
    type = "text",
  ): HTMLInputElement {
    const id = `${formId}-${key}`;
    const label = document.createElement("label");
    label.htmlFor = id;
    label.textContent = labelText;
    const input = document.createElement("input");
    input.type = type;
    input.id = id;
    input.value = value;
    input.maxLength = maxLength;
    input.dataset.fleetFocus = key;
    form.append(label, input);
    return input;
  }

  private numberField(
    parent: HTMLElement,
    formId: string,
    key: string,
    labelText: string,
    value: number,
    min: number,
    max: number,
  ): HTMLInputElement {
    const id = `${formId}-${key}`;
    const label = document.createElement("label");
    label.htmlFor = id;
    label.textContent = labelText;
    const input = document.createElement("input");
    input.type = "number";
    input.id = id;
    input.value = String(value);
    input.min = String(min);
    input.max = String(max);
    input.step = "0.1";
    input.required = true;
    input.dataset.fleetFocus = key;
    parent.append(label, input);
    return input;
  }

  private dialogId(prefix: string): string {
    return `printer-fleet-${prefix}-${++this.nextDialogId}`;
  }

  private async completeReminder(id: string): Promise<void> {
    const completing = this.state.completeMaintenanceReminder(id);
    this.renderLiveRegions();
    await completing;
    this.render();
  }

  private async confirmDeleteLog(log: MaintenanceLog, invoker: HTMLElement): Promise<void> {
    if (!window.confirm(`Remove the ${log.task} maintenance log?`)) return;
    if (invoker instanceof HTMLButtonElement) invoker.disabled = true;
    const deleting = this.state.deleteMaintenanceLog(log.id, log.metadataVersion);
    this.renderLiveRegions();
    await deleting;
    this.render();
    (
      findFleetFocusControl(this.content, invoker.dataset.fleetFocus ?? "") ??
      findFleetFocusControl(this.content, `maintenance:${log.printerId}`)
    )?.focus();
  }

  private async confirmDeleteReminder(
    reminder: MaintenanceReminder,
    invoker: HTMLElement,
  ): Promise<void> {
    if (!window.confirm(`Remove the ${reminder.task} reminder?`)) return;
    if (invoker instanceof HTMLButtonElement) invoker.disabled = true;
    const deleting = this.state.deleteMaintenanceReminder(reminder.id);
    this.renderLiveRegions();
    await deleting;
    this.render();
    (
      findFleetFocusControl(this.content, invoker.dataset.fleetFocus ?? "") ??
      findFleetFocusControl(this.content, `maintenance:${reminder.printerId}`)
    )?.focus();
  }

  private async confirmCompleteReminder(
    reminder: MaintenanceReminder,
    control: HTMLButtonElement,
  ): Promise<void> {
    control.disabled = true;
    await this.completeReminder(reminder.id);
    const replacement =
      findFleetFocusControl(this.content, `edit-reminder:${reminder.id}`) ??
      findFleetFocusControl(this.content, `maintenance:${reminder.printerId}`);
    replacement?.focus();
  }

  private renderLiveRegions(): void {
    this.statusRegion.textContent = this.state.status;
    this.errorRegion.textContent = this.state.error;
  }
}

function findFleetFocusControl(root: HTMLElement, key: string): HTMLElement | null {
  return (
    [...root.querySelectorAll<HTMLElement>("[data-fleet-focus]")].find(
      (control) => control.dataset.fleetFocus === key,
    ) ?? null
  );
}

function fleetButton(label: string, action?: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn";
  button.textContent = label;
  if (action) button.addEventListener("click", action);
  return button;
}

function cloneFleetModel(model: PrinterFleetModel): PrinterFleetModel {
  return {
    managedPrinters: model.managedPrinters.map(cloneManagedPrinter),
    slicerDetections: model.slicerDetections.map((printer) => ({
      ...printer,
      bedMm: [...printer.bedMm],
    })),
    activePrinterId: model.activePrinterId,
    maintenanceLogs: model.maintenanceLogs.map((log) => ({ ...log })),
    maintenanceReminders: model.maintenanceReminders.map((reminder) => ({ ...reminder })),
  };
}

function printerMatchesConnectionSnapshot(
  current: ManagedPrinter | undefined,
  expected: ManagedPrinter,
): boolean {
  return (
    current !== undefined &&
    current.id === expected.id &&
    current.metadataVersion === expected.metadataVersion &&
    current.endpointApproved === expected.endpointApproved &&
    current.make === expected.make &&
    current.model === expected.model &&
    current.connection.kind === expected.connection.kind &&
    (current.connection.kind !== "network" ||
      (expected.connection.kind === "network" &&
        current.connection.address === expected.connection.address))
  );
}

function cloneManagedPrinter(printer: ManagedPrinter): ManagedPrinter {
  return {
    ...printer,
    bedMm: [...printer.bedMm],
    connection: safeConnectionCopy(printer.connection),
  };
}

function clonePrinterDraft(draft: PrinterDraft): PrinterDraft {
  return {
    ...draft,
    bedMm: [...draft.bedMm],
    connection: safeConnectionCopy(draft.connection),
  };
}

function safeConnectionCopy(connection: PrinterConnection): PrinterConnection {
  if (connection.kind === "local") return { kind: "local" };
  return { kind: "network", address: redactAddressCredentials(connection.address) };
}

function suggestionToDraft(suggestion: PrinterDiscoverySuggestion): PrinterDraft {
  const deviceMetadata = discoveryDeviceMetadata(suggestion.source);
  return {
    id: null,
    metadataVersion: null,
    name: suggestion.name,
    make: deviceMetadata.make,
    model: suggestion.model,
    printerType: suggestion.printerType,
    firmware: deviceMetadata.firmware,
    webUrl: suggestion.connection.kind === "network" ? suggestion.connection.address : null,
    notes: "",
    bedMm: [...suggestion.bedMm],
    connection: { ...suggestion.connection },
    endpointApproved: false,
  };
}

function discoveryDeviceMetadata(source: string): { make: string; firmware: string } {
  switch (source) {
    case "bambu_mqtt_tls":
      return { make: "Bambu Lab", firmware: "Bambu OS" };
    case "prusalink":
      return { make: "Prusa", firmware: "PrusaLink" };
    case "moonraker":
      return { make: "Klipper", firmware: "Klipper" };
    default:
      return { make: "", firmware: "" };
  }
}

function validNetworkAddress(address: string): boolean {
  const value = address.trim();
  if (!value || value.length > 250 || /\s/.test(value)) return false;
  try {
    const url = new URL(value.includes("://") ? value : `http://${value}`);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      Boolean(url.hostname) &&
      !url.username &&
      !url.password &&
      url.pathname === "/" &&
      !url.search &&
      !url.hash
    );
  } catch {
    return false;
  }
}

function redactAddressCredentials(address: string): string {
  try {
    const url = new URL(address.includes("://") ? address : `http://${address}`);
    if (!url.username && !url.password) return address;
    url.username = "";
    url.password = "";
    return url.toString().replace(/\/$/, "");
  } catch {
    return address.replace(/^(https?:\/\/)[^/@\s]*@/i, "$1");
  }
}

function normalizeAddress(address: string): string {
  return redactAddressCredentials(address).trim().replace(/\/$/, "").toLowerCase();
}

function isIPv4Address(address: string): boolean {
  const segments = address.split(".");
  return (
    segments.length === 4 &&
    segments.every((segment) => /^\d{1,3}$/.test(segment) && Number(segment) <= 255)
  );
}

function isPrivateIPv4Address(address: string): boolean {
  const [first, second] = address.split(".").map(Number);
  return (
    first === 10 ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 192 && second === 168)
  );
}

interface NumericIPv4Range {
  start: number;
  end: number;
}

function parseDiscoveryRanges(
  value: string,
): { ranges: NumericIPv4Range[]; addressCount: number } | null {
  const terms = value.split(",").map((term) => term.trim());
  if (!terms.length || terms.length > 64 || terms.some((term) => !term)) return null;
  const ranges: NumericIPv4Range[] = [];
  let addressCount = 0;
  for (const term of terms) {
    const cidr = term.split("/");
    let start: number;
    let end: number;
    let count: number;
    if (cidr.length === 2) {
      if (!isIPv4Address(cidr[0]) || !/^\d{1,2}$/.test(cidr[1])) return null;
      const prefix = Number(cidr[1]);
      if (prefix < 0 || prefix > 32) return null;
      const size = 2 ** (32 - prefix);
      const network = ipv4Number(cidr[0]);
      const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
      start = (network & mask) >>> 0;
      end = start + size - 1;
      count = prefix <= 30 ? size - 2 : size;
    } else if (term.includes("-")) {
      const endpoints = term.split("-").map((part) => part.trim());
      if (endpoints.length !== 2 || !endpoints.every(isIPv4Address)) return null;
      start = ipv4Number(endpoints[0]);
      end = ipv4Number(endpoints[1]);
      if (end < start) return null;
      count = end - start + 1;
    } else {
      if (!isIPv4Address(term)) return null;
      start = ipv4Number(term);
      end = start;
      count = 1;
    }
    if (!isPrivateIPv4Address(ipv4Text(start)) || !isPrivateIPv4Address(ipv4Text(end))) {
      return null;
    }
    addressCount += count;
    if (addressCount > MAX_DISCOVERY_ADDRESSES) return { ranges, addressCount };
    ranges.push({ start, end });
  }
  return { ranges, addressCount };
}

function ipv4Text(value: number): string {
  return `${(value >>> 24) & 255}.${(value >>> 16) & 255}.${(value >>> 8) & 255}.${value & 255}`;
}

function isSelectablePrinterId(model: PrinterFleetModel, id: string): boolean {
  return (
    id === GENERIC_PRINTER_ID ||
    model.managedPrinters.some((printer) => printer.id === id) ||
    model.slicerDetections.some((printer) => printer.id === id)
  );
}

export function connectionSnapshotCopy(printer: ManagedPrinter, now = Date.now()): string {
  const checked = printer.lastCheckedAt ? Date.parse(printer.lastCheckedAt) : NaN;
  const age = Number.isFinite(checked) ? now - checked : Number.POSITIVE_INFINITY;
  if (!Number.isFinite(checked)) return "Status snapshot: not checked yet.";
  const checkedLabel = new Date(checked).toLocaleString();
  const stale = age > CONNECTION_SNAPSHOT_STALE_AFTER_MS;
  return `${stale ? "Stale status snapshot" : "Status snapshot"}: ${connectionStatusLabel(printer.connectionStatus)} · job ${jobStatusLabel(printer.jobStatus)} · checked ${checkedLabel}${stale ? " · test again to refresh" : ""}.`;
}

export function requiresElegooMainboardId(
  printer: Pick<ManagedPrinter, "make" | "model">,
): boolean {
  const make = printer.make?.trim().toLowerCase() ?? "";
  const model = printer.model.trim().toLowerCase();
  return (
    make.includes("elegoo") && model.includes("centauri carbon") && !model.includes("carbon 2")
  );
}

function connectionStatusLabel(status: ManagedPrinter["connectionStatus"]): string {
  switch (status) {
    case "online":
      return "online";
    case "offline":
      return "offline";
    case "auth_required":
      return "authentication required";
    case "error":
      return "error";
    default:
      return "unknown";
  }
}

function jobStatusLabel(status: ManagedPrinter["jobStatus"]): string {
  switch (status) {
    case "idle":
      return "idle";
    case "printing":
      return "printing";
    case "paused":
      return "paused";
    case "complete":
      return "complete";
    default:
      return "unknown";
  }
}

function cleanOptional(value: string | null): string | null {
  const cleaned = value?.trim();
  return cleaned ? cleaned : null;
}

function suggestionAddressWithinRange(
  suggestion: PrinterDiscoverySuggestion,
  range: string,
): boolean {
  if (validateDiscoveryRange(range) || suggestion.connection.kind !== "network") return false;
  let address: string;
  try {
    const url = new URL(
      suggestion.connection.address.includes("://")
        ? suggestion.connection.address
        : `http://${suggestion.connection.address}`,
    );
    if (url.username || url.password) return false;
    address = url.hostname;
  } catch {
    return false;
  }
  if (!isIPv4Address(address) || !isPrivateIPv4Address(address)) return false;
  const parsed = parseDiscoveryRanges(range);
  if (!parsed) return false;
  const candidate = ipv4Number(address);
  return parsed.ranges.some((item) => candidate >= item.start && candidate <= item.end);
}

function ipv4Number(address: string): number {
  return address
    .split(".")
    .map(Number)
    .reduce((value, part) => ((value << 8) | part) >>> 0, 0);
}

function isDateOnly(value: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(`${value}T00:00:00.000Z`);
  return !Number.isNaN(date.getTime()) && date.toISOString().slice(0, 10) === value;
}

function upsertById<T extends { id: string }>(items: T[], updated: T): T[] {
  return items.some((item) => item.id === updated.id)
    ? items.map((item) => (item.id === updated.id ? { ...updated } : item))
    : [...items, { ...updated }];
}

function errorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message
    .replace(/(https?:\/\/)[^\s/@]+:[^\s/@]*@/gi, "$1[redacted]@")
    .replace(
      /\b(password|passphrase|secret|token|authorization|api[_-]?key)\b\s*[:=]\s*["']?[^\s,;"'}]+/gi,
      "$1=[redacted]",
    )
    .replace(/\b(Bearer|Basic)\s+\S+/gi, "$1 [redacted]")
    .slice(0, 300);
}
