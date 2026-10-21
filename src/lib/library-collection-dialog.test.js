import { expect, test } from "bun:test";
import { LibraryCollectionNameDialog } from "./library-collection-dialog";

class Node {
  constructor(tag = "node") {
    this.tag = tag;
  }
  children = [];
  listeners = new Map();
  value = "";
  textContent = "";
  disabled = false;
  open = false;
  isConnected = true;
  append(...children) {
    this.children.push(...children);
  }
  setAttribute() {}
  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }
  focus() {
    document.activeElement = this;
  }
  select() {
    this.selected = true;
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
    this.fire("close");
  }
  remove() {
    this.isConnected = false;
  }
  fire(type) {
    let prevented = false;
    if (type === "click" && this.disabled) return false;
    this.listeners.get(type)?.({
      preventDefault() {
        prevented = true;
      },
    });
    return prevented;
  }
  all() {
    return [this, ...this.children.flatMap((child) => child.all())];
  }
}
const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
async function dom(run) {
  const before = { document: globalThis.document, HTMLElement: globalThis.HTMLElement };
  globalThis.HTMLElement = Node;
  const invoker = new Node("button");
  globalThis.document = { activeElement: invoker, createElement: (tag) => new Node(tag) };
  const container = new Node();
  try {
    await run(container, invoker);
  } finally {
    globalThis.document = before.document;
    globalThis.HTMLElement = before.HTMLElement;
  }
}
const field = (container) => container.all().find((node) => node.tag === "input");
const dialog = (container) => container.all().find((node) => node.tag === "dialog");
const form = (container) => container.all().find((node) => node.tag === "form");

test("collection create and rename use native modal fields and restore focus", async () => {
  await dom(async (container, invoker) => {
    const editor = new LibraryCollectionNameDialog(container);
    const saved = [];
    editor.show("New collection", "", async (name) => {
      saved.push(name);
    });
    expect(dialog(container).open).toBe(true);
    expect(document.activeElement).toBe(field(container));
    field(container).value = "  Fixture collection  ";
    form(container).fire("submit");
    await settle();
    expect(saved).toEqual(["Fixture collection"]);
    expect(dialog(container).open).toBe(false);
    expect(document.activeElement).toBe(invoker);
    editor.show("Rename collection", "Fixture collection", async (name) => {
      saved.push(name);
    });
    expect(field(container).value).toBe("Fixture collection");
    expect(field(container).selected).toBe(true);
    field(container).value = "Renamed";
    form(container).fire("submit");
    await settle();
    expect(saved).toEqual(["Fixture collection", "Renamed"]);
  });
});

test("collection cancellation and invalid names never write; backend failures stay editable", async () => {
  await dom(async (container) => {
    const editor = new LibraryCollectionNameDialog(container);
    let calls = 0;
    editor.show("New collection", "", async () => {
      calls += 1;
      throw new Error("Catalog changed; retry");
    });
    for (const value of ["   ", ".", "..", "a/b", "a\\b"]) {
      field(container).value = value;
      form(container).fire("submit");
      await settle();
    }
    expect(calls).toBe(0);
    expect(dialog(container).open).toBe(true);
    field(container).value = "Valid";
    form(container).fire("submit");
    await settle();
    expect(calls).toBe(1);
    expect(dialog(container).open).toBe(true);
    expect(field(container).disabled).toBe(false);
    expect(document.activeElement).toBe(field(container));
    expect(container.all().some((node) => node.textContent === "Catalog changed; retry")).toBe(
      true,
    );
    container
      .all()
      .find((node) => node.textContent === "Cancel")
      .fire("click");
    expect(dialog(container).open).toBe(false);
    expect(calls).toBe(1);
  });
});

test("collection save returns focus to the visible Library when refresh replaced its invoker", async () => {
  await dom(async (container, invoker) => {
    const fallback = new Node("input");
    const editor = new LibraryCollectionNameDialog(container, () => fallback.focus());
    editor.show("New collection", "", async () => {
      invoker.isConnected = false;
    });
    field(container).value = "Refresh creates a new navigation row";
    form(container).fire("submit");
    await settle();
    expect(dialog(container).open).toBe(false);
    expect(document.activeElement).toBe(fallback);
  });
});
