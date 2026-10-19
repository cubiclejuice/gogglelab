import { expect, test } from "bun:test";
import { printerNetworkDraftPolicy } from "./desktop-printer-network-settings";

test("disabled host printer access cannot inherit a range or datagram permissions", () => {
  expect(printerNetworkDraftPolicy(false, "not a range", true, true)).toBeNull();
});

test("host printer access requires selected scopes and independent datagram choices", () => {
  expect(() => printerNetworkDraftPolicy(true, " \n ", false, false)).toThrow();
  expect(printerNetworkDraftPolicy(true, " 10.250.0.1/32\n\n10.250.0.2/32 ", true, false)).toEqual({
    allowedScopes: ["10.250.0.1/32", "10.250.0.2/32"],
    allowMulticastDiscovery: true,
    allowCarbonBroadcast: false,
  });
  expect(printerNetworkDraftPolicy(true, "10.250.0.1/32", false, true)).toEqual({
    allowedScopes: ["10.250.0.1/32"],
    allowMulticastDiscovery: false,
    allowCarbonBroadcast: true,
  });
});

test("host policy drafts are bounded before native validation", () => {
  expect(() => printerNetworkDraftPolicy(true, "x".repeat(4097), false, false)).toThrow();
  expect(() =>
    printerNetworkDraftPolicy(true, Array(33).fill("10.250.0.1/32").join("\n"), false, false),
  ).toThrow();
});
