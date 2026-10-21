import { expect, test } from "bun:test";
import {
  BoundedTaskQueue,
  isThumbnailRequestCurrent,
  shouldRetryStaleThumbnail,
} from "./browser-thumbnail-queue";

test("browser thumbnail work never exceeds its concurrency bound", async () => {
  const queue = new BoundedTaskQueue(2);
  let active = 0;
  let peak = 0;
  const releases = [];
  const tasks = Array.from({ length: 5 }, (_, index) =>
    queue.run(
      () =>
        new Promise((resolve) => {
          active += 1;
          peak = Math.max(peak, active);
          releases.push(() => {
            active -= 1;
            resolve(index);
          });
        }),
    ),
  );
  await Promise.resolve();
  while (releases.length) {
    releases.shift()();
    await Promise.resolve();
  }
  expect(await Promise.all(tasks)).toEqual([0, 1, 2, 3, 4]);
  expect(peak).toBe(2);
});

test("stale thumbnail IDs retry only one current not-found response", () => {
  expect(shouldRetryStaleThumbnail(false, "notFound", true)).toBe(true);
  expect(shouldRetryStaleThumbnail(true, "notFound", true)).toBe(false);
  expect(shouldRetryStaleThumbnail(false, "notFound", false)).toBe(false);
  expect(shouldRetryStaleThumbnail(false, "forbidden", true)).toBe(false);
});

test("a delayed thumbnail from an old connection cannot repopulate the cache", async () => {
  let generation = 1;
  const cache = new Map();
  let release;
  const delayed = new Promise((resolve) => (release = resolve));
  const requestGeneration = generation;
  const completion = delayed.then((value) => {
    if (isThumbnailRequestCurrent(requestGeneration, generation, false)) cache.set("model", value);
  });
  generation += 1;
  release("stale-url");
  await completion;
  expect(cache.size).toBe(0);
});

test("browser thumbnail queue cancels work that has not started", async () => {
  const queue = new BoundedTaskQueue(1);
  let release;
  const active = queue.run(() => new Promise((resolve) => (release = resolve)));
  const queued = queue.run(async () => "stale");
  queue.cancelPending();
  await expect(queued).rejects.toHaveProperty("name", "AbortError");
  release("done");
  expect(await active).toBe("done");
});
