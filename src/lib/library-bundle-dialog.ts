import type { BundlePreviewSummary, DecodedGeometry, GeometryStats, SlicerApp } from "./ipc";
import { Viewport } from "./viewport";

export function isCurrentBundleRequest(currentToken: string | null, requestToken: string): boolean {
  return currentToken === requestToken;
}

export class LibraryBundleDialog {
  private readonly dialog: HTMLDialogElement;
  private summary: BundlePreviewSummary | null = null;
  private invoker: HTMLElement | null = null;
  private viewport: Viewport | null = null;
  private error: HTMLElement | null = null;
  onLoadGeometry: (token: string) => Promise<DecodedGeometry> = async () => ({
    triangleCount: 0,
    positions: new Float32Array(),
  });
  onClosePreview: (token: string) => Promise<void> = async () => {};
  onHandoff: (bundleId: string, slicerId: string, token: string) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-bundle-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      void this.close();
    });
    container.append(this.dialog);
  }

  async show(
    summary: BundlePreviewSummary,
    slicers: SlicerApp[],
    invoker?: HTMLElement,
  ): Promise<void> {
    const previous = this.summary;
    let cleanupError = "";
    if (previous && previous.token !== summary.token) {
      try {
        await this.onClosePreview(previous.token);
      } catch (error) {
        cleanupError = `Could not clean up the previous preview: ${String((error as { message?: unknown })?.message ?? error)}`;
      }
    }
    this.summary = summary;
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = `Bundle preview · ${summary.memberCount} models`;
    header.append(
      title,
      this.button("Close", () => void this.close()),
    );
    const stats = document.createElement("p");
    stats.textContent = `${summary.previewedCount} previewed · ${summary.triangleCount.toLocaleString()} triangles · ${summary.layout} layout`;
    const notice = document.createElement("p");
    notice.className = "hint";
    notice.textContent = summary.notice ?? "Every bundle member fits within the preview capacity.";
    this.error = document.createElement("div");
    this.error.className = "library-editor-error";
    this.error.setAttribute("role", "alert");
    this.error.textContent = cleanupError;
    const grid = document.createElement("div");
    grid.className = "library-bundle-preview-grid";
    for (const member of summary.members) {
      const item = document.createElement("div");
      const name = document.createElement("strong");
      name.textContent = member.displayName;
      const meta = document.createElement("span");
      meta.textContent = `${member.triangleCount.toLocaleString()} triangles · ${member.offset.join(", ")}`;
      item.append(name, meta);
      grid.append(item);
    }
    const canvas = document.createElement("div");
    canvas.className = "library-bundle-canvas";
    canvas.setAttribute("aria-label", "Independent 3D bundle preview");
    const actions = document.createElement("footer");
    const slicer = document.createElement("select");
    slicer.setAttribute("aria-label", "Slicer for all bundle members");
    for (const app of slicers) slicer.add(new Option(app.display_name, app.id));
    const handoff = this.button(
      `Send all ${summary.memberCount} to slicer`,
      () => void this.handoff(slicer.value, handoff),
    );
    const retry = this.button(
      "Retry 3D preview",
      () => void this.loadGeometry(summary, this.viewport, retry),
    );
    retry.hidden = true;
    handoff.disabled = !slicers.length;
    actions.append(retry, slicer, handoff);
    this.dialog.append(header, stats, notice, this.error, canvas, grid, actions);
    if (!this.dialog.open) this.dialog.showModal();
    this.viewport?.dispose();
    this.viewport = new Viewport(canvas);
    void this.loadGeometry(summary, this.viewport, retry);
    this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
  }

  private async loadGeometry(
    summary: BundlePreviewSummary,
    viewport: Viewport | null,
    retry: HTMLButtonElement,
  ): Promise<void> {
    if (!viewport) return;
    retry.disabled = true;
    try {
      const geometry = await this.onLoadGeometry(summary.token);
      if (
        !isCurrentBundleRequest(this.summary?.token ?? null, summary.token) ||
        this.viewport !== viewport
      )
        return;
      viewport.loadMesh({
        id: 0,
        name: "Bundle",
        geometry,
        stats: this.geometryStats(geometry),
        parts: [],
      });
      retry.hidden = true;
      if (this.error) this.error.textContent = "";
    } catch (error) {
      if (!isCurrentBundleRequest(this.summary?.token ?? null, summary.token)) return;
      retry.hidden = false;
      if (this.error) {
        this.error.textContent = `Could not load the 3D preview: ${String((error as { message?: unknown })?.message ?? error)}`;
      }
    } finally {
      retry.disabled = false;
    }
  }

  private geometryStats(geometry: DecodedGeometry): GeometryStats | null {
    if (!geometry.positions.length) return null;
    const min: [number, number, number] = [Infinity, Infinity, Infinity];
    const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
    for (let index = 0; index < geometry.positions.length; index += 3) {
      for (let axis = 0; axis < 3; axis += 1) {
        min[axis] = Math.min(min[axis], geometry.positions[index + axis]);
        max[axis] = Math.max(max[axis], geometry.positions[index + axis]);
      }
    }
    return {
      bbox: { min, max },
      dimensions: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
      surface_area_mm2: 0,
      volume_mm3: 0,
    };
  }

  private async handoff(slicerId: string, button: HTMLButtonElement): Promise<void> {
    const summary = this.summary;
    if (!summary || !slicerId) return;
    button.disabled = true;
    try {
      await this.onHandoff(summary.bundleId, slicerId, summary.token);
      await this.close();
    } catch (error) {
      if (this.error) {
        this.error.textContent = `Could not send the bundle: ${String((error as { message?: unknown })?.message ?? error)}`;
      }
    } finally {
      button.disabled = false;
    }
  }

  async dismiss(): Promise<void> {
    await this.close();
  }

  private async close(): Promise<void> {
    const summary = this.summary;
    try {
      if (summary) await this.onClosePreview(summary.token);
    } catch (error) {
      if (this.error) {
        this.error.textContent = `Could not close the native preview: ${String((error as { message?: unknown })?.message ?? error)}`;
      }
      this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
      return;
    }
    this.summary = null;
    this.viewport?.dispose();
    this.viewport = null;
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }
}
