import { expect, test } from "bun:test";

test("nested modal escape defers to the focused open dialog", () => {
  class FakeDialog {
    constructor(open) {
      this.open = open;
    }
  }
  const original = globalThis.HTMLDialogElement;
  globalThis.HTMLDialogElement = FakeDialog;
  return import(`./dialog-escape?test=${Date.now()}`).then(({ pathContainsOpenDialog }) => {
    expect(pathContainsOpenDialog([{}, new FakeDialog(true), {}])).toBe(true);
    expect(pathContainsOpenDialog([{}, new FakeDialog(false), {}])).toBe(false);
    if (original) globalThis.HTMLDialogElement = original;
    else delete globalThis.HTMLDialogElement;
  });
});
