import { expect, test } from "bun:test";
import { handoffAvailability } from "./handoff-availability";

const base = {
  hasAction: true,
  hasSlicer: true,
  hasModel: true,
  format: "3mf",
  entitled: true,
};

test("keeps handoff out of Community and offers it only for supported loaded models", () => {
  expect(handoffAvailability({ ...base, hasAction: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, hasSlicer: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, hasModel: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, format: "step" })).toBe("hidden");
  expect(handoffAvailability({ ...base, format: "f3d" })).toBe("hidden");
  expect(handoffAvailability(base)).toBe("ready");
});

test("keeps the official handoff entrypoint visible but locked without its entitlement", () => {
  expect(handoffAvailability({ ...base, entitled: false })).toBe("locked");
});
