export type CadPreviewFormat = "step" | "f3d";

export function isCadPreview(format: string): format is CadPreviewFormat {
  return format === "step" || format === "f3d";
}

export function cadPreviewLabel(format: CadPreviewFormat): string {
  return format === "f3d" ? "Fusion preview" : "STEP preview";
}
