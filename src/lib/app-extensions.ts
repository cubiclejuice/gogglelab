import type { LibraryFolder, LibraryModel, ModelSummary, SlicerPalette } from "./ipc";
import type { ColorResolution, ColorTarget, ColorwayMode, ResolveColor } from "./color-types";

/** Color rendering behavior supplied by the Community app or a licensed extension. */
export interface ColorFeature {
  readonly root: HTMLElement;
  readonly mode: ColorwayMode;
  readonly resolveColor: ResolveColor;
  onChange: () => void;
  load(summary: ModelSummary): void;
  reset(generation?: number): void;
  selectPlate(plateId: number): void;
  setMode(mode: ColorwayMode): void;
  canUseActivePalette?(): boolean;
  setActivePalette?(palette: SlicerPalette | null): void;
  setEntitlements?(entitlements: ProEntitlements): void;
  selectTarget?(target: ColorTarget): void;
  assignColor?(target: ColorTarget, color: string): boolean;
  clearColor?(target: ColorTarget): void;
  dispose(): void;
}

export interface LibraryPanelFeature {
  readonly isOpen: boolean;
  onImport: () => void;
  onOpenModel: (modelId: string) => void;
  show(invoker?: HTMLElement): void;
  hide(): void;
  render(folders: LibraryFolder[], models: LibraryModel[]): void;
  setStatus(message: string): void;
  setLocked?(locked: boolean): void;
}

export interface LibraryService {
  isEnabled(): boolean;
  listFolders(): Promise<LibraryFolder[]>;
  listModels(): Promise<LibraryModel[]>;
  importFile(path: string): Promise<unknown>;
  openModel(modelId: string, generation: number): Promise<ModelSummary>;
}

export interface ProEntitlements {
  library: boolean;
  colorways: boolean;
  slicerHandoff: boolean;
  revision: number;
}

export interface AppExtensions {
  createColorFeature(): ColorFeature;
  createLibraryPanel?(container: HTMLElement): LibraryPanelFeature | null;
  createSettingsSection?: () => HTMLElement | null;
  onZipExtracted?: (root: string, zipPath: string) => Promise<void>;
  createModelsFolder?: (root: string, name: string) => Promise<string>;
  readonly library?: LibraryService;
  getSlicerPalette?(slicerId: string): Promise<SlicerPalette | null>;
  requestHandoff?(generation: number, slicerId: string): Promise<void>;
  onHandoffLocked?(): void;
  currentEntitlements?(): ProEntitlements;
  subscribeEntitlements?(listener: (entitlements: ProEntitlements) => void): () => void;
  mountLicenseControl?(
    container: HTMLElement,
    addFooterControl: (element: HTMLElement) => void,
  ): void;
  dispose?(): void;
}

export type { ColorResolution };
