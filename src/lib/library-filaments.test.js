import { expect, test } from "bun:test";
import {
  AI_DISCLOSURE,
  aiAfterTags,
  aiTargets,
  assignmentIdsPreservingUnknown,
  buildAiReviewDecisions,
  filamentReferenceLabel,
  filamentAssignmentBatch,
  matchesFilamentFilter,
} from "./library-filaments";

const filament = (id, overrides = {}) => ({
  id,
  name: id,
  brand: "",
  material: "PLA",
  resource_kind: "manual",
  external_id: null,
  ...overrides,
});

test("unknown legacy filament IDs survive assignment edits until explicitly removed", () => {
  const catalog = [filament("known"), filament("archived", { archived: true })];
  expect(
    assignmentIdsPreservingUnknown(["known", "legacy"], catalog, ["known"], new Set()),
  ).toEqual(["known", "legacy"]);
  expect(
    assignmentIdsPreservingUnknown(["known", "legacy"], catalog, ["known"], new Set(["legacy"])),
  ).toEqual(["known"]);
  expect(filamentReferenceLabel("legacy", catalog)).toBe("Unknown filament (legacy)");
  expect(assignmentIdsPreservingUnknown(["archived"], catalog, [], new Set())).toEqual([
    "archived",
  ]);
});

test("filament filter matches any selected assignment and explicit unassigned", () => {
  expect(matchesFilamentFilter({ filament_ids: ["a", "b"] }, new Set(["b"]), false)).toBe(true);
  expect(matchesFilamentFilter({ filament_ids: [] }, new Set(), true)).toBe(true);
  expect(matchesFilamentFilter({ filament_ids: ["a"] }, new Set(), true)).toBe(false);
});

test("heterogeneous model assignments become one atomic per-model batch payload", () => {
  expect(
    filamentAssignmentBatch([
      { target: { model_id: "a", expected_version: 2 }, filamentIds: ["known", "legacy-a"] },
      { target: { model_id: "b", expected_version: 4 }, filamentIds: ["known", "legacy-b"] },
    ]),
  ).toEqual([
    {
      target: { model_id: "a", expected_version: 2 },
      filament_ids: ["known", "legacy-a"],
    },
    {
      target: { model_id: "b", expected_version: 4 },
      filament_ids: ["known", "legacy-b"],
    },
  ]);
});

test("AI targets snapshot optimistic versions and disclosure excludes sensitive metadata", () => {
  expect(aiTargets([{ id: "a", metadata_version: 7 }])).toEqual([
    { model_id: "a", expected_version: 7 },
  ]);
  expect(AI_DISCLOSURE).toContain("File paths, notes, source URLs, print history");
});

test("AI review requires an explicit accept or reject decision for every successful target", () => {
  const batch = {
    targets: [
      { modelId: "a", displayName: "A", suggestions: [], expectedVersion: 1 },
      { modelId: "b", displayName: "B", suggestions: [], expectedVersion: 1 },
    ],
  };
  expect(
    buildAiReviewDecisions(
      batch,
      new Map([["a", ["suggestion-a"]]]),
      new Map([
        ["a", "accept"],
        ["b", "reject"],
      ]),
    ),
  ).toEqual([
    { modelId: "a", action: "accept", acceptedSuggestionIds: ["suggestion-a"] },
    { modelId: "b", action: "reject", acceptedSuggestionIds: [] },
  ]);
  expect(() => buildAiReviewDecisions(batch, new Map(), new Map())).toThrow();
});

test("AI before/after tag sets follow append, replace, and category-aware merge exactly", () => {
  const existing = [
    { name: "Old type", category: "Type" },
    { name: "Favorite", category: null },
    { name: "Keep", category: "Workflow" },
  ];
  const suggestions = [
    { suggestionId: "type", name: "New type", category: "Type" },
    { suggestionId: "loose", name: "Desk", category: null },
  ];
  const accepted = new Set(["type", "loose"]);
  expect(aiAfterTags(existing, suggestions, accepted, "replace").map((tag) => tag.name)).toEqual([
    "New type",
    "Desk",
  ]);
  expect(aiAfterTags(existing, suggestions, accepted, "merge").map((tag) => tag.name)).toEqual([
    "Favorite",
    "Keep",
    "New type",
    "Desk",
  ]);
  expect(aiAfterTags(existing, suggestions, accepted, "append").map((tag) => tag.name)).toEqual([
    "Old type",
    "Favorite",
    "Keep",
    "New type",
    "Desk",
  ]);
});
