import { expect, test } from "bun:test";
import { cadPreviewLabel, isCadPreview } from "./cad-format.ts";

test("STEP and Fusion formats share CAD preview behavior", () => {
  expect(isCadPreview("step")).toBe(true);
  expect(isCadPreview("f3d")).toBe(true);
  expect(isCadPreview("stl")).toBe(false);
  expect(isCadPreview("3mf")).toBe(false);
});

test("CAD preview labels distinguish Fusion conversion from direct STEP", () => {
  expect(cadPreviewLabel("step")).toBe("STEP preview");
  expect(cadPreviewLabel("f3d")).toBe("Fusion preview");
});
