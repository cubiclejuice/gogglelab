import type { GeometryStats } from "./ipc";

export interface LayoutPlate {
  id: number;
  stats: Pick<GeometryStats, "dimensions"> | null;
}

export interface PlateTile {
  id: number;
  center: [number, number];
  footprint: [number, number];
}

export interface PlateLayout {
  columns: number;
  rows: number;
  tileSize: [number, number];
  tiles: PlateTile[];
  bounds: {
    min: [number, number];
    max: [number, number];
    dimensions: [number, number];
  };
}

/** Fit a bounding sphere to both camera axes, including narrow viewports. */
export function plateCameraDistance(
  dimensions: readonly [number, number, number],
  verticalFov: number,
  aspect: number,
): number {
  const vertical = (verticalFov * Math.PI) / 360;
  const horizontal = Math.atan(Math.tan(vertical) * Math.max(aspect, 0.001));
  return (
    Math.max(Math.hypot(...dimensions) / 2 / Math.sin(Math.min(vertical, horizontal)), 0.001) * 1.25
  );
}

/**
 * Creates a centered, uniformly-spaced XY grid for the plates in a project.
 * A tile reserves the larger of the active bed and every plate's displayed
 * geometry, so a model extending beyond its bed never collides with a neighbor.
 */
export function layoutPlates(
  plates: readonly LayoutPlate[],
  bedMm: readonly [number, number, number],
): PlateLayout {
  if (plates.length === 0) {
    return {
      columns: 0,
      rows: 0,
      tileSize: [bedMm[0], bedMm[1]],
      tiles: [],
      bounds: { min: [0, 0], max: [0, 0], dimensions: [0, 0] },
    };
  }

  const footprints = plates.map((plate): [number, number] => [
    Math.max(bedMm[0], plate.stats?.dimensions[0] ?? 0),
    Math.max(bedMm[1], plate.stats?.dimensions[1] ?? 0),
  ]);
  const tileSize: [number, number] = [
    Math.max(...footprints.map(([width]) => width)),
    Math.max(...footprints.map(([, height]) => height)),
  ];
  const columns = Math.ceil(Math.sqrt(plates.length));
  const rows = Math.ceil(plates.length / columns);
  const gap = Math.max(40, Math.min(tileSize[0], tileSize[1]) * 0.08);
  const width = columns * tileSize[0] + (columns - 1) * gap;
  const height = rows * tileSize[1] + (rows - 1) * gap;

  const tiles = plates.map((plate, index): PlateTile => {
    const column = index % columns;
    const row = Math.floor(index / columns);
    const platesInRow = Math.min(columns, plates.length - row * columns);
    const rowWidth = platesInRow * tileSize[0] + (platesInRow - 1) * gap;
    return {
      id: plate.id,
      center: [
        -rowWidth / 2 + tileSize[0] / 2 + column * (tileSize[0] + gap),
        height / 2 - tileSize[1] / 2 - row * (tileSize[1] + gap),
      ],
      footprint: footprints[index]!,
    };
  });

  return {
    columns,
    rows,
    tileSize,
    tiles,
    bounds: {
      min: [-width / 2, -height / 2],
      max: [width / 2, height / 2],
      dimensions: [width, height],
    },
  };
}
