import { expect, test } from "bun:test";
import { convertStepMesh } from "./step-mesh.ts";

test("STEP tessellation becomes a bounded millimeter-scale viewport mesh", () => {
  const result = convertStepMesh({
    positions: new Float32Array([0, 0, 0, 10, 0, 0, 0, 20, 0]),
    indices: new Uint32Array([0, 1, 2]),
  });
  expect(result.geometry.triangleCount).toBe(1);
  expect([...result.geometry.positions]).toEqual([0, 0, 0, 10, 0, 0, 0, 20, 0]);
  expect(result.stats.dimensions).toEqual([10, 20, 0]);
  expect(result.stats.surface_area_mm2).toBe(100);
});

test("STEP tessellation rejects invalid indices before rendering", () => {
  expect(() =>
    convertStepMesh({
      positions: new Float32Array([0, 0, 0]),
      indices: new Uint32Array([0, 1, 2]),
    }),
  ).toThrow("invalid vertex index");
});
