import { expect, test } from "bun:test";
import { isBrowserDialogCurrent } from "./browser-dialog";

test("closed, replaced, and disconnected browser dialogs reject stale async responses", () => {
  expect(isBrowserDialogCurrent(4, 4, true, true)).toBe(true);
  expect(isBrowserDialogCurrent(3, 4, true, true)).toBe(false);
  expect(isBrowserDialogCurrent(4, 4, false, true)).toBe(false);
  expect(isBrowserDialogCurrent(4, 4, true, false)).toBe(false);
});
