import { expect, test } from "bun:test";
import { buildSlicerImportDrafts, SlicerImportReviewState } from "./slicer-import";

const records = [{ name: "Custom Slicer", path: "/untrusted/from-json" }];

test("slicer import requires a local selection and fresh trust for each created profile", () => {
  const state = new SlicerImportReviewState(records);
  expect(state.ready).toBe(false);
  state.choose(0, "create");
  state.setTrust(0, true);
  expect(state.ready).toBe(false);
  state.selectPath(0, "/local/picked-app");
  expect(state.ready).toBe(false);
  state.setTrust(0, true);
  expect(state.ready).toBe(true);
  expect(state.drafts).toEqual([
    { id: null, name: "Custom Slicer", path: "/local/picked-app", trust: true },
  ]);
  state.selectPath(0, "/local/different-app");
  expect(state.ready).toBe(false);
  state.setTrust(0, true);
  state.choose(0, "skip");
  expect(state.ready).toBe(true);
  expect(state.drafts).toEqual([]);
  state.choose(0, "create");
  expect(state.ready).toBe(false);
});

test("review rejects unreviewed rows, duplicates and forged states without passing imported paths", () => {
  expect(() => buildSlicerImportDrafts(records, new Map())).toThrow(/review/i);
  expect(() =>
    buildSlicerImportDrafts(records, new Map([[0, { action: "create", path: "", trusted: true }]])),
  ).toThrow(/local/i);
  const two = [...records, { name: "Custom Slicer", path: "/hint-two" }];
  const reviews = new Map([
    [0, { action: "create", path: "/picked/one", trusted: true }],
    [1, { action: "create", path: "/picked/two", trusted: true }],
  ]);
  expect(() => buildSlicerImportDrafts(two, reviews)).toThrow(/duplicate/i);
  expect(() =>
    buildSlicerImportDrafts(
      records,
      new Map([[0, { action: "create", path: "/picked", trusted: false }]]),
    ),
  ).toThrow(/trust/i);
});

test("slicer batch retries failures, preserves successful completion, and leaves empty imports silent", async () => {
  const empty = new SlicerImportReviewState([]);
  await empty.apply(async () => {
    throw new Error("empty import must not write");
  });
  expect(empty.summary).toBe("");
  const state = new SlicerImportReviewState(records);
  state.choose(0, "create");
  state.selectPath(0, "/local/picked-app");
  state.setTrust(0, true);
  let writes = 0;
  const save = async (drafts) => {
    writes += 1;
    expect(drafts[0].path).toBe("/local/picked-app");
    if (writes === 1) throw new Error("disk unavailable");
    return ["local-id"];
  };
  await expect(state.apply(save)).rejects.toThrow("disk unavailable");
  expect(state.completed).toBe(false);
  await state.apply(save);
  await state.apply(save); // A downstream model-import retry must not create another slicer.
  state.choose(0, "skip"); // Applied rows cannot be changed into a second batch.
  expect(writes).toBe(2);
  expect(state.completed).toBe(true);
  expect(state.summary).toContain("1 created, 0 skipped");
});
