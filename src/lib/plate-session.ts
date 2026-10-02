import type { DecodedGeometry, IntegrityReport, ModelSummary, PlateSummary } from "./ipc";
import type { ViewportPlate } from "./viewport";

export interface PlateView {
  summary: ModelSummary;
  plate: PlateSummary;
  overview: boolean;
  plates: ViewportPlate[];
  integrity: IntegrityReport | null;
}

/** File generations protect loads; revisions protect views within the same file. */
export class PlateSession {
  view: PlateView | null = null;
  busy = false;
  onChange: () => void = () => {};
  private generation = -1;
  private revision = 0;
  private summary: ModelSummary | null = null;
  private selectedId = 0;
  private overview = false;
  private geometry = new Map<number, Promise<DecodedGeometry>>();
  private reports = new Map<number, IntegrityReport>();

  constructor(
    private fetchGeometry: (generation: number, plateId: number) => Promise<DecodedGeometry>,
  ) {}

  reset(generation: number) {
    this.generation = generation;
    this.revision++;
    this.summary = null;
    this.view = null;
    this.busy = false;
    this.overview = false;
    this.geometry = new Map();
    this.reports = new Map();
    this.onChange();
  }

  async load(summary: ModelSummary) {
    if (summary.generation !== this.generation) return;
    this.summary = summary;
    this.selectedId =
      (summary.plates.find((p) => p.triangle_count > 0) ?? summary.plates[0])?.id ?? 0;
    this.overview = false;
    await this.render();
  }

  async select(plateId: number) {
    if (!this.summary?.plates.some((p) => p.id === plateId)) return;
    this.selectedId = plateId;
    await this.render();
  }

  async setOverview(overview: boolean) {
    if (!this.summary) return;
    this.overview = overview;
    await this.render();
  }

  receiveIntegrity(report: IntegrityReport) {
    if (report.generation !== this.generation) return;
    this.reports.set(report.plate_id, report);
    if (this.view?.plate.id === report.plate_id) {
      this.view = { ...this.view, integrity: report };
      this.onChange();
    }
  }

  private getGeometry(summary: ModelSummary, plate: PlateSummary): Promise<DecodedGeometry> {
    if (plate.triangle_count === 0) {
      return Promise.resolve({ triangleCount: 0, positions: new Float32Array(0) });
    }
    const cache = this.geometry;
    const cached = cache.get(plate.id);
    if (cached) return cached;
    const pending = this.fetchGeometry(summary.generation, plate.id).catch((error) => {
      cache.delete(plate.id);
      throw error;
    });
    cache.set(plate.id, pending);
    return pending;
  }

  private async render() {
    const summary = this.summary;
    const plate = summary?.plates.find((p) => p.id === this.selectedId);
    if (!summary || !plate) return;
    const revision = ++this.revision;
    const overview = this.overview;
    this.busy = true;
    this.onChange();
    try {
      const plates = await Promise.all(
        (overview ? summary.plates : [plate]).map(async (p) => ({
          id: p.id,
          name: p.name,
          stats: p.geometry,
          parts: p.parts.map((part) => ({ ...part })),
          geometry: await this.getGeometry(summary, p),
        })),
      );
      if (revision !== this.revision) return;
      this.view = {
        summary,
        plate,
        overview,
        plates,
        integrity: this.reports.get(plate.id) ?? null,
      };
      this.busy = false;
      this.onChange();
    } catch (error) {
      if (revision !== this.revision) return;
      this.busy = false;
      // Keep the last complete view and let the user retry the failed choice.
      if (this.view) {
        this.selectedId = this.view.plate.id;
        this.overview = this.view.overview;
      }
      this.onChange();
      throw error;
    }
  }
}
