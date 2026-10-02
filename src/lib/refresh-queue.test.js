import { expect, test } from "bun:test";
import { RefreshQueue } from "./refresh-queue";

test("one scan serves a burst of invalidations", async () => {
  let scans = 0;
  const queue = new RefreshQueue(async () => {
    scans++;
  });
  await Promise.all([queue.request(), queue.request(), queue.request()]);
  expect(scans).toBe(1);
});

test("changes during an active scan trigger a follow-up without overlap", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  let scans = 0,
    active = 0,
    maxActive = 0;
  const queue = new RefreshQueue(async () => {
    const scan = ++scans;
    maxActive = Math.max(maxActive, ++active);
    if (scan === 1) await gate;
    active--;
  });
  const first = queue.request();
  await Promise.resolve();
  const second = queue.request(),
    third = queue.request();
  release();
  await Promise.all([first, second, third]);
  expect(scans).toBe(2);
  expect(maxActive).toBe(1);
});

test("a failed scan does not disable later refreshes", async () => {
  let scans = 0;
  const queue = new RefreshQueue(async () => {
    if (++scans === 1) throw new Error("unavailable");
  });
  await expect(queue.request()).rejects.toThrow("unavailable");
  await queue.request();
  expect(scans).toBe(2);
});
