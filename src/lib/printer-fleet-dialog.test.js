import { expect, test } from "bun:test";
import { PrinterFleetManager } from "./printer-fleet";

class Node {
  constructor(tag = "node") {
    this.tag = tag;
  }
  children = [];
  listeners = new Map();
  attributes = new Map();
  dataset = {};
  value = "";
  textContent = "";
  disabled = false;
  open = false;
  isConnected = true;
  get options() {
    return this.children;
  }
  append(...nodes) {
    this.children.push(...nodes);
  }
  add(node) {
    this.append(node);
  }
  prepend(...nodes) {
    this.children.unshift(...nodes);
  }
  replaceChildren(...nodes) {
    this.children = nodes;
  }
  setAttribute(name, value) {
    this.attributes.set(name, value);
  }
  addEventListener(type, callback) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), callback]);
  }
  contains(node) {
    return this.all().includes(node);
  }
  all() {
    return [this, ...this.children.flatMap((child) => child.all())];
  }
  querySelectorAll(selector) {
    return this.all().filter((node) =>
      selector === "[data-fleet-focus]" ? node.dataset.fleetFocus : node.tag === selector,
    );
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }
  focus() {
    document.activeElement = this;
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
  }
  fire(type) {
    if (type === "click" && this.disabled) return;
    for (const callback of this.listeners.get(type) ?? []) callback({ preventDefault() {} });
  }
}
const carbon = {
  id: "carbon",
  metadataVersion: 2,
  name: "Fixture Carbon",
  make: "Elegoo",
  model: "Centauri Carbon",
  printerType: "fdm",
  firmware: null,
  webUrl: null,
  notes: "",
  bedMm: [256, 256, 256],
  connection: { kind: "network", address: "http://192.0.2.10" },
  endpointApproved: true,
  connectionStatus: "unknown",
  jobStatus: "unknown",
  lastCheckedAt: null,
  connectionErrorCode: null,
  sourceKind: "manual",
  printCount: 0,
  lastPrintedAt: null,
  createdAt: "2026-10-19T00:00:00Z",
  updatedAt: "2026-10-19T00:00:00Z",
};
const model = (printers = [carbon]) => ({
  managedPrinters: printers,
  slicerDetections: [],
  activePrinterId: "carbon",
  maintenanceLogs: [],
  maintenanceReminders: [],
});
const settle = async () => {
  for (let i = 0; i < 12; i++) await Promise.resolve();
};
async function dom(run, transport = async () => ({ result: "connected", printer: carbon })) {
  const saved = {
    document: globalThis.document,
    HTMLElement: globalThis.HTMLElement,
    Option: globalThis.Option,
    window: globalThis.window,
  };
  globalThis.HTMLElement = Node;
  globalThis.Option = class extends Node {
    constructor(text, value) {
      super("option");
      this.textContent = text;
      this.value = value;
    }
  };
  globalThis.document = {
    activeElement: null,
    createElement: (tag) => new Node(tag),
    createTextNode: (text) => {
      const node = new Node();
      node.textContent = text;
      return node;
    },
  };
  globalThis.window = {
    prompt: () => {
      throw new Error("Native WebView prompt must not be used");
    },
  };
  const container = new Node();
  const calls = [];
  const manager = new PrinterFleetManager(container, model(), {
    testConnection: async (...args) => {
      calls.push(args);
      return transport(...args);
    },
  });
  try {
    await run({ container, manager, calls });
  } finally {
    Object.assign(globalThis, saved);
  }
}
const control = (container, key) => container.all().find((node) => node.dataset.fleetFocus === key);
const modal = (container) => container.all().find((node) => node.tag === "dialog");
const input = (container) => modal(container).querySelector("input");
const form = (container) => modal(container).querySelector("form");

test("manual Carbon dialog validates ephemeral ID and starts only one explicit unicast check", async () => {
  await dom(async ({ container, calls }) => {
    const invoker = control(container, "test:carbon");
    invoker.focus();
    invoker.fire("click");
    expect(modal(container).open).toBe(true);
    expect(calls).toEqual([]);
    expect(
      modal(container)
        .all()
        .some((n) => n.textContent.includes("unencrypted")),
    ).toBe(true);
    const field = input(container);
    expect(field.autocomplete).toBe("off");
    expect(field.autocapitalize).toBe("off");
    expect(field.spellcheck).toBe(false);
    const submit = form(container);
    for (const value of ["", "bad id", "a".repeat(129)]) {
      field.value = value;
      submit.fire("submit");
      await settle();
      expect(calls).toEqual([]);
    }
    field.value = "  Mainboard_123-abc.9  ";
    submit.fire("submit");
    submit.fire("submit");
    await settle();
    expect(calls).toEqual([["carbon", "Mainboard_123-abc.9", false]]);
    expect(field.value).toBe("");
    expect(modal(container).open).toBe(false);
    expect(JSON.stringify(calls[0])).not.toContain("discover");
  });
});

test("manual Carbon dialog cancel and stale detached callbacks never start checks", async () => {
  await dom(async ({ container, calls }) => {
    control(container, "test:carbon").fire("click");
    const oldForm = form(container);
    const oldInput = input(container);
    oldInput.value = "private-id";
    modal(container).querySelector("button").fire("click");
    expect(oldInput.value).toBe("");
    expect(modal(container).open).toBe(false);
    control(container, "test:carbon").fire("click");
    oldInput.value = "stale-id";
    oldForm.fire("submit");
    await settle();
    expect(calls).toEqual([]);
    expect(modal(container).open).toBe(true);
    expect(input(container).value).toBe("");
    modal(container).fire("cancel");
    expect(modal(container).open).toBe(false);
  });
});

test("manual Carbon dialog rejects changed, unapproved or removed profiles before transport", async () => {
  for (const printers of [
    [{ ...carbon, metadataVersion: 3 }],
    [{ ...carbon, endpointApproved: false }],
    [{ ...carbon, connection: { kind: "network", address: "http://192.0.2.11" } }],
    [{ ...carbon, model: "Centauri Carbon 2" }],
    [],
  ]) {
    await dom(async ({ container, manager, calls }) => {
      control(container, "test:carbon").fire("click");
      input(container).value = "fixture-id";
      manager.updateModel(model(printers));
      form(container).fire("submit");
      await settle();
      expect(calls).toEqual([]);
      expect(
        modal(container)
          .all()
          .some((n) => n.textContent.includes("changed")),
      ).toBe(true);
    });
  }
});

test("completed old Carbon request cannot close a newly opened fleet dialog", async () => {
  let finish;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  await dom(
    async ({ container, calls }) => {
      control(container, "test:carbon").fire("click");
      input(container).value = "fixture-id";
      form(container).fire("submit");
      await settle();
      expect(calls.length).toBe(1);
      modal(container).querySelector("button").fire("click");
      control(container, "add-printer").fire("click");
      const replacement = form(container);
      finish({ result: "connected", printer: carbon });
      await settle();
      expect(modal(container).open).toBe(true);
      expect(form(container)).toBe(replacement);
    },
    () => pending,
  );
});
