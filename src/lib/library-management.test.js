import { expect, test } from "bun:test";
import {
  bundlePreviewCapacity,
  chooseRouletteModel,
  explicitScopeIds,
  partitionBundleGroups,
  removalTargets,
} from "./library-management";
import { isCurrentBundleRequest } from "./library-bundle-dialog";

const model = (id, overrides = {}) => ({
  id,
  metadata_version: 1,
  source_missing: false,
  ...overrides,
});

test("management scope excludes unavailable models and snapshots versions", () => {
  const models = [model("a", { metadata_version: 3 }), model("missing", { source_missing: true })];
  expect(explicitScopeIds(models)).toEqual(["a"]);
  expect(removalTargets(models)).toEqual([
    { modelId: "a", expectedVersion: 3 },
    { modelId: "missing", expectedVersion: 1 },
  ]);
});

test("roulette is deterministic for tests and never chooses unavailable models", () => {
  const models = [model("missing", { source_missing: true }), model("a"), model("b")];
  expect(chooseRouletteModel(models, () => 0)?.id).toBe("a");
  expect(chooseRouletteModel(models, () => 0.999)?.id).toBe("b");
});

test("only two-plus bundle groups collapse while singletons remain normal rows", () => {
  const result = partitionBundleGroups([
    model("a", { bundle_id: "bundle" }),
    model("b", { bundle_id: "bundle" }),
    model("single", { bundle_id: "singleton" }),
    model("loose"),
  ]);
  expect(result.groups.map((group) => group.models.map((item) => item.id))).toEqual([["a", "b"]]);
  expect(result.singles.map((item) => item.id)).toEqual(["single", "loose"]);
});

test("bundle preview capacity is exactly 32 with an explicit omitted count", () => {
  const models = Array.from({ length: 35 }, (_, index) => model(String(index)));
  expect(bundlePreviewCapacity(models).visible).toHaveLength(32);
  expect(bundlePreviewCapacity(models).omitted).toBe(3);
});

test("stale bundle geometry responses cannot replace a superseding preview", () => {
  expect(isCurrentBundleRequest("new-token", "old-token")).toBe(false);
  expect(isCurrentBundleRequest("new-token", "new-token")).toBe(true);
  expect(isCurrentBundleRequest(null, "old-token")).toBe(false);
});
