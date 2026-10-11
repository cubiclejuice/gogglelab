import { expect, test } from "bun:test";
import { libraryServerStatusCopy, shouldRevealAccessKey } from "./library-server-settings";

test("desktop server settings clearly distinguish stopped and loopback-running states", () => {
  expect(libraryServerStatusCopy({ running: false, baseUrl: null, mcpUrl: null })).toEqual({
    state: "Stopped",
    address: "Not accepting connections",
    action: "Start server",
  });
  expect(
    libraryServerStatusCopy({
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: "http://127.0.0.1:3745/mcp",
    }),
  ).toEqual({
    state: "Running on this computer",
    address: "http://127.0.0.1:3745",
    action: "Stop server",
  });
});

test("a delayed access-key response cannot reappear after stop, refresh, or dialog close", () => {
  expect(shouldRevealAccessKey(3, 3, true, true)).toBe(true);
  expect(shouldRevealAccessKey(3, 4, true, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, false, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, true, false)).toBe(false);
});
