import { expect, test } from "bun:test";
import {
  browserSelectionTargets,
  ensureBulkSelectionLimit,
  pagesToRestore,
} from "./browser-selection";

test("browser bulk selection snapshots exact IDs and decimal-string versions", () => {
  const models = new Map([
    ["a", { id: "a", metadataVersion: "18446744073709551615" }],
    ["b", { id: "b", metadataVersion: "9" }],
  ]);
  expect(browserSelectionTargets(models.values())).toEqual([
    { modelId: "a", expectedVersion: "18446744073709551615" },
    { modelId: "b", expectedVersion: "9" },
  ]);
});

test("all-filtered selection refuses scope beyond the atomic bulk limit", () => {
  expect(() => ensureBulkSelectionLimit(10_000)).not.toThrow();
  expect(() => ensureBulkSelectionLimit(10_001)).toThrow("Narrow the filters");
});

test("catalog invalidation restores the number of pages the user had loaded", () => {
  expect(pagesToRestore(0, 100)).toBe(1);
  expect(pagesToRestore(100, 100)).toBe(1);
  expect(pagesToRestore(156, 100)).toBe(2);
});
