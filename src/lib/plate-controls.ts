import type { PlateView } from "./plate-session";
import { FloatingPanel } from "./floating-panel";

const CHEVRON =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="m4 10 4-4 4 4"/></svg>';
const CHECK =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><path d="m3 8 3 3 7-7"/></svg>';

/** A nonmodal, movable plate manager; selection remains owned by PlateSession. */
export class PlateControls {
  readonly root = document.createElement("section");
  readonly warning = document.createElement("p");
  onSelect: (plateId: number) => void = () => {};
  onOverview: (overview: boolean) => void = () => {};
  private body = document.createElement("div");
  private count = document.createElement("span");
  private collapse = document.createElement("button");
  private single = document.createElement("button");
  private all = document.createElement("button");
  private list = document.createElement("div");
  private status = document.createElement("p");
  private notice = document.createElement("p");
  private rows = new Map<number, HTMLButtonElement>();
  private generation = -1;
  private collapsed = false;
  private busy = false;
  private error: string | null = null;
  private floating: FloatingPanel;

  constructor() {
    this.root.className = "plate-controls";
    this.root.setAttribute("aria-label", "Plate manager");
    this.root.hidden = true;
    this.warning.className = "plate-warning";
    this.warning.setAttribute("role", "alert");
    this.warning.hidden = true;
    const header = document.createElement("div");
    header.className = "plate-window-header";
    const move = document.createElement("button");
    move.type = "button";
    move.className = "plate-window-move";
    move.innerHTML =
      '<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><circle cx="5" cy="4" r="1"/><circle cx="11" cy="4" r="1"/><circle cx="5" cy="8" r="1"/><circle cx="11" cy="8" r="1"/><circle cx="5" cy="12" r="1"/><circle cx="11" cy="12" r="1"/></svg><span>Plate manager</span>';
    this.count.className = "plate-window-count";
    move.append(this.count);
    this.collapse.type = "button";
    this.collapse.className = "plate-window-collapse";
    this.collapse.innerHTML = CHEVRON;
    this.collapse.setAttribute("aria-controls", "plate-manager-body");
    this.collapse.addEventListener("click", () => this.setCollapsed(!this.collapsed));
    header.append(move, this.collapse);

    this.body.id = "plate-manager-body";
    this.body.className = "plate-window-body";
    const modes = document.createElement("div");
    modes.className = "plate-view-modes";
    modes.setAttribute("role", "group");
    modes.setAttribute("aria-label", "Plate view");
    for (const [button, label, overview] of [
      [this.single, "Single plate", false],
      [this.all, "All plates", true],
    ] as const) {
      button.type = "button";
      button.textContent = label;
      button.setAttribute("aria-pressed", String(!overview));
      button.addEventListener("click", () => {
        if (!this.busy) this.onOverview(overview);
      });
      modes.append(button);
    }
    this.list.className = "plate-list";
    this.list.setAttribute("role", "group");
    this.list.setAttribute("aria-label", "Plates");
    this.list.addEventListener("keydown", (event) => {
      const buttons = [...this.rows.values()];
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (current < 0 || !["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      if (this.busy) return;
      const index =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? buttons.length - 1
            : Math.min(
                buttons.length - 1,
                Math.max(0, current + (event.key === "ArrowDown" ? 1 : -1)),
              );
      if (buttons[index]?.disabled) return;
      buttons[index]?.focus({ preventScroll: true });
      buttons[index]?.scrollIntoView({ block: "nearest" });
      buttons[index]?.click();
    });
    this.status.className = "plate-status";
    this.status.setAttribute("role", "status");
    this.notice.className = "plate-notice";
    this.notice.setAttribute("role", "alert");
    this.notice.hidden = true;
    this.body.append(modes, this.list, this.status, this.notice);
    this.root.append(header, this.body);
    this.floating = new FloatingPanel(this.root, move, "Move plate manager");
    this.setCollapsed(false);
  }

  private setCollapsed(collapsed: boolean) {
    this.collapsed = collapsed;
    this.body.hidden = collapsed;
    this.root.dataset.collapsed = String(collapsed);
    this.collapse.setAttribute("aria-expanded", String(!collapsed));
    this.collapse.setAttribute(
      "aria-label",
      collapsed ? "Expand plate manager" : "Collapse plate manager",
    );
    this.collapse.title = collapsed ? "Expand plate manager" : "Collapse plate manager";
    this.floating?.refresh();
  }

  update(view: PlateView | null, busy: boolean) {
    this.busy = busy;
    if (!view) {
      this.root.hidden = true;
      this.warning.hidden = true;
      this.generation = -1;
      this.error = null;
      return;
    }
    const { summary, plate, overview } = view;
    this.root.hidden = summary.plates.length <= 1;
    this.warning.textContent = this.root.hidden ? summary.plate_warning : "";
    this.warning.hidden = !this.warning.textContent;
    this.root.setAttribute("aria-busy", String(busy));
    if (busy) this.error = null;
    if (this.generation !== summary.generation) {
      this.generation = summary.generation;
      this.error = null;
      this.rows.clear();
      this.count.textContent = String(summary.plates.length);
      this.list.replaceChildren(
        ...summary.plates.map((p, index) => {
          const button = document.createElement("button");
          button.type = "button";
          button.className = "plate-row";
          button.title = p.name;
          button.setAttribute("aria-label", `Plate ${index + 1}: ${p.name}`);
          const number = document.createElement("span");
          number.className = "plate-row-number";
          number.textContent = String(index + 1).padStart(2, "0");
          number.setAttribute("aria-hidden", "true");
          const info = document.createElement("span");
          info.className = "plate-row-info";
          const name = document.createElement("span");
          name.className = "plate-row-name";
          name.textContent = p.name;
          const detail = document.createElement("span");
          detail.className = "plate-row-detail";
          detail.textContent = p.geometry
            ? `${p.geometry.dimensions.map((n) => n.toFixed(1)).join(" × ")} mm`
            : "Empty plate";
          info.append(name, detail);
          const check = document.createElement("span");
          check.className = "plate-row-check";
          check.innerHTML = CHECK;
          button.append(number, info, check);
          button.addEventListener("click", () => {
            if (!this.busy) this.onSelect(p.id);
          });
          this.rows.set(p.id, button);
          return button;
        }),
      );
      this.setCollapsed(false);
    }
    for (const [id, button] of this.rows) {
      button.setAttribute("aria-pressed", String(id === plate.id));
      button.tabIndex = id === plate.id ? 0 : -1;
      // Keep focused controls reachable while an asynchronous selection loads.
      button.setAttribute("aria-disabled", String(busy));
    }
    this.single.setAttribute("aria-pressed", String(!overview));
    this.all.setAttribute("aria-pressed", String(overview));
    this.single.setAttribute("aria-disabled", String(busy));
    this.all.disabled = summary.plates.length < 2;
    this.all.setAttribute("aria-disabled", String(busy || this.all.disabled));
    this.status.textContent = busy ? "Loading plate…" : `Measurements: ${plate.name}`;
    this.status.title = this.status.textContent;
    this.notice.textContent = this.error ?? summary.plate_warning;
    this.notice.hidden = !this.notice.textContent;
    this.floating.refresh();
  }

  showError(message: string) {
    this.error = message;
    if (this.root.hidden) {
      this.warning.textContent = message;
      this.warning.hidden = false;
      return;
    }
    this.notice.textContent = message;
    this.notice.hidden = false;
    this.root.hidden = false;
    this.setCollapsed(false);
  }

  dispose() {
    this.floating.dispose();
  }
}
