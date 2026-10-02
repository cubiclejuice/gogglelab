import { expect, test } from "bun:test";
import { PlateSession } from "./plate-session";

const geometry = (id) => ({ triangleCount: 1, positions: new Float32Array([id]) });
const stats = {
  bbox: { min: [0, 0, 0], max: [10, 10, 10] },
  dimensions: [10, 10, 10],
  surface_area_mm2: 600,
  volume_mm3: 1000,
};
const summary = (generation) => ({
  generation,
  file_name: "plates.3mf",
  file_size_bytes: 1,
  format: "3mf",
  triangle_count: 2,
  geometry: stats,
  parse_ms: 1,
  plate_metadata: true,
  plate_warning: null,
  plates: [
    {
      id: 0,
      name: "First",
      triangle_count: 1,
      geometry: stats,
      parts: [{ id: `part-${generation}-0`, name: "Part", triangle_start: 0, triangle_count: 1 }],
    },
    {
      id: 1,
      name: "Second",
      triangle_count: 1,
      geometry: stats,
      parts: [{ id: `part-${generation}-1`, name: "Part", triangle_start: 0, triangle_count: 1 }],
    },
  ],
});
const deferred = () => {
  let resolve;
  const promise = new Promise((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

test("selects first populated plate and caches geometry across overview toggles", async () => {
  const calls = [];
  const session = new PlateSession(async (gen, id) => {
    calls.push([gen, id]);
    return geometry(id);
  });
  session.reset(1);
  await session.load(summary(1));
  expect(session.view.plate.id).toBe(0);
  await session.setOverview(true);
  expect(session.view.overview).toBe(true);
  expect(session.view.plates.length).toBe(2);
  await session.select(1);
  expect(session.view.plate.id).toBe(1);
  await session.setOverview(false);
  expect(session.view.overview).toBe(false);
  expect(calls).toEqual([
    [1, 0],
    [1, 1],
  ]);
});

test("late selection cannot overwrite a newer selection", async () => {
  const second = deferred();
  const session = new PlateSession(async (_gen, id) => (id === 1 ? second.promise : geometry(id)));
  session.reset(1);
  await session.load(summary(1));
  const slow = session.select(1);
  await session.select(0);
  second.resolve(geometry(1));
  await slow;
  expect(session.view.plate.id).toBe(0);
});

test("opening another file discards old geometry and integrity results", async () => {
  const old = deferred();
  const session = new PlateSession(async (gen, id) => (gen === 1 ? old.promise : geometry(id)));
  session.reset(1);
  const slow = session.load(summary(1));
  session.reset(2);
  await session.load(summary(2));
  old.resolve(geometry(99));
  await slow;
  session.receiveIntegrity({
    generation: 1,
    plate_id: 0,
    status: "SkippedTooLarge",
    triangle_count: 1,
    limit: 1,
  });
  expect(session.view.summary.generation).toBe(2);
  expect(session.view.plates[0].geometry.positions[0]).toBe(0);
  expect(session.view.integrity).toBe(null);
});

test("late geometry cannot attach stale plate parts to the current view", async () => {
  const old = deferred();
  const session = new PlateSession(async (generation) =>
    generation === 1 ? old.promise : geometry(2),
  );
  session.reset(1);
  const stale = session.load(summary(1));
  session.reset(2);
  await session.load(summary(2));
  old.resolve(geometry(1));
  await stale;
  expect(session.view.summary.generation).toBe(2);
  expect(session.view.plates[0].parts.map((part) => part.id)).toEqual(["part-2-0"]);
});

test("early integrity results are buffered per plate and restored on selection", async () => {
  const session = new PlateSession(async (_gen, id) => geometry(id));
  session.reset(1);
  const first = {
    generation: 1,
    plate_id: 0,
    status: "SkippedTooLarge",
    triangle_count: 1,
    limit: 1,
  };
  const second = { ...first, plate_id: 1, limit: 2 };
  session.receiveIntegrity(first);
  session.receiveIntegrity(second);
  await session.load(summary(1));
  expect(session.view.integrity).toEqual(first);
  await session.select(1);
  expect(session.view.integrity).toEqual(second);
});

test("empty plates remain selectable without fetching geometry", async () => {
  const calls = [];
  const session = new PlateSession(async (_gen, id) => {
    calls.push(id);
    return geometry(id);
  });
  const file = summary(1);
  file.plates[0] = { ...file.plates[0], triangle_count: 0, geometry: null };
  session.reset(1);
  await session.load(file);
  expect(session.view.plate.id).toBe(1);
  await session.select(0);
  expect(session.view.plate.geometry).toBe(null);
  expect(calls).toEqual([1]);
});

test("a failed geometry request preserves the displayed plate and can be retried", async () => {
  let fail = true;
  const session = new PlateSession(async (_gen, id) => {
    if (id === 1 && fail) throw new Error("read failed");
    return geometry(id);
  });
  session.reset(1);
  await session.load(summary(1));
  await expect(session.select(1)).rejects.toThrow("read failed");
  expect(session.view.plate.id).toBe(0);
  fail = false;
  await session.select(1);
  expect(session.view.plate.id).toBe(1);
});
