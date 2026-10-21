import { expect, test } from "bun:test";
import { handoffAvailability } from "./handoff-availability";

const base = {
  hasAction: true,
  hasSlicer: true,
  hasModel: true,
  format: "3mf",
};

test("offers free handoff only for supported loaded models and an installed slicer", () => {
  expect(handoffAvailability({ ...base, hasAction: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, hasSlicer: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, hasModel: false })).toBe("hidden");
  expect(handoffAvailability({ ...base, format: "step" })).toBe("hidden");
  expect(handoffAvailability({ ...base, format: "f3d" })).toBe("hidden");
  expect(handoffAvailability(base)).toBe("ready");
});
