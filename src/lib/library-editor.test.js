import { expect, test } from "bun:test";
import {
  buildMetadataPatch,
  isMetadataVersionError,
  matchesPrintedFilter,
  restoreLibraryFocus,
  selectionScopeIds,
  versionedMetadataTargets,
} from "./library-editor";
import { localDateInputValue, printAttemptSummary, validatePrintAttempt } from "./library-history";

const model = (id, metadata_version = 1, metadata = undefined) => ({
  id,
  metadata_version,
  metadata,
});

test("selection scopes distinguish the visible page from every filtered result", () => {
  expect(selectionScopeIds(["a", "b", "c"], ["a", "b"], "visible")).toEqual(["a", "b"]);
  expect(selectionScopeIds(["a", "b", "c"], ["a", "b"], "filtered")).toEqual(["a", "b", "c"]);
});

test("batch metadata payload includes only explicitly applied fields and preserves notes", () => {
  const patch = buildMetadataPatch(
    { designer: "Ada", notes: "must remain untouched", parent_model_id: "" },
    new Set(["designer", "parent_model_id", "tags"]),
    ["tag-a"],
    "merge",
  );
  expect(patch).toEqual({
    designer: "Ada",
    parent_model_id: "",
    tags: ["tag-a"],
    tag_mode: "merge",
  });
  expect(patch.notes).toBeUndefined();
});

test("ratings and favorite changes are versioned metadata fields for single and batch edits", () => {
  expect(
    buildMetadataPatch(
      { rating: "5", favorite: "true" },
      new Set(["rating", "favorite"]),
      [],
      "merge",
    ),
  ).toEqual({ rating: 5, favorite: true });
  expect(() => buildMetadataPatch({ rating: "6" }, new Set(["rating"]), [], "merge")).toThrow(
    "Rating must be a whole number from 0 to 5.",
  );
});

test("bulk targets snapshot IDs and optimistic versions without duplicating the patch", () => {
  expect(versionedMetadataTargets([model("a", 3), model("b", 8)], ["b", "a"])).toEqual([
    { model_id: "a", expected_version: 3 },
    { model_id: "b", expected_version: 8 },
  ]);
});

test("version conflicts get a focused reload error", () => {
  expect(isMetadataVersionError({ kind: "Library", message: "metadata version conflict" })).toBe(
    true,
  );
  expect(isMetadataVersionError({ kind: "Library", message: "invalid URL" })).toBe(false);
});

test("editor save restores focus through a fallback when rerender replaced its invoker", () => {
  let originalFocused = false;
  let fallbackFocused = false;
  restoreLibraryFocus(
    { isConnected: false, focus: () => (originalFocused = true) },
    () => (fallbackFocused = true),
  );
  expect(originalFocused).toBe(false);
  expect(fallbackFocused).toBe(true);
});

test("printed filters preserve lifetime history when current status later becomes failed", () => {
  const failedAfterSuccess = model("m", 1, {
    print_status: "failed",
    print_count: 2,
    last_printed_at: "2026-10-01T00:00:00Z",
  });
  expect(matchesPrintedFilter(failedAfterSuccess, "ever")).toBe(true);
  expect(matchesPrintedFilter(failedAfterSuccess, "never")).toBe(false);
  expect(matchesPrintedFilter(failedAfterSuccess, "last30", Date.parse("2026-10-10"))).toBe(true);
});

test("print history validates dates, quantity, and the five-filament display limit", () => {
  const valid = {
    printed_at: "2026-10-10",
    quantity: 2,
    notes: "Clean result",
    filament_ids: ["pla-black", "pla-white"],
    outcome: "printed",
  };
  expect(validatePrintAttempt(valid)).toBeNull();
  expect(validatePrintAttempt({ ...valid, quantity: 0 })).toContain("Quantity");
  expect(
    validatePrintAttempt({ ...valid, filament_ids: ["1", "2", "3", "4", "5", "6"] }),
  ).toContain("five");
});

test("print history display summarizes normalized attempts", () => {
  expect(
    printAttemptSummary({
      id: "attempt",
      model_id: "model",
      printed_at: "2026-10-10T00:00:00.000Z",
      quantity: 3,
      notes: "",
      filament_ids: [],
      outcome: "failed",
      created_at: "",
      updated_at: "",
    }),
  ).toBe("2026-10-10 · 3 · failed");
});

test("new print attempts default to the local calendar date without UTC rollover", () => {
  expect(localDateInputValue(new Date(2026, 9, 10, 23, 45))).toBe("2026-10-10");
});
