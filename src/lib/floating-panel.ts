export interface PanelPoint {
  x: number;
  y: number;
}
export interface PanelSize {
  width: number;
  height: number;
}

/** Keep the whole panel reachable, even when its host becomes smaller. */
export function clampPanelPosition(
  point: PanelPoint,
  size: PanelSize,
  bounds: PanelSize,
): PanelPoint {
  const maxX = Math.max(0, bounds.width - size.width);
  const maxY = Math.max(0, bounds.height - size.height);
  const padX = Math.min(12, maxX / 2);
  const padY = Math.min(12, maxY / 2);
  return {
    x: Math.min(maxX - padX, Math.max(padX, point.x)),
    y: Math.min(maxY - padY, Math.max(padY, point.y)),
  };
}

/** Pointer/keyboard movement for a nonmodal panel inside its positioned host. */
export class FloatingPanel {
  private position: PanelPoint | null = null;
  private drag: { pointer: number; start: PanelPoint; origin: PanelPoint } | null = null;
  private host: HTMLElement | null = null;
  private readonly initialStyles: {
    left: string;
    top: string;
    right: string;
    bottom: string;
  };
  private events = new AbortController();
  private observer = new ResizeObserver(() => this.refresh());

  constructor(
    private root: HTMLElement,
    handle: HTMLButtonElement,
    moveLabel: string,
  ) {
    this.initialStyles = {
      left: root.style.left,
      top: root.style.top,
      right: root.style.right,
      bottom: root.style.bottom,
    };
    const options = { signal: this.events.signal };
    handle.title = "Drag to move. Arrow keys move; Enter or Home resets position.";
    handle.setAttribute("aria-label", moveLabel);
    handle.setAttribute("aria-description", handle.title);
    handle.addEventListener(
      "pointerdown",
      (event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.stopPropagation();
        this.refresh();
        handle.focus({ preventScroll: true });
        this.drag = {
          pointer: event.pointerId,
          start: { x: event.clientX, y: event.clientY },
          origin: this.currentPosition(),
        };
        handle.setPointerCapture(event.pointerId);
        root.dataset.dragging = "true";
      },
      options,
    );
    handle.addEventListener(
      "pointermove",
      (event) => {
        if (this.drag?.pointer !== event.pointerId) return;
        this.move({
          x: this.drag.origin.x + event.clientX - this.drag.start.x,
          y: this.drag.origin.y + event.clientY - this.drag.start.y,
        });
      },
      options,
    );
    const stop = () => {
      this.drag = null;
      delete root.dataset.dragging;
    };
    handle.addEventListener("pointerup", stop, options);
    handle.addEventListener("pointercancel", stop, options);
    handle.addEventListener("lostpointercapture", stop, options);
    handle.addEventListener(
      "keydown",
      (event) => {
        const step = event.shiftKey ? 40 : 10;
        const delta: Record<string, PanelPoint> = {
          ArrowLeft: { x: -step, y: 0 },
          ArrowRight: { x: step, y: 0 },
          ArrowUp: { x: 0, y: -step },
          ArrowDown: { x: 0, y: step },
        };
        if (event.key === "Home" || delta[event.key]) {
          event.preventDefault();
          event.stopPropagation();
          if (event.key === "Home") this.reset();
          else {
            const current = this.currentPosition();
            this.move({ x: current.x + delta[event.key].x, y: current.y + delta[event.key].y });
          }
        }
      },
      options,
    );
    handle.addEventListener(
      "click",
      (event) => {
        if (event.detail === 0) this.reset();
      },
      options,
    );
    this.observer.observe(root);
  }

  private currentPosition(): PanelPoint {
    const panel = this.root.getBoundingClientRect();
    const host = this.root.parentElement?.getBoundingClientRect();
    return { x: panel.left - (host?.left ?? 0), y: panel.top - (host?.top ?? 0) };
  }

  private move(point: PanelPoint) {
    const host = this.root.parentElement;
    if (!host) return;
    this.position = clampPanelPosition(
      point,
      { width: this.root.offsetWidth, height: this.root.offsetHeight },
      { width: host.clientWidth, height: host.clientHeight },
    );
    this.root.style.removeProperty("bottom");
    this.root.style.left = `${this.position.x}px`;
    this.root.style.top = `${this.position.y}px`;
    this.root.style.right = "auto";
  }

  refresh() {
    if (this.root.parentElement !== this.host) {
      if (this.host) this.observer.unobserve(this.host);
      this.host = this.root.parentElement;
      if (this.host) this.observer.observe(this.host);
    }
    if (!this.root.hidden && this.position) this.move(this.position);
  }

  reset() {
    this.position = null;
    for (const property of ["left", "top", "right", "bottom"] as const) {
      const value = this.initialStyles[property];
      if (value) this.root.style.setProperty(property, value);
      else this.root.style.removeProperty(property);
    }
  }

  dispose() {
    this.events.abort();
    this.observer.disconnect();
  }
}
