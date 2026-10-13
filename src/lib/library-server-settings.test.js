import { expect, test } from "bun:test";
import {
  libraryServerExternalIntegrationChoice,
  libraryServerStatusCopy,
  shouldRevealAccessKey,
} from "./library-server-settings";

test("desktop server settings show external integration access off by default", () => {
  expect(
    libraryServerStatusCopy({
      running: false,
      baseUrl: null,
      mcpUrl: null,
      allowExternalIntegrations: false,
    }),
  ).toEqual({
    state: "Stopped",
    address: "Not accepting connections",
    action: "Start server",
    externalIntegrations: "Off",
  });
  expect(
    libraryServerStatusCopy({
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: "http://127.0.0.1:3745/mcp",
      allowExternalIntegrations: true,
    }),
  ).toEqual({
    state: "Running on this computer",
    address: "http://127.0.0.1:3745",
    action: "Stop server",
    externalIntegrations: "On for this session",
  });
});

test("a delayed access-key response cannot reappear after stop, refresh, or dialog close", () => {
  expect(shouldRevealAccessKey(3, 3, true, true)).toBe(true);
  expect(shouldRevealAccessKey(3, 4, true, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, false, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, true, false)).toBe(false);
});

test("refresh preserves an explicit next-start choice while stopped", () => {
  const stopped = {
    running: false,
    baseUrl: null,
    mcpUrl: null,
    allowExternalIntegrations: false,
  };
  expect(libraryServerExternalIntegrationChoice(stopped, true, true)).toBe(true);
  expect(libraryServerExternalIntegrationChoice(stopped, true, false)).toBe(false);
});

test("refresh follows the live server policy and discards a pending next-start choice", () => {
  const running = {
    running: true,
    baseUrl: "http://127.0.0.1:3745",
    mcpUrl: "http://127.0.0.1:3745/mcp",
    allowExternalIntegrations: false,
  };
  expect(libraryServerExternalIntegrationChoice(running, true, true)).toBe(false);
});
