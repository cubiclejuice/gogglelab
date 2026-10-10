import { expect, test } from "bun:test";
import { libraryFolderDepths } from "./library";

const folder = (id, parent_id = null) => ({ id, parent_id, name: id, model_count: 0 });

test("Library folder depths follow valid parents and stop at missing legacy parents", () => {
  const depths = libraryFolderDepths([
    folder("root"),
    folder("child", "root"),
    folder("grandchild", "child"),
    folder("orphan", "missing"),
  ]);
  expect(Object.fromEntries(depths)).toEqual({ root: 0, child: 1, grandchild: 2, orphan: 0 });
});

test("Library folder depth calculation terminates corrupt parent cycles", () => {
  const depths = libraryFolderDepths([folder("a", "b"), folder("b", "a"), folder("self", "self")]);
  expect(Object.fromEntries(depths)).toEqual({ a: 0, b: 0, self: 0 });
});
