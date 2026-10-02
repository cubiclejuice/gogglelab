import type { ModelSummary } from "./ipc";
import { resolveAuthoredColor, type AuthoredColorTarget } from "./authored-colors";
import type { ColorFeature } from "./app-extensions";
import type { ColorResolution, ColorTarget, ColorwayMode, ResolveColor } from "./color-types";

const DEFAULT_COLOR: ColorResolution = { color: null, source: "GoggleLab default" };

/** Session-only authored color view. It never reads or writes saved assignments. */
export class AuthoredColorSession {
  mode: ColorwayMode = "color";
  summary: ModelSummary | null = null;
  selectedPlateId: number | null = null;
  generation = -1;
  onChange: () => void = () => {};

  readonly resolveColor: ResolveColor = (target: ColorTarget) => {
    if (!this.summary) return DEFAULT_COLOR;
    return resolveAuthoredColor(
      { summary: this.summary, selectedPlateId: this.selectedPlateId },
      target as AuthoredColorTarget,
    );
  };

  load(summary: ModelSummary): void {
    if (summary.generation < this.generation) return;
    this.generation = summary.generation;
    this.summary = summary;
    if (!summary.plates.some((plate) => plate.id === this.selectedPlateId)) {
      this.selectedPlateId =
        (summary.plates.find((plate) => plate.triangle_count > 0) ?? summary.plates[0])?.id ?? null;
    }
    this.onChange();
  }

  reset(generation?: number): void {
    if (generation !== undefined && generation < this.generation) return;
    this.generation = generation ?? this.generation + 1;
    this.summary = null;
    this.selectedPlateId = null;
    this.onChange();
  }

  selectPlate(plateId: number): void {
    if (!this.summary?.plates.some((plate) => plate.id === plateId)) return;
    if (this.selectedPlateId === plateId) return;
    this.selectedPlateId = plateId;
    this.onChange();
  }

  setMode(mode: ColorwayMode): void {
    if (mode === this.mode) return;
    this.mode = mode;
    this.onChange();
  }
}

/** Free color preview has a mode switch, but no target picker or assignment actions. */
export class AuthoredColorFeature implements ColorFeature {
  readonly root: HTMLButtonElement;
  readonly session = new AuthoredColorSession();
  onChange: () => void = () => {};

  private readonly toggle = () => this.session.setMode(this.mode === "color" ? "model" : "color");

  constructor() {
    this.root = document.createElement("button");
    this.root.type = "button";
    this.root.className = "btn color-preview-toggle";
    this.root.setAttribute("aria-label", "Toggle authored color preview");
    this.root.addEventListener("click", this.toggle);
    this.session.onChange = () => {
      this.root.setAttribute("aria-pressed", String(this.mode === "color"));
      this.root.textContent = this.mode === "color" ? "Color preview on" : "Color preview off";
      this.onChange();
    };
    this.session.onChange();
  }

  get mode(): ColorwayMode {
    return this.session.mode;
  }

  get resolveColor(): ResolveColor {
    return this.session.resolveColor;
  }

  load(summary: ModelSummary): void {
    this.session.load(summary);
  }

  reset(generation?: number): void {
    this.session.reset(generation);
  }

  selectPlate(plateId: number): void {
    this.session.selectPlate(plateId);
  }

  setMode(mode: ColorwayMode): void {
    this.session.setMode(mode);
  }

  dispose(): void {
    this.root.removeEventListener("click", this.toggle);
    this.root.remove();
  }
}
