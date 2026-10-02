import { describe, expect, test } from "bun:test";

import { layoutPlates, plateCameraDistance } from "./plate-layout.ts";

const bed = [256, 256, 256];

test("camera fits every plate to the horizontal field of view in narrow windows", () => {
  const dimensions = [552, 256, 64];
  const radius = Math.hypot(...dimensions) / 2;
  const distance = plateCameraDistance(dimensions, 45, 0.5);
  const halfHorizontal = Math.atan(Math.tan(Math.PI / 8) * 0.5);
  expect(radius / distance).toBeLessThan(Math.sin(halfHorizontal));
  expect(distance).toBeGreaterThan(plateCameraDistance(dimensions, 45, 1.5));
});

describe("layoutPlates", () => {
  test("places several plates in centered rows without overlapping beds", () => {
    const layout = layoutPlates(
      [
        { id: 1, stats: null },
        { id: 2, stats: null },
        { id: 3, stats: null },
        { id: 4, stats: null },
        { id: 5, stats: null },
      ],
      bed,
    );

    expect(layout.tiles).toHaveLength(5);
    expect(new Set(layout.tiles.map((tile) => tile.center.join(","))).size).toBe(5);
    expect(layout.columns).toBe(3);
    expect(layout.rows).toBe(2);
    expect(layout.bounds.dimensions[0]).toBeGreaterThanOrEqual(3 * 256);
    expect(layout.bounds.dimensions[1]).toBeGreaterThanOrEqual(2 * 256);
    expect((layout.tiles[3].center[0] + layout.tiles[4].center[0]) / 2).toBe(0);
  });

  test("uses oversized geometry dimensions to prevent plates from overlapping", () => {
    const layout = layoutPlates(
      [
        { id: 1, stats: { dimensions: [640, 120, 10] } },
        { id: 2, stats: { dimensions: [20, 700, 10] } },
      ],
      bed,
    );

    expect(layout.tileSize).toEqual([640, 700]);
    expect(Math.abs(layout.tiles[0].center[0] - layout.tiles[1].center[0])).toBeGreaterThanOrEqual(
      640,
    );
  });

  test("gives empty plates the printer-bed footprint", () => {
    const layout = layoutPlates([{ id: 9, stats: null }], [180, 300, 180]);

    expect(layout.tiles[0].footprint).toEqual([180, 300]);
    expect(layout.bounds.dimensions).toEqual([180, 300]);
  });
});
