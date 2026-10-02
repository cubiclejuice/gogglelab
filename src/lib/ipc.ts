// The only file that calls invoke() (spec §2, S4). Types here mirror
// src-tauri/src/types.rs and src-tauri/src/error.rs field-for-field —
// keep them in sync by hand; there is no shared schema generator in v1.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeFusionPreview, type FusionPreviewSource } from "./fusion-preview";

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

/** Geometry buffer v2 magic bytes, in file order — see spec §2. */
const MAGIC = 0x324c5453; // the LE u32 whose bytes spell "STL2"

export interface DecodedGeometry {
  triangleCount: number;
  /** Non-indexed positions, length = 9 * triangleCount. */
  positions: Float32Array;
}

/**
 * `invoke()` for a command returning `tauri::ipc::Response` does not
 * reliably resolve to a real `ArrayBuffer` instance across platforms/IPC
 * transports (observed in practice: a plain `number[]`, caught by manual
 * testing in the built app -- `new DataView()` on it threw "Expected
 * ArrayBuffer for the first argument", something no Rust-side unit test
 * could have caught since it's purely a frontend marshaling question).
 * Normalizing through Uint8Array first accepts whatever shape actually
 * comes back.
 */
function toArrayBuffer(raw: ArrayBuffer | Uint8Array | number[]): ArrayBuffer {
  if (raw instanceof ArrayBuffer) return raw;
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

export function decodeGeometryBuffer(raw: ArrayBuffer | Uint8Array | number[]): DecodedGeometry {
  const buf = toArrayBuffer(raw);
  const head = new DataView(buf);
  const magic = head.getUint32(0, true);
  if (magic !== MAGIC) {
    throw new Error(`bad geometry buffer magic: 0x${magic.toString(16)}`);
  }
  const triangleCount = head.getUint32(8, true);
  const positions = new Float32Array(buf, 16, triangleCount * 9);
  return { triangleCount, positions };
}

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

export function moveTreeFileToTrash(path: string): Promise<void> {
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

export function getDefaultSlicer(): Promise<string | null> {
  return invoke("get_default_slicer");
}

export function setDefaultSlicer(slicerId: string): Promise<void> {
  return invoke("set_default_slicer", { slicerId });
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

export function recoveryExportColorways(rawJson: string, destinationPath: string): Promise<number> {
  return invoke("recovery_export_colorways", { rawJson, destinationPath });
}
