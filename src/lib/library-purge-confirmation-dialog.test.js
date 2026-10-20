import { expect, test } from "bun:test";
import { LibraryPurgeConfirmationDialog } from "./library-purge-confirmation-dialog";
import { LibraryManagementDialog } from "./library-management-dialog";

class Node {
  constructor(tag = "node") {
    this.tag = tag;
  }
  style = {};
  children = [];
  listeners = new Map();
  textContent = "";
  value = "";
  disabled = false;
  open = false;
  isConnected = true;
  append(...children) {
    this.children.push(...children);
  }
  replaceChildren(...children) {
    for (const child of this.children) child.disconnect();
    this.children = children;
  }
  disconnect() {
    this.isConnected = false;
    for (const child of this.children) child.disconnect();
  }
  setAttribute() {}
  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }
  focus() {
    document.activeElement = this;
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
    this.fire("close");
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
  querySelector(selector) {
    return this.all().find((node) => node.tag === selector);
  }
}
const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};
async function dom(run) {
  const before = { document: globalThis.document, HTMLElement: globalThis.HTMLElement };
  globalThis.HTMLElement = Node;
  const invoker = new Node("button");
  globalThis.document = { activeElement: invoker, createElement: (tag) => new Node(tag) };
  try {
    await run(new Node(), invoker);
  } finally {
    globalThis.document = before.document;
    globalThis.HTMLElement = before.HTMLElement;
  }
}
const find = (root, text) => root.all().find((node) => node.textContent === text);
const confirmation = (root) =>
  root
    .all()
    .find((node) => node.className === "library-editor-dialog library-purge-confirmation-dialog");
const input = (root) =>
  confirmation(root)
    .all()
    .find((node) => node.tag === "input");
const form = (root) =>
  confirmation(root)
    .all()
    .find((node) => node.tag === "form");

test("purge requires the exact token and displays the selected backup as text", async () => {
  await dom(async (root, invoker) => {
    const saved = [];
    const editor = new LibraryPurgeConfirmationDialog(root);
    const destination = "/synthetic/<backup & safe>.gogglelab-backup";
    editor.show(destination, async (token, path) => {
      saved.push([token, path]);
    });
    expect(find(root, destination)).toBeDefined();
    expect(document.activeElement).toBe(input(root));
    for (const value of ["", "purge", " PURGE", "PURGE ", "PurGe"]) {
      input(root).value = value;
      input(root).fire("input");
      expect(
        root
          .all()
          .find((node) => node.tag === "button" && node.textContent === "Back up and purge catalog")
          .disabled,
      ).toBe(true);
      form(root).fire("submit");
      await settle();
    }
    expect(saved).toEqual([]);
    input(root).value = "PURGE";
    input(root).fire("input");
    expect(
      root
        .all()
        .find((node) => node.tag === "button" && node.textContent === "Back up and purge catalog")
        .disabled,
    ).toBe(false);
    form(root).fire("submit");
    await settle();
    expect(saved).toEqual([["PURGE", destination]]);
    expect(confirmation(root).open).toBe(false);
    expect(document.activeElement).toBe(invoker);
  });
});

test("purge cancel and Escape do not call the backend; one pending write stays guarded", async () => {
  await dom(async (root) => {
    let calls = 0;
    let finish;
    const editor = new LibraryPurgeConfirmationDialog(root);
    const save = async () => {
      calls += 1;
      await new Promise((resolve) => {
        finish = resolve;
      });
    };
    editor.show("/synthetic/backup", save);
    find(root, "Cancel").fire("click");
    expect(calls).toBe(0);
    editor.show("/synthetic/backup", save);
    confirmation(root).fire("cancel");
    expect(calls).toBe(0);
    editor.dismiss();
    editor.show("/synthetic/backup", save);
    input(root).value = "PURGE";
    input(root).fire("input");
    form(root).fire("submit");
    form(root).fire("submit");
    await settle();
    expect(calls).toBe(1);
    expect(confirmation(root).fire("cancel")).toBe(true);
    expect(find(root, "Cancel").disabled).toBe(true);
    expect(editor.show("/synthetic/changed", save)).toBe(false);
    finish();
    await settle();
    expect(confirmation(root).open).toBe(false);
  });
});

