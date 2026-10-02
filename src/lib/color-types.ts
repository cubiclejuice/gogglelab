export type ColorwayMode = "model" | "color";

export type ColorTarget = { kind: "model" } | { kind: "part"; id: string };

export type ColorSource = "Manual" | "Embedded 3MF" | "Active slicer" | "GoggleLab default";

export interface ColorResolution {
  color: string | null;
  source: ColorSource;
}

export type ResolveColor = (target: ColorTarget) => ColorResolution;
