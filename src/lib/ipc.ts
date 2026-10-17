// The only file that calls invoke() (spec §2, S4). Types here mirror
// src-tauri/src/types.rs and src-tauri/src/error.rs field-for-field —
// keep them in sync by hand; there is no shared schema generator in v1.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeFusionPreview, type FusionPreviewSource } from "./fusion-preview";
import {
  decodeGeometryBuffer,
  normalizeArrayBuffer as toArrayBuffer,
  type DecodedGeometry,
} from "./geometry-wire";

export type StlFormat = "binary" | "ascii" | "3mf" | "step" | "f3d";

export interface Bbox {
  min: [number, number, number];
  max: [number, number, number];
}

export interface GeometryStats {
  bbox: Bbox;
  dimensions: [number, number, number];
  surface_area_mm2: number;
  volume_mm3: number;
}

export interface PartSummary {
  id: string;
  name: string;
  triangle_start: number;
  triangle_count: number;
  material_id: string | null;
  filament_slot: number | null;
}

export type MaterialSource = "3mf_base_material" | "3mf_color_group";

export interface MaterialSummary {
  id: string;
  name: string | null;
  color: string | null;
  source: MaterialSource;
}

export interface EmbeddedFilamentSummary {
  slot: number;
  name: string | null;
  material: string | null;
  color: string;
}

export interface PlateSummary {
  id: number;
  name: string;
  triangle_count: number;
  geometry: GeometryStats | null;
  parts: PartSummary[];
}

export interface ModelSummary {
  generation: number;
  file_name: string;
  file_size_bytes: number;
  format: StlFormat;
  triangle_count: number;
  geometry: GeometryStats | null;
  parse_ms: number;
  plates: PlateSummary[];
  materials: MaterialSummary[];
  embedded_filaments: EmbeddedFilamentSummary[];
  part_warning: string | null;
  plate_metadata: boolean;
  plate_warning: string | null;
  source_identity: string;
  source_digest: string;
}

export interface Integrity {
  boundary_edges: number;
  non_manifold_edges: number;
  inconsistent_orientation: number;
  degenerate_triangles: number;
  distinct_vertices: number;
  zero_normals: number;
  normal_disagreements: number;
  watertight: boolean;
}

export type IntegrityReport =
  | { status: "Computed"; generation: number; plate_id: number; integrity: Integrity }
  | {
      status: "SkippedTooLarge";
      generation: number;
      plate_id: number;
      triangle_count: number;
      limit: number;
    };

// Mirrors AppError's serde(tag = "kind") shape (src-tauri/src/error.rs).
export type AppError =
  | { kind: "NotFound" }
  | { kind: "NotReadable" }
  | { kind: "EmptyFile" }
  | { kind: "FileTooLarge"; size: number; limit: number }
  | { kind: "NotStl" }
  | { kind: "MalformedHeader" }
  | { kind: "TruncatedFile"; expected: number; actual: number }
  | { kind: "TooManyTriangles"; count: number; limit: number }
  | { kind: "NonFiniteCoordinate"; triangle_index: number }
  | { kind: "Malformed3mf"; detail: string }
  | { kind: "MalformedStep"; detail: string }
  | { kind: "Fusion"; message: string; setup_required: boolean }
  | { kind: "Superseded" }
  | { kind: "NoModelLoaded" }
  | { kind: "UnknownPlate" }
  | { kind: "Archive"; message: string }
  | { kind: "Watch"; message: string }
  | { kind: "OutsideRoot" }
  | { kind: "UnknownSlicer" }
  | { kind: "LaunchFailed"; status: number | null }
  | { kind: "EntitlementRequired"; feature: string }
  | { kind: "Library"; message: string };

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "kind" in e;
}

/** One specific, actionable message per AppError variant (spec §4, S8's
 * acceptance criterion) -- never a generic "something went wrong". */
