import type { ModelSummary, PartSummary } from "./ipc";

export type AuthoredColorTarget = { kind: "model" } | { kind: "part"; id: string };

export interface AuthoredColorResolution {
  color: string | null;
  source: "Embedded 3MF" | "GoggleLab default";
}

export interface AuthoredColorOptions {
  readonly summary: ModelSummary;
  readonly selectedPlateId?: number | null;
}

const DEFAULT_COLOR: AuthoredColorResolution = { color: null, source: "GoggleLab default" };

function normalizeColor(color: string | null | undefined): string | null {
  return typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color) ? color.toUpperCase() : null;
}

function partForId(summary: ModelSummary, plateId: number | null, id: string): PartSummary | null {
  if (plateId !== null) {
    const plate = summary.plates.find((candidate) => candidate.id === plateId);
    return plate?.parts.find((part) => part.id === id) ?? null;
  }
  for (const plate of summary.plates) {
    const part = plate.parts.find((candidate) => candidate.id === id);
    if (part) return part;
  }
  return null;
}

function materialColor(summary: ModelSummary, materialId: string | null): string | null {
  if (!materialId) return null;
  const matches = summary.materials.filter((material) => material.id === materialId);
  if (matches.length === 0) return null;
  const colors = matches.map((material) => normalizeColor(material.color));
  const first = colors[0];
  return first && colors.every((color) => color === first) ? first : null;
}

function filamentColor(summary: ModelSummary, slot: number | null): string | null {
  if (slot === null || !Number.isInteger(slot) || slot < 1) return null;
  return normalizeColor(
    summary.embedded_filaments.find((filament) => filament.slot === slot)?.color,
  );
}

/** Resolve only colors authored into a 3MF; the viewport owns the default appearance. */
export function resolveAuthoredColor(
  options: AuthoredColorOptions,
  target: AuthoredColorTarget,
): AuthoredColorResolution {
  const { summary } = options;
  if (summary.format !== "3mf") return DEFAULT_COLOR;

  let color: string | null;
  if (target.kind === "model") {
    color = filamentColor(summary, 1);
  } else {
    const part = partForId(summary, options.selectedPlateId ?? null, target.id);
    color = part
      ? (materialColor(summary, part.material_id) ?? filamentColor(summary, part.filament_slot))
      : filamentColor(summary, 1);
  }

  return color ? { color, source: "Embedded 3MF" } : DEFAULT_COLOR;
}
