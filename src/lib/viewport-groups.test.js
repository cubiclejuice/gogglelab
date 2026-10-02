import { expect, test } from "bun:test";
import { planGroups } from "./viewport-groups";

const part = (id, triangle_start, triangle_count, overrides = {}) => ({
  id,
  name: `Part ${id}`,
  triangle_start,
  triangle_count,
  material_id: null,
  filament_slot: null,
  ...overrides,
});

test("plans contiguous part ranges and converts triangles to vertex groups", () => {
  const parts = [part("base", 0, 2), part("infill", 2, 3), part("supports", 5, 1)];

  expect(planGroups(6, parts)).toEqual([
    { startVertex: 0, vertexCount: 6, partId: "base" },
    { startVertex: 6, vertexCount: 9, partId: "infill" },
    { startVertex: 15, vertexCount: 3, partId: "supports" },
  ]);
});

test("plans a single structural part spanning the complete model", () => {
  expect(planGroups(17, [part("single", 0, 17)])).toEqual([
    { startVertex: 0, vertexCount: 51, partId: "single" },
  ]);
});

test("rejects gaps, overlaps, and ranges extending beyond the model", () => {
  expect(planGroups(6, [part("first", 0, 2), part("gap", 3, 3)])).toBeNull();
  expect(planGroups(6, [part("first", 0, 3), part("overlap", 2, 3)])).toBeNull();
  expect(planGroups(6, [part("first", 0, 5), part("outside", 5, 2)])).toBeNull();
  expect(planGroups(6, [part("outside", 7, 1)])).toBeNull();
});

test("rejects zero-length and otherwise invalid part ranges", () => {
  expect(planGroups(1, [part("zero", 0, 0)])).toBeNull();
  expect(planGroups(1, [part("negative-start", -1, 1)])).toBeNull();
  expect(planGroups(1, [part("negative-count", 0, -1)])).toBeNull();
  expect(planGroups(1, [part("fractional-start", 0.5, 1)])).toBeNull();
  expect(planGroups(1, [part("fractional-count", 0, 0.5)])).toBeNull();
});

test("rejects missing, blank, non-string, and duplicate part ids", () => {
  expect(planGroups(1, [part("", 0, 1)])).toBeNull();
  expect(planGroups(1, [part("   ", 0, 1)])).toBeNull();
  expect(planGroups(1, [part(null, 0, 1)])).toBeNull();
  expect(planGroups(1, [part("same", 0, 1), part("same", 1, 1)])).toBeNull();
  expect(planGroups(1, [null])).toBeNull();
});

test("accepts exactly the 1024-part group limit and rejects one more", () => {
  const parts = Array.from({ length: 1024 }, (_, index) => part(`part-${index}`, index, 1));
  const groups = planGroups(1024, parts);

  expect(groups).toHaveLength(1024);
  expect(groups?.[0]).toEqual({ startVertex: 0, vertexCount: 3, partId: "part-0" });
  expect(groups?.[511]).toEqual({ startVertex: 1533, vertexCount: 3, partId: "part-511" });
  expect(groups?.[1023]).toEqual({ startVertex: 3069, vertexCount: 3, partId: "part-1023" });
  expect(planGroups(1024, [...parts, part("part-over-limit", 1024, 1)])).toBeNull();
});

test("rejects invalid model triangle counts before planning", () => {
  const oneTriangle = [part("one", 0, 1)];

  expect(planGroups(-1, oneTriangle)).toBeNull();
  expect(planGroups(0.5, oneTriangle)).toBeNull();
  expect(planGroups(Number.NaN, oneTriangle)).toBeNull();
  expect(planGroups(Number.POSITIVE_INFINITY, oneTriangle)).toBeNull();
  expect(planGroups("1", oneTriangle)).toBeNull();
  expect(planGroups(null, oneTriangle)).toBeNull();
});

test("rejects invalid range numbers and unsafe vertex arithmetic", () => {
  const safeTriangles = Math.floor(Number.MAX_SAFE_INTEGER / 3);

  expect(planGroups(1, [part("nan-start", Number.NaN, 1)])).toBeNull();
  expect(planGroups(1, [part("infinite-count", 0, Number.POSITIVE_INFINITY)])).toBeNull();
  expect(planGroups(1, [part("string-count", 0, "1")])).toBeNull();
  expect(planGroups(1, [part("unsafe-start", Number.MAX_SAFE_INTEGER, 1)])).toBeNull();
  expect(planGroups(safeTriangles, [part("safe", 0, safeTriangles)])).toEqual([
    {
      startVertex: 0,
      vertexCount: safeTriangles * 3,
      partId: "safe",
    },
  ]);
  expect(planGroups(safeTriangles + 1, [part("unsafe", 0, safeTriangles + 1)])).toBeNull();
});
