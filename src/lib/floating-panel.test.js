import { expect, test } from "bun:test";
import { FloatingPanel, clampPanelPosition } from "./floating-panel";

test("dragging beyond viewport edges keeps the complete panel reachable", () => {
  expect(
    clampPanelPosition(
      { x: -100, y: 900 },
      { width: 300, height: 250 },
      { width: 800, height: 600 },
    ),
  ).toEqual({ x: 12, y: 338 });
});
test("resizing the viewport clamps the old position into the new bounds", () => {
  expect(
    clampPanelPosition(
      { x: 480, y: 330 },
      { width: 300, height: 250 },
      { width: 340, height: 290 },
    ),
  ).toEqual({ x: 28, y: 28 });
});
test("a viewport smaller than the panel keeps its title bar accessible", () => {
  expect(
    clampPanelPosition(
      { x: 100, y: 100 },
      { width: 300, height: 250 },
      { width: 200, height: 150 },
    ),
  ).toEqual({ x: 0, y: 0 });
});

class FakeResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

function panelFixture(bottom = "", right = "") {
  const events = new Map();
  const root = {
    style: {
      bottom,
      left: "",
      top: "",
      right,
      setProperty(property, value) {
        this[property] = value;
      },
      removeProperty(property) {
        this[property] = "";
      },
    },
    dataset: {},
    hidden: false,
    offsetWidth: 300,
    offsetHeight: 200,
    parentElement: {
      clientWidth: 800,
      clientHeight: 600,
      getBoundingClientRect: () => ({ left: 0, top: 0 }),
    },
    getBoundingClientRect: () => ({ left: 100, top: 100 }),
  };
  const handle = {
    attrs: new Map(),
    addEventListener(type, listener) {
      events.set(type, listener);
    },
    setAttribute(name, value) {
      this.attrs.set(name, value);
    },
    focus() {},
    setPointerCapture() {},
  };
  return { events, handle, root };
}

test("FloatingPanel uses its explicit move label and restores a bottom anchor", () => {
  const previousResizeObserver = globalThis.ResizeObserver;
  globalThis.ResizeObserver = FakeResizeObserver;
  const { events, handle, root } = panelFixture("12px", "12px");
  const panel = new FloatingPanel(root, handle, "Move test panel");

  expect(handle.attrs.get("aria-label")).toBe("Move test panel");
  events.get("keydown")({
    key: "ArrowRight",
    shiftKey: false,
    preventDefault() {},
    stopPropagation() {},
  });
  expect(root.style.bottom).toBe("");
  expect(root.style.left).toBe("110px");
  expect(root.style.top).toBe("100px");
  expect(root.style.right).toBe("auto");

  events.get("keydown")({
    key: "Home",
    preventDefault() {},
    stopPropagation() {},
  });
  expect(root.style.bottom).toBe("12px");
  expect(root.style.left).toBe("");
  expect(root.style.top).toBe("");
  expect(root.style.right).toBe("12px");

  panel.dispose();
  if (previousResizeObserver) globalThis.ResizeObserver = previousResizeObserver;
  else delete globalThis.ResizeObserver;
});

test("FloatingPanel preserves top-right reset behavior", () => {
  const previousResizeObserver = globalThis.ResizeObserver;
  globalThis.ResizeObserver = FakeResizeObserver;
  const { events, handle, root } = panelFixture();
  const panel = new FloatingPanel(root, handle, "Move test panel");

  events.get("pointerdown")({
    button: 0,
    pointerId: 1,
    clientX: 100,
    clientY: 100,
    preventDefault() {},
    stopPropagation() {},
  });
  events.get("pointermove")({
    pointerId: 1,
    clientX: 120,
    clientY: 130,
  });
  expect(root.style.left).toBe("120px");
  expect(root.style.top).toBe("130px");
  expect(root.style.right).toBe("auto");
  events.get("pointerup")();
  events.get("keydown")({
    key: "Home",
    preventDefault() {},
    stopPropagation() {},
  });
  expect(root.style.left).toBe("");
  expect(root.style.top).toBe("");
  expect(root.style.right).toBe("");
  expect(root.style.bottom).toBe("");

  panel.dispose();
  if (previousResizeObserver) globalThis.ResizeObserver = previousResizeObserver;
  else delete globalThis.ResizeObserver;
});
