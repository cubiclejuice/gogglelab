import { expect, test } from "bun:test";
import { resolveAuthoredColor } from "./authored-colors";

const part = (id, material_id = null, filament_slot = null) => ({
  id,
  name: id,
  triangle_start: 0,
  triangle_count: 1,
  material_id,
  filament_slot,
});

const summary = ({ format = "3mf", plates, materials = [], embedded_filaments = [] } = {}) => ({
  format,
  plates: plates ?? [{ id: 0, parts: [part("a")] }],
  materials,
  embedded_filaments,
});

const embedded = (color) => ({ color, source: "Embedded 3MF" });
const fallback = { color: null, source: "GoggleLab default" };

test("model target uses embedded filament slot one", () => {
  const model = summary({ embedded_filaments: [{ slot: 1, color: "#aBc123" }] });
  expect(resolveAuthoredColor({ summary: model }, { kind: "model" })).toEqual(embedded("#ABC123"));
  expect(resolveAuthoredColor({ summary: summary() }, { kind: "model" })).toEqual(fallback);
});

test("part material takes precedence over its embedded filament", () => {
  const model = summary({
    plates: [{ id: 0, parts: [part("a", "mat", 2)] }],
    materials: [{ id: "mat", color: "#123abc" }],
    embedded_filaments: [{ slot: 2, color: "#999999" }],
  });
  expect(
    resolveAuthoredColor({ summary: model, selectedPlateId: 0 }, { kind: "part", id: "a" }),
  ).toEqual(embedded("#123ABC"));
});

test("part falls back to embedded filament when material is absent or ambiguous", () => {
  for (const materials of [
    [],
    [{ id: "mat", color: "not-a-color" }],
    [
      { id: "mat", color: "#123456" },
      { id: "mat", color: "#654321" },
    ],
  ]) {
    const model = summary({
      plates: [{ id: 0, parts: [part("a", "mat", 2)] }],
      materials,
      embedded_filaments: [{ slot: 2, color: "#abcdef" }],
    });
    expect(resolveAuthoredColor({ summary: model }, { kind: "part", id: "a" })).toEqual(
      embedded("#ABCDEF"),
    );
  }
});

test("invalid and absent authored colors fall back to default", () => {
  const model = summary({
    plates: [{ id: 0, parts: [part("a", "mat", 0)] }],
    materials: [{ id: "mat", color: "#12345" }],
    embedded_filaments: [
      { slot: 0, color: "#FFFFFF" },
      { slot: 1, color: "#GGGGGG" },
    ],
  });
  expect(resolveAuthoredColor({ summary: model }, { kind: "part", id: "a" })).toEqual(fallback);
  expect(resolveAuthoredColor({ summary: model }, { kind: "model" })).toEqual(fallback);
});

test("STL and STEP never use embedded color metadata", () => {
  for (const format of ["binary", "ascii", "step"]) {
    const model = summary({
      format,
      plates: [{ id: 0, parts: [part("a", "mat", 1)] }],
      materials: [{ id: "mat", color: "#123456" }],
      embedded_filaments: [{ slot: 1, color: "#ABCDEF" }],
    });
    expect(resolveAuthoredColor({ summary: model }, { kind: "model" })).toEqual(fallback);
    expect(resolveAuthoredColor({ summary: model }, { kind: "part", id: "a" })).toEqual(fallback);
  }
});

test("selected plate scopes repeated part IDs without changing model color", () => {
  const model = summary({
    plates: [
      { id: 0, parts: [part("shared", "red")] },
      { id: 1, parts: [part("shared", "blue")] },
    ],
    materials: [
      { id: "red", color: "#FF0000" },
      { id: "blue", color: "#0000FF" },
    ],
    embedded_filaments: [{ slot: 1, color: "#00FF00" }],
  });
  expect(
    resolveAuthoredColor({ summary: model, selectedPlateId: 1 }, { kind: "part", id: "shared" }),
  ).toEqual(embedded("#0000FF"));
  expect(
    resolveAuthoredColor({ summary: model, selectedPlateId: 0 }, { kind: "part", id: "shared" }),
  ).toEqual(embedded("#FF0000"));
  expect(
    resolveAuthoredColor({ summary: model, selectedPlateId: 99 }, { kind: "part", id: "shared" }),
  ).toEqual(embedded("#00FF00"));
});
