import type { ModelSummary, SlicerPalette } from "./ipc";
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

export interface ProEntitlements {
  library: boolean;
  colorways: boolean;
  slicerHandoff: boolean;
  revision: number;
}

export interface AppExtensions {
  createColorFeature(): ColorFeature;
  createSettingsSection?: () => HTMLElement | null;
  onZipExtracted?: (root: string, zipPath: string) => Promise<void>;
  createModelsFolder?: (root: string, name: string) => Promise<string>;
  getSlicerPalette?(slicerId: string): Promise<SlicerPalette | null>;
  currentEntitlements?(): ProEntitlements;
  subscribeEntitlements?(listener: (entitlements: ProEntitlements) => void): () => void;
  mountLicenseControl?(
    container: HTMLElement,
    addFooterControl: (element: HTMLElement) => void,
  ): void;
  dispose?(): void;
}

export type { ColorResolution };
