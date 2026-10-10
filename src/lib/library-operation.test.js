import { expect, test } from "bun:test";
import { LibraryOperationGuard } from "./library-operation";

test("a newer Library operation prevents an older async completion from owning the UI", () => {
  const guard = new LibraryOperationGuard();
  const importing = guard.begin();
  expect(guard.isCurrent(importing)).toBe(true);

  const refresh = guard.begin();
  expect(guard.isCurrent(importing)).toBe(false);
  expect(guard.isCurrent(refresh)).toBe(true);
});