export function describeError(e: AppError): string {
  switch (e.kind) {
    case "NotFound":
      return "File not found — it may have moved or been deleted.";
    case "NotReadable":
      return "Could not read this file. Check its permissions and try again.";
    case "EmptyFile":
      return "This file is empty (0 bytes).";
    case "FileTooLarge":
      return `This file is ${formatBytes(e.size)}, over the ${formatBytes(e.limit)} limit for this app.`;
    case "NotStl":
      return "This doesn't look like an STL or 3MF file.";
    case "MalformedHeader":
      return "This STL file's header is malformed or unreadable.";
    case "TruncatedFile":
      return `This STL file is truncated — expected ${formatBytes(e.expected)}, found ${formatBytes(e.actual)}.`;
    case "TooManyTriangles":
      return `This model has ${e.count.toLocaleString()} triangles, over the ${e.limit.toLocaleString()} limit for this app.`;
    case "NonFiniteCoordinate":
      return `This model contains an invalid (NaN or Infinity) coordinate at triangle ${e.triangle_index}.`;
    case "Malformed3mf":
      return `This 3MF file couldn't be read: ${e.detail}.`;
    case "MalformedStep":
      return `This STEP file couldn't be previewed: ${e.detail}.`;
    case "Fusion":
      return e.message;
    case "Superseded":
      return ""; // never shown -- a newer load already superseded this one
    case "NoModelLoaded":
      return "No model is loaded.";
    case "UnknownPlate":
      return "That plate is no longer available. Reopen the model and try again.";
    case "Archive":
      return e.message;
    case "Watch":
      return e.message;
    case "OutsideRoot":
      return "That folder is outside the one currently open.";
    case "UnknownSlicer":
      return "That slicer is no longer detected. Try reopening the app.";
    case "LaunchFailed":
      return e.status !== null
        ? `Could not launch the slicer (exit code ${e.status}).`
        : "Could not launch the slicer.";
    case "EntitlementRequired":
      return "A valid GoggleLab Pro license with this feature is required.";
    case "Library":
      return e.message;
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export { decodeGeometryBuffer };
export type { DecodedGeometry } from "./geometry-wire";

export interface Printer {
  id: string;
  name: string;
  bed_mm: [number, number, number];
}

export type AppearanceMode = "system" | "light" | "dark";

export interface Settings {
  appearance_mode: AppearanceMode;
  accent_color: string | null;
  default_color_preview: boolean;
  default_slicer: string | null;
  printers: Printer[];
  active_printer: string;
  display_units: "mm" | "in";
  default_folder: string | null;
}

export interface DetectedPrinter {
  id: string;
  name: string;
  bed_mm: [number, number, number];
  source_slicer: string;
  source_display: string;
}

/** Catalog-backed printer records use the camelCase serde contract. */
export type PrinterSourceKind = "manual" | "discovered" | "imported";
export type PrinterConnectionStatus = "unknown" | "online" | "offline" | "auth_required" | "error";
export type PrinterJobStatus = "unknown" | "idle" | "printing" | "paused" | "complete";
export type PrinterReminderStatus = "active" | "paused" | "completed";

export interface LibraryPrinter {
  id: string;
  metadataVersion: number;
  nickname: string;
  make: string | null;
  model: string | null;
  printerType: string | null;
  firmware: string | null;
  host: string | null;
  webUrl: string | null;
  endpointApproved: boolean;
  bedMm: [number, number, number] | null;
  notes: string;
  sourceKind: PrinterSourceKind;
  connectionStatus: PrinterConnectionStatus;
  jobStatus: PrinterJobStatus;
  lastCheckedAt: string | null;
  connectionErrorCode: string | null;
  printCount: number;
  lastPrintedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PrinterInput {
  nickname: string;
  make: string | null;
  model: string | null;
  printerType: string | null;
  firmware: string | null;
  host: string | null;
  webUrl: string | null;
  bedMm: [number, number, number] | null;
  notes: string;
}

export interface PrinterBatchInput {
  input: PrinterInput;
  endpointApproved: boolean;
}

export interface PrinterDiscoverySuggestion {
  key: string;
  name: string;
  model: string;
  bedMm: [number, number, number];
  connection: { kind: "network"; address: string };
  source: "moonraker" | "prusalink" | "bambu_mqtt_tls";
  printerType: "FDM";
}

export type PrinterConnectionTestResult =
  | "connected"
  | "unreachable"
  | "unsupported"
  | "authRequired"
  | "failed"
  | "cancelled";

export interface PrinterConnectionTestResponse {
  result: PrinterConnectionTestResult;
  printer: LibraryPrinter;
  statusEventObserved: boolean;
}

export interface PrinterMaintenanceInput {
  title: string;
  performedAt: string;
  notes: string;
}

export interface PrinterMaintenanceLog extends PrinterMaintenanceInput {
  id: string;
  metadataVersion: number;
  printerId: string | null;
  printerNickname: string;
  createdAt: string;
}

export interface PrinterReminderInput {
  title: string;
  notes: string;
  dueAt: string;
  intervalDays: number | null;
}

export interface PrinterReminder {
  id: string;
  metadataVersion: number;
  printerId: string | null;
  printerNickname: string;
  title: string;
  notes: string;
  dueAt: string;
  intervalDays: number | null;
  status: PrinterReminderStatus;
  lastCompletedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PrinterReminderCompletionInput {
  completedAt: string;
  notes: string;
}

export interface PrinterReminderCompletion {
  reminder: PrinterReminder;
  maintenanceLog: PrinterMaintenanceLog;
}

export interface PrinterDataImportPrinter {
  externalRef: string;
  mapToLocalPrinterId?: string | null;
  input: PrinterInput;
  createdAt?: string;
  updatedAt?: string;
}

export interface PrinterDataImportMaintenanceLog {
  externalRef: string;
  printerExternalRef: string | null;
  printerNickname: string;
  input: PrinterMaintenanceInput;
  createdAt?: string;
}

export interface PrinterDataImportReminder {
  externalRef: string;
  printerExternalRef: string | null;
  printerNickname?: string | null;
  input: PrinterReminderInput;
  status: PrinterReminderStatus;
  lastCompletedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PrinterDataImport {
  printers: PrinterDataImportPrinter[];
  maintenanceLogs: PrinterDataImportMaintenanceLog[];
  reminders: PrinterDataImportReminder[];
}

export interface PrinterDataImportResult {
  printerRefMap: { externalRef: string; localPrinterId: string }[];
  importedPrinterCount: number;
  mappedPrinterCount: number;
  importedMaintenanceLogCount: number;
  importedReminderCount: number;
}

export function libraryListPrinters(): Promise<LibraryPrinter[]> {
  return invoke("library_list_printers");
}

export function libraryCreatePrinter(
  input: PrinterInput,
  endpointApproved: boolean,
): Promise<LibraryPrinter> {
  return invoke("library_create_printer", { input, endpointApproved });
}

export function libraryUpdatePrinter(
  id: string,
  expectedVersion: number,
  input: PrinterInput,
  endpointApproved: boolean | null,
): Promise<LibraryPrinter> {
  return invoke("library_update_printer", { id, expectedVersion, input, endpointApproved });
}

export function libraryDeletePrinter(id: string, expectedVersion: number): Promise<void> {
  return invoke("library_delete_printer", { id, expectedVersion });
}

export function libraryCreatePrintersBatch(inputs: PrinterBatchInput[]): Promise<LibraryPrinter[]> {
  return invoke("library_create_printers_batch", { inputs });
}

export function libraryImportPrinterData(
  input: PrinterDataImport,
): Promise<PrinterDataImportResult> {
  return invoke("library_import_printer_data", { input });
}

export function libraryDiscoverPrinters(range: string): Promise<PrinterDiscoverySuggestion[]> {
  return invoke("library_discover_printers", { range });
}

export function libraryCancelPrinterDiscovery(): Promise<void> {
  return invoke("library_cancel_printer_discovery");
}

export function libraryTestPrinterConnection(
  id: string,
  mainboardId?: string,
  discoverMainboardId = false,
): Promise<PrinterConnectionTestResponse> {
  return invoke("library_test_printer_connection", {
    id,
    mainboardId: mainboardId ?? null,
    discoverMainboardId,
  });
}

export function libraryListPrinterMaintenanceLogs(
  printerId: string | null = null,
): Promise<PrinterMaintenanceLog[]> {
  return invoke("library_list_printer_maintenance_logs", { printerId });
}

export function libraryCreatePrinterMaintenanceLog(
  printerId: string,
  input: PrinterMaintenanceInput,
): Promise<PrinterMaintenanceLog> {
  return invoke("library_create_printer_maintenance_log", { printerId, input });
}

export function libraryUpdatePrinterMaintenanceLog(
  id: string,
  expectedVersion: number,
  input: PrinterMaintenanceInput,
): Promise<PrinterMaintenanceLog> {
  return invoke("library_update_printer_maintenance_log", { id, expectedVersion, input });
}

export function libraryDeletePrinterMaintenanceLog(
  id: string,
  expectedVersion: number,
): Promise<void> {
  return invoke("library_delete_printer_maintenance_log", { id, expectedVersion });
}

export function libraryListPrinterReminders(
  printerId: string | null = null,
  includeCompleted = true,
): Promise<PrinterReminder[]> {
  return invoke("library_list_printer_reminders", { printerId, includeCompleted });
}

export function libraryCreatePrinterReminder(
  printerId: string,
  input: PrinterReminderInput,
): Promise<PrinterReminder> {
  return invoke("library_create_printer_reminder", { printerId, input });
}

export function libraryUpdatePrinterReminder(
  id: string,
  expectedVersion: number,
  input: PrinterReminderInput,
): Promise<PrinterReminder> {
  return invoke("library_update_printer_reminder", { id, expectedVersion, input });
}

export function libraryDeletePrinterReminder(id: string, expectedVersion: number): Promise<void> {
  return invoke("library_delete_printer_reminder", { id, expectedVersion });
}

export function libraryCompletePrinterReminder(
  id: string,
  expectedVersion: number,
  completion: PrinterReminderCompletionInput,
): Promise<PrinterReminderCompletion> {
  return invoke("library_complete_printer_reminder", { id, expectedVersion, completion });
}

export function detectPrinters(): Promise<DetectedPrinter[]> {
  return invoke("detect_printers");
}

export function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export function setActivePrinter(printerId: string): Promise<void> {
  return invoke("set_active_printer", { printerId });
}

export function setAppearanceMode(mode: AppearanceMode): Promise<void> {
  return invoke("set_appearance_mode", { mode });
}

export function setDisplayUnits(units: "mm" | "in"): Promise<void> {
  return invoke("set_display_units", { units });
}

export function setAccentColor(color: string | null): Promise<void> {
  return invoke("set_accent_color", { color });
}

export function setDefaultColorPreview(enabled: boolean): Promise<void> {
  return invoke("set_default_color_preview", { enabled });
}

export function setDefaultFolder(folder: string | null): Promise<void> {
  return invoke("set_default_folder", { folder });
}

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

export function searchDir(root: string, query: string): Promise<DirEntry[]> {
  return invoke("search_dir", { root, query });
}

export interface ZipArchiveList {
  archives: DirEntry[];
  truncated: boolean;
}

export interface ModelFileList {
  models: DirEntry[];
  truncated: boolean;
}

export function listModelFiles(root: string): Promise<ModelFileList> {
  return invoke("list_model_files", { root });
}

export function listZipFiles(root: string): Promise<ZipArchiveList> {
  return invoke("list_zip_files", { root });
}

export function listDir(root: string, dir: string): Promise<DirEntry[]> {
  return invoke("list_dir", { root, dir });
}

export interface ExtractResult {
  destination: string;
  files: number;
}
export interface FolderChangeEvent {
  root: string;
  watch_id: number;
  error: string | null;
}

export function extractZip(root: string, path: string): Promise<ExtractResult> {
  return invoke("extract_zip", { root, path });
}

export interface FileTreeTrashReceipt {
  originalPath: string;
  recoveryPath: string;
  journalPath: string;
  bytes: number | null;
  sha256: string | null;
}

export function moveTreeFileToTrash(path: string): Promise<FileTreeTrashReceipt> {
  return invoke("move_tree_file_to_trash", { path });
}
export function moveTreeEntry(
  root: string,
  path: string,
  destinationDirectory: string,
): Promise<void> {
  return invoke("move_tree_entry", { root, path, destinationDirectory });
}
export function watchFolder(root: string | null, watchId: number): Promise<string | null> {
  return invoke("watch_folder", { root, watchId });
}
export function listenFolderChanges(
  callback: (event: FolderChangeEvent) => void,
): Promise<() => void> {
  return listen<FolderChangeEvent>("folder-changed", (event) => callback(event.payload));
}

export interface SlicerApp {
  id: string;
  bundle_id: string;
  display_name: string;
  app_path: string;
}

export interface SlicerProfileView {
  id: string;
  bundle_id: string;
  display_name: string;
  app_path: string;
  is_custom: boolean;
  available: boolean;
}

export interface SlicerProfileDraft {
  id: string | null;
  name: string;
  path: string;
  /** True only after the user explicitly confirms this executable identity. */
  trust: boolean;
}

export interface SlicerPalette {
  slicer_id: string;
  slots: ActiveFilamentSlot[];
}

export interface ActiveFilamentSlot {
  slot: number;
  name: string | null;
  color: string;
}

export function detectSlicers(): Promise<SlicerApp[]> {
  return invoke("detect_slicers");
}

export function listSlicerProfiles(): Promise<SlicerProfileView[]> {
  return invoke("list_slicer_profiles");
}

export function detectSlicerProfileCandidates(): Promise<SlicerProfileDraft[]> {
  return invoke("detect_slicer_profile_candidates");
}

export function saveSlicerProfiles(profiles: SlicerProfileDraft[]): Promise<void> {
  return invoke("save_slicer_profiles", { profiles });
}

/** Append explicitly selected, trusted local apps without replacing saved profiles. */
export function importSlicerProfiles(profiles: SlicerProfileDraft[]): Promise<string[]> {
  return invoke("import_slicer_profiles", { profiles });
}

export function getDefaultSlicer(): Promise<string | null> {
  return invoke("get_default_slicer");
}

export function setDefaultSlicer(slicerId: string | null): Promise<void> {
  return invoke("set_default_slicer", { slicerId });
}

export function openInSlicer(generation: number, slicerId: string): Promise<void> {
  return invoke("open_in_slicer", { generation, slicerId });
}

export function openModel(path: string, generation: number): Promise<ModelSummary> {
  return invoke("open_model", { path, generation });
}

export async function readStepFile(path: string, generation: number): Promise<ArrayBuffer> {
  const raw = await invoke<ArrayBuffer | Uint8Array | number[]>("read_step_file", {
    path,
    generation,
  });
  return toArrayBuffer(raw);
}

export function installFusionBridge(): Promise<void> {
  return invoke("install_fusion_bridge");
}

export async function readFusionFile(
  path: string,
  signal: AbortSignal,
): Promise<FusionPreviewSource> {
  const requestId = crypto.randomUUID().replaceAll("-", "");
  const cancel = () => {
    void invoke("cancel_fusion_preview", { requestId }).catch(() => {});
  };
  signal.addEventListener("abort", cancel, { once: true });
  try {
    if (signal.aborted) throw new DOMException("Fusion preview canceled", "AbortError");
    const raw = await invoke<ArrayBuffer | Uint8Array | number[]>("read_fusion_file", {
      path,
      requestId,
    });
    if (signal.aborted) throw new DOMException("Fusion preview canceled", "AbortError");
    return decodeFusionPreview(toArrayBuffer(raw));
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

export async function modelGeometry(
  generation: number,
  plateId?: number,
): Promise<DecodedGeometry> {
  const raw = await invoke<ArrayBuffer | Uint8Array | number[]>("model_geometry", {
    generation,
    plateId,
  });
  return decodeGeometryBuffer(raw);
}

export interface LibraryFolder {
  id: string;
  name: string;
  parent_id: string | null;
  model_count: number;
}

export interface LibraryModel {
  id: string;
  display_name: string;
  description: string;
  folder_id: string;
  original_name: string | null;
  format: string | null;
  bytes: number;
  content_hash: string | null;
  storage_path: string | null;
  metadata_version: number;
  created_at: string;
  updated_at: string;
  tags: string[];
  /** Managed entries are durable app-owned copies. Indexed entries remain at their source path. */
  source_kind?: "managed" | "indexed";
  source_path?: string | null;
  source_root_id?: string | null;
  source_missing?: boolean;
  /** Native index fingerprint in epoch nanoseconds; legacy catalogs may contain epoch seconds. */
  source_modified_at?: number | null;
  thumbnail_path?: string | null;
  metadata?: CoreMetadata;
  bundle_id?: string | null;
  source_archive_member?: string | null;
  filament_ids?: string[];
}

export interface Filament {
  id: string;
  version: number;
  brand: string;
  material: string;
  color: string;
  name: string;
  notes: string;
  origin: "manual" | "spoolman";
  resource_kind: "manual" | "filament" | "spool";
  source_id: string | null;
  external_id: string | null;
  parent_filament_id: string | null;
  archived: boolean;
  source_missing: boolean;
  created_at: string;
  updated_at: string;
}

export interface FilamentInput {
  brand: string;
  material: string;
  color: string;
  name: string;
  notes: string;
}

export interface SpoolmanConfig {
  baseUrl: string;
  endpointScope: "local" | "remote";
  pageSize: number;
  timeoutSeconds: number;
}

export interface SpoolmanPullResult {
  filamentsReceived: number;
  spoolsReceived: number;
  filamentsUpserted: number;
  spoolsUpserted: number;
  removedLocally: number;
  lastSuccessfulPullAt: string;
}

export interface AiProviderPublicConfig {
  id: string;
  label: string;
  kind: "openaiCompatible" | "anthropic" | "gemini";
  baseUrl: string | null;
  endpointScope: "local" | "remote";
  model: string;
  credentialStorage: "none" | "session";
  hasCredential: boolean;
  maxTags: number;
  detail: "brief" | "balanced" | "detailed";
  categoryMode: "none" | "existingOnly" | "allowNew";
  concurrency: number;
  timeoutSeconds: number;
  structuredOutput: "jsonSchema" | "jsonObject" | "plainJson";
  categorySharing: "local" | "provider";
}

export interface IntegrationSettings {
  spoolman: SpoolmanConfig | null;
  aiProviders: AiProviderPublicConfig[];
}

export interface AiSuggestion {
  suggestionId: string;
  name: string;
  category: string | null;
  confidence: number | null;
}

export interface AiSuggestionBatch {
  reviewToken: string;
  providerId: string;
  model: string;
  categorySharing: "local" | "provider";
  targets: Array<{
    modelId: string;
    expectedVersion: number;
    displayName: string;
    existingTags: Array<{ name: string; category: string | null }>;
    suggestions: AiSuggestion[];
  }>;
  failures: Array<{ modelId: string; code: string; message: string }>;
}

export type PrintStatus = "unprinted" | "want" | "queued" | "printing" | "printed" | "failed";

export interface CoreMetadata {
  designer?: string | null;
  source_url?: string | null;
  notes?: string | null;
  model_license?: string | null;
  print_status: PrintStatus;
  parent_model_id?: string | null;
  last_printed_at?: string | null;
  print_count: number;
  rating?: number;
  favorite?: boolean;
}

export interface LibraryTag {
  id: string;
  name: string;
  category?: string | null;
}

export interface MetadataPatch {
  designer?: string;
  source_url?: string;
  notes?: string;
  model_license?: string;
  print_status?: PrintStatus;
  tags?: string[];
  tag_mode?: "replace" | "merge";
  folder_id?: string;
  parent_model_id?: string;
  rating?: number;
  favorite?: boolean;
}

export interface VersionedModelTarget {
  model_id: string;
  expected_version: number;
}

export type PrintOutcome = "printed" | "failed" | "cancelled";

export interface PrintAttemptPartInput {
  id: string;
  /** Amount consumed for one printed copy. */
  quantity: number;
  /** Historical label for an imported association that has no local stock row. */
  snapshot?: {
    name: string;
    category: string | null;
    unit: string;
  };
}

export interface PrintAttemptPart {
  id: string | null;
  name: string;
  category: string | null;
  unit: string;
  quantityPerCopy: number;
  /** Total consumed across `PrintAttempt.quantity` copies. */
  quantity: number;
}

export interface PrintAttemptInput {
  printed_at: string;
  quantity: number;
  notes: string;
  filament_ids: string[];
  outcome: PrintOutcome;
  /** Optional for compatibility with older callers and stored event payloads. */
  parts?: PrintAttemptPartInput[];
}

export interface PrintAttempt extends Omit<PrintAttemptInput, "parts"> {
  id: string;
  model_id: string;
  printer_id?: string | null;
  printer_name?: string | null;
  /** Older history entries may not contain part snapshots. */
  parts?: PrintAttemptPart[];
  /** Null marks legacy part usage whose effect on stock cannot be reconstructed. */
  stock_adjusted?: boolean | null;
  created_at: string;
  updated_at: string;
}

export interface PartStockInput {
  name: string;
  category: string | null;
  quantity: number;
  unit: string;
  notes: string;
  lowStock: number;
}

export interface PartStock extends PartStockInput {
  id: string;
  metadataVersion: number;
  createdAt?: string;
  updatedAt?: string;
}

export interface PrintAttemptImportItem {
  input: PrintAttemptInput;
  createdAt: string | null;
  printerId: string | null;
  printerName: string | null;
}

export interface PrintAttemptImportResult {
  addedCount: number;
  skippedCount: number;
  metadataVersion: number;
}

export interface IndexRoot {
  id: string;
  path: string;
  enabled: boolean;
  last_scanned_at: string | null;
  scan_interval_minutes: number;
  max_file_bytes: number;
  scan_zip: boolean;
}

export interface LibraryScanStatus {
  running: boolean;
  cancelling: boolean;
  currentRootId: string | null;
  rootsTotal: number;
  rootsCompleted: number;
  filesVisited: number;
  filesIndexed: number;
  errors: string[];
  startedAt: string | null;
  finishedAt: string | null;
  jobKind: "scan" | "thumbnails" | null;
  thumbnailsTotal: number;
  thumbnailsCompleted: number;
}

export interface CatalogSettings {
  thumbnail_size: 128 | 256 | 512;
  thumbnail_background: string;
  thumbnail_concurrency: number;
  scan_batch_size: number;
}

export interface VersionedRemovalTarget {
  modelId: string;
  expectedVersion: number;
}

export interface DuplicateGroup {
  contentHash: string;
  models: LibraryModel[];
}

export interface RemovalResult {
  snapshotId: string;
  removedIds: string[];
}

export interface RemovedModel {
  model: LibraryModel;
  removedAt: string;
  snapshotId: string;
}

export interface TrashResult {
  snapshotId: string;
  modelId: string;
}

export interface BackupResult {
  path: string;
  files: number;
  bytes: number;
  sha256: string;
}

export interface RestoreResult {
  previousLibraryBackup: string;
  models: number;
  generation: number;
}

export interface CleanupResult {
  /** Hash-verified generated files permanently removed after cleanup admission. */
  files: number;
  bytes: number;
  issues: string[];
  handoffCleanupSkipped: boolean;
}

export interface ManagementStatus {
  maintenance: boolean;
  generation: number;
}

export interface BundlePreviewMember {
  modelId: string;
  displayName: string;
  triangleCount: number;
  offset: [number, number, number];
}

export interface BundlePreviewSummary {
  token: string;
  bundleId: string;
  memberCount: number;
  previewedCount: number;
  omittedCount: number;
  triangleCount: number;
  layout: "combined" | "grid";
  members: BundlePreviewMember[];
  notice: string | null;
}

export function libraryListFolders(): Promise<LibraryFolder[]> {
  return invoke("library_list_folders");
}

export function libraryListModels(query?: string): Promise<LibraryModel[]> {
  return invoke("library_list_models", {
    folderId: null,
    query: query ?? null,
  });
}

export function libraryImportFile(
  sourcePath: string,
  folderId = "root",
): Promise<{ model: LibraryModel; reused_blob: boolean }> {
  return invoke("library_import_file", {
    sourcePath,
    folderId,
    displayName: null,
  });
}

export function libraryOpenModel(modelId: string, generation: number): Promise<ModelSummary> {
  return invoke("library_open_model", { modelId, generation });
}

export function libraryListRoots(): Promise<IndexRoot[]> {
  return invoke("library_list_roots");
}

export function libraryAddRoot(path: string): Promise<IndexRoot> {
  return invoke("library_add_root", { path });
}

export function libraryUpdateRoot(root: IndexRoot): Promise<IndexRoot> {
  return invoke("library_update_root", {
    rootId: root.id,
    enabled: root.enabled,
    scanIntervalMinutes: root.scan_interval_minutes,
    maxFileBytes: root.max_file_bytes,
    scanZip: root.scan_zip,
  });
}

export function libraryRemoveRoot(rootId: string): Promise<void> {
  return invoke("library_remove_root", { rootId });
}

export function libraryStartScan(rootId: string | null = null): Promise<void> {
  return invoke("library_start_scan", { rootId });
}

export function libraryScanStatus(): Promise<LibraryScanStatus> {
  return invoke("library_scan_status");
}

export function libraryCancelScan(): Promise<void> {
  return invoke("library_cancel_scan");
}

export function libraryGetThumbnail(modelId: string, regenerate = false): Promise<string | null> {
  return invoke("library_get_thumbnail", { modelId, regenerate });
}

export function libraryPurgeThumbnails(): Promise<void> {
  return invoke("library_purge_thumbnails");
}

export function libraryStartThumbnailJob(regenerate: boolean): Promise<void> {
  return invoke("library_start_thumbnail_job", { regenerate });
}

export function libraryGetCatalogSettings(): Promise<CatalogSettings> {
  return invoke("library_get_catalog_settings");
}

export function librarySetCatalogSettings(settings: CatalogSettings): Promise<CatalogSettings> {
  return invoke("library_set_catalog_settings", { settings });
}

export function libraryListTags(): Promise<LibraryTag[]> {
  return invoke("library_list_tags");
}

export function libraryCreateTag(name: string, category: string | null): Promise<LibraryTag> {
  return invoke("library_create_tag", { name, category });
}

export function libraryUpdateTag(
  tagId: string,
  name: string,
  category: string | null,
): Promise<LibraryTag> {
  return invoke("library_update_tag", { tagId, name, category });
}

export function libraryDeleteTag(tagId: string): Promise<void> {
  return invoke("library_delete_tag", { tagId });
}

export function libraryUpdateModelMetadata(
  modelId: string,
  expectedVersion: number,
  patch: MetadataPatch,
): Promise<LibraryModel> {
  return invoke("library_update_model_metadata", { modelId, expectedVersion, patch });
}

export function libraryUpdateModelsMetadata(
  targets: VersionedModelTarget[],
  patch: MetadataPatch,
): Promise<LibraryModel[]> {
  return invoke("library_update_models_metadata", { targets, patch });
}

export function libraryCreatePrintAttempt(
  modelId: string,
  input: PrintAttemptInput,
): Promise<PrintAttempt> {
  return invoke("library_create_print_attempt", { modelId, input });
}

export function libraryCreatePrintAttemptWithPrinter(
  modelId: string,
  input: PrintAttemptInput,
  printerId: string | null,
): Promise<PrintAttempt> {
  return invoke("library_create_print_attempt_with_printer", { modelId, input, printerId });
}

export function libraryImportPrintAttempt(
  modelId: string,
  input: PrintAttemptInput,
  createdAt: string | null,
  printerId: string | null,
  printerName: string | null,
): Promise<PrintAttempt> {
  return invoke("library_import_print_attempt", {
    modelId,
    input,
    createdAt,
    printerId,
    printerName,
  });
}

export function libraryImportPrintAttempts(
  modelId: string,
  attempts: PrintAttemptImportItem[],
): Promise<PrintAttemptImportResult> {
  return invoke("library_import_print_attempts", { modelId, attempts });
}

export function libraryListPrintAttempts(modelId: string): Promise<PrintAttempt[]> {
  return invoke("library_list_print_attempts", { modelId });
}

export function libraryGetPrintAttempt(attemptId: string): Promise<PrintAttempt | null> {
  return invoke("library_get_print_attempt", { attemptId });
}

export function libraryUpdatePrintAttempt(
  attemptId: string,
  input: PrintAttemptInput,
): Promise<PrintAttempt> {
  return invoke("library_update_print_attempt", { attemptId, input });
}

export function libraryUpdatePrintAttemptWithPrinter(
  attemptId: string,
  input: PrintAttemptInput,
  printerId: string | null | undefined,
): Promise<PrintAttempt> {
  return invoke("library_update_print_attempt_with_printer", { attemptId, input, printerId });
}

export function libraryDeletePrintAttempt(attemptId: string): Promise<void> {
  return invoke("library_delete_print_attempt", { attemptId });
}

export function libraryListPartStock(): Promise<PartStock[]> {
  return invoke("library_list_part_stock");
}

export function libraryCreatePartStock(input: PartStockInput): Promise<PartStock> {
  return invoke("library_create_part_stock", { input });
}

export function libraryUpdatePartStock(
  id: string,
  expectedVersion: number,
  input: PartStockInput,
): Promise<PartStock> {
  return invoke("library_update_part_stock", { id, expectedVersion, input });
}

export function libraryDeletePartStock(id: string, expectedVersion: number): Promise<void> {
  return invoke("library_delete_part_stock", { id, expectedVersion });
}

export function libraryCreateBundle(modelIds: string[]): Promise<string> {
  return invoke("library_create_bundle", { modelIds });
}

export function librarySetModelBundle(
  modelId: string,
  bundleId: string | null,
): Promise<LibraryModel> {
  return invoke("library_set_model_bundle", { modelId, bundleId });
}

export function libraryCreateFolder(
  name: string,
  parentId: string | null = null,
): Promise<LibraryFolder> {
  return invoke("library_create_folder", { name, parentId });
}

export function libraryRenameFolder(id: string, name: string): Promise<LibraryFolder> {
  return invoke("library_rename_folder", { id, name });
}

export function libraryDeleteFolder(id: string): Promise<void> {
  return invoke("library_delete_folder", { id });
}

export function libraryDuplicateGroups(modelIds: string[]): Promise<DuplicateGroup[]> {
  return invoke("library_duplicate_groups", { modelIds });
}

export function libraryRemoveModels(targets: VersionedRemovalTarget[]): Promise<RemovalResult> {
  return invoke("library_remove_models", { targets });
}

export function libraryListRemovedModels(): Promise<RemovedModel[]> {
  return invoke("library_list_removed_models");
}

export function libraryRestoreRemoval(snapshotId: string): Promise<LibraryModel[]> {
  return invoke("library_restore_removal", { snapshotId });
}

export function libraryTrashLooseSource(
  modelId: string,
  expectedVersion: number,
): Promise<TrashResult> {
  return invoke("library_trash_loose_source", { modelId, expectedVersion });
}

export function libraryUndoTrash(snapshotId: string): Promise<LibraryModel> {
  return invoke("library_undo_trash", { snapshotId });
}

export function libraryRevealModelSource(
  modelId: string,
): Promise<{ path: string; archiveMember?: string | null }> {
  return invoke("library_reveal_model_source", { modelId });
}

export function libraryCreateBackup(destinationPath: string): Promise<BackupResult> {
  return invoke("library_create_backup", { destinationPath });
}

export function libraryRestoreBackup(sourcePath: string): Promise<RestoreResult> {
  return invoke("library_restore_backup", { sourcePath });
}

export function libraryPurgeCatalog(
  confirmation: string,
  destinationPath: string,
): Promise<BackupResult> {
  return invoke("library_purge_catalog", { confirmation, destinationPath });
}

export function libraryCleanupOrphanCache(): Promise<CleanupResult> {
  return invoke("library_cleanup_orphan_cache");
}

export function libraryManagementStatus(): Promise<ManagementStatus> {
  return invoke("library_management_status");
}

export function libraryPrepareBundlePreview(
  bundleId: string,
  generation: number,
): Promise<BundlePreviewSummary> {
  return invoke("library_prepare_bundle_preview", { bundleId, generation });
}

export async function libraryBundlePreviewGeometry(token: string): Promise<DecodedGeometry> {
  const raw = await invoke<ArrayBuffer | Uint8Array | number[]>("library_bundle_preview_geometry", {
    token,
  });
  return decodeGeometryBuffer(raw);
}

export function libraryCloseBundlePreview(token: string): Promise<void> {
  return invoke("library_close_bundle_preview", { token });
}

export function librarySendBundleToSlicer(
  bundleId: string,
  slicerId: string,
  token: string,
): Promise<void> {
  return invoke("library_send_bundle_to_slicer", { bundleId, slicerId, token });
}

export function librarySetCustomThumbnail(
  modelId: string,
  expectedVersion: number,
  path: string,
): Promise<LibraryModel> {
  return invoke("library_set_custom_thumbnail", { modelId, expectedVersion, path });
}

export function libraryClearCustomThumbnail(
  modelId: string,
  expectedVersion: number,
): Promise<LibraryModel> {
  return invoke("library_clear_custom_thumbnail", { modelId, expectedVersion });
}

export function libraryListFilaments(includeUnavailable = true): Promise<Filament[]> {
  return invoke("library_list_filaments", { includeUnavailable });
}

export function libraryCreateFilament(input: FilamentInput): Promise<Filament> {
  return invoke("library_create_filament", { input });
}

export function libraryUpdateFilament(
  id: string,
  expectedVersion: number,
  input: FilamentInput,
): Promise<Filament> {
  return invoke("library_update_filament", { id, expectedVersion, input });
}

export function libraryDeleteFilament(
  id: string,
  expectedVersion: number,
): Promise<Filament | null> {
  return invoke("library_delete_filament", { id, expectedVersion });
}

export function librarySetModelFilaments(
  modelId: string,
  expectedVersion: number,
  filamentIds: string[],
): Promise<LibraryModel> {
  return invoke("library_set_model_filaments", { modelId, expectedVersion, filamentIds });
}

export function libraryBulkSetModelFilaments(
  targets: VersionedModelTarget[],
  filamentIds: string[],
): Promise<LibraryModel[]> {
  return invoke("library_bulk_set_model_filaments", { targets, filamentIds });
}

export function libraryBulkAssignModelFilaments(
  assignments: Array<{ target: VersionedModelTarget; filament_ids: string[] }>,
): Promise<LibraryModel[]> {
  return invoke("library_bulk_assign_model_filaments", { assignments });
}

export function integrationGetSettings(): Promise<IntegrationSettings> {
  return invoke("integration_get_settings");
}

export function integrationSaveSpoolmanConfig(
  config: SpoolmanConfig | null,
): Promise<IntegrationSettings> {
  return invoke("integration_save_spoolman_config", { config });
}

export function spoolmanPull(): Promise<SpoolmanPullResult> {
  return invoke("spoolman_pull");
}

export function integrationSaveAiProvider(
  config: AiProviderPublicConfig,
  credentialUpdate: {
    action: "keep" | "clear" | "setSession";
    secret?: string;
  },
): Promise<AiProviderPublicConfig> {
  return invoke("integration_save_ai_provider", { config, credentialUpdate });
}

export function integrationDeleteAiProvider(providerId: string): Promise<void> {
  return invoke("integration_delete_ai_provider", { providerId });
}

export function aiSuggestTags(
  providerId: string,
  targets: VersionedModelTarget[],
): Promise<AiSuggestionBatch> {
  return invoke("ai_suggest_tags", { providerId, targets });
}

export function aiApplyTagReview(
  reviewToken: string,
  decisions: Array<{
    modelId: string;
    action: "accept" | "reject";
    acceptedSuggestionIds: string[];
  }>,
  strategy: "append" | "replace" | "merge",
): Promise<LibraryModel[]> {
  return invoke("ai_apply_tag_review", { reviewToken, decisions, strategy });
}

export function aiDiscardTagReview(reviewToken: string): Promise<void> {
  return invoke("ai_discard_tag_review", { reviewToken });
}

export interface DesktopLibraryServerStatus {
  running: boolean;
  baseUrl: string | null;
  mcpUrl: string | null;
  allowExternalIntegrations: boolean;
}

export function libraryServerStart(
  port?: number,
  allowExternalIntegrations = false,
): Promise<DesktopLibraryServerStatus> {
  return invoke("library_server_start", { port, allowExternalIntegrations });
}

export function libraryServerStop(): Promise<DesktopLibraryServerStatus> {
  return invoke("library_server_stop");
}

export function libraryServerStatus(): Promise<DesktopLibraryServerStatus> {
  return invoke("library_server_status");
}

export function libraryServerRevealAccessKey(): Promise<{ accessKey: string }> {
  return invoke("library_server_reveal_access_key");
}

export interface RecoveryManifest {
  manifest_version: number;
  library_schema_version: number;
  models: unknown[];
}

export function recoveryListLibrary(): Promise<RecoveryManifest> {
  return invoke("recovery_list_library");
}

export function recoveryExportLibrary(
  modelIds: string[],
  destinationPath: string,
): Promise<RecoveryManifest> {
  return invoke("recovery_export_library", { modelIds, destinationPath });
}

export function recoveryExportPrintventoryJson(destinationPath: string): Promise<number> {
  return invoke("recovery_export_printventory_json", { destinationPath });
}

export function recoveryExportColorways(rawJson: string, destinationPath: string): Promise<number> {
  return invoke("recovery_export_colorways", { rawJson, destinationPath });
}