test("purge failures stay visible and editable; disconnected invokers use a valid fallback", async () => {
  await dom(async (root, invoker) => {
    const fallback = new Node("button");
    let calls = 0;
    const editor = new LibraryPurgeConfirmationDialog(root, () => fallback.focus());
    editor.show("/synthetic/backup", async () => {
      calls += 1;
      throw new Error("Verified backup could not be created");
    });
    input(root).value = "PURGE";
    input(root).fire("input");
    form(root).fire("submit");
    await settle();
    expect(calls).toBe(1);
    expect(confirmation(root).open).toBe(true);
    expect(find(root, "Verified backup could not be created")).toBeDefined();
    expect(input(root).disabled).toBe(false);
    invoker.isConnected = false;
    find(root, "Cancel").fire("click");
    expect(document.activeElement).toBe(fallback);
  });
});

test("management discards a backup picker result after close and reopen", async () => {
  await dom(async (root) => {
    const management = new LibraryManagementDialog(root);
    let finish;
    let calls = 0;
    management.onChoosePurgeBackupDestination = async () =>
      await new Promise((resolve) => {
        finish = resolve;
      });
    management.onPurge = async () => {
      calls += 1;
    };
    management.show([]);
    find(root, "Back up and purge catalog…").fire("click");
    await settle();
    management.dismiss();
    management.show([]);
    finish("/synthetic/stale-backup");
    await settle();
    expect(confirmation(root).open).toBe(false);
    expect(calls).toBe(0);
  });
});

test("management chooses a backup first then forwards only confirmed exact arguments", async () => {
  await dom(async (root) => {
    const management = new LibraryManagementDialog(root);
    const saved = [];
    management.onChoosePurgeBackupDestination = async () => "/synthetic/chosen-backup";
    management.onPurge = async (...args) => {
      saved.push(args);
    };
    management.show([]);
    find(root, "Back up and purge catalog…").fire("click");
    await settle();
    expect(confirmation(root)).toBeDefined();
    expect(confirmation(root).open).toBe(true);
    expect(saved).toEqual([]);
    input(root).value = "PURGE";
    input(root).fire("input");
    form(root).fire("submit");
    await settle();
    expect(saved).toEqual([["PURGE", "/synthetic/chosen-backup"]]);
  });
});

test("closing management invalidates an already typed purge confirmation", async () => {
  await dom(async (root) => {
    const management = new LibraryManagementDialog(root);
    let calls = 0;
    management.onChoosePurgeBackupDestination = async () => "/synthetic/backup";
    management.onPurge = async () => {
      calls += 1;
    };
    management.show([]);
    find(root, "Back up and purge catalog…").fire("click");
    await settle();
    input(root).value = "PURGE";
    input(root).fire("input");
    const oldForm = form(root);
    management.dismiss();
    management.show([]);
    oldForm.fire("submit");
    await settle();
    expect(calls).toBe(0);
    expect(confirmation(root).open).toBe(false);
  });
});

test("cancelled or failed backup selection never opens purge confirmation", async () => {
  await dom(async (root) => {
    const management = new LibraryManagementDialog(root);
    let calls = 0;
    management.onPurge = async () => {
      calls += 1;
    };
    management.onChoosePurgeBackupDestination = async () => null;
    management.show([]);
    find(root, "Back up and purge catalog…").fire("click");
    await settle();
    expect(confirmation(root).open).toBe(false);
    expect(calls).toBe(0);
    management.onChoosePurgeBackupDestination = async () => {
      throw new Error("Picker unavailable");
    };
    find(root, "Back up and purge catalog…").fire("click");
    await settle();
    expect(find(root, "Picker unavailable")).toBeDefined();
    expect(confirmation(root).open).toBe(false);
    expect(calls).toBe(0);
  });
});
