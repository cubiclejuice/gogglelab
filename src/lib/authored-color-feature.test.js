import { describe, expect, test } from "bun:test";
import { AuthoredColorSession } from "./authored-color-feature";

function summary(generation, { format = "3mf" } = {}) {
  return {
    generation,
    format,
    source_identity: `model-${generation}`,
    source_digest: "a".repeat(64),
    plates: [
      {
        id: 1,
        name: "Plate 1",
        triangle_count: 2,
        parts: [{ id: "part-1", name: "Badge", material_id: "mat-1", filament_slot: 2 }],
      },
    ],
    materials: [{ id: "mat-1", name: "Blue", color: "#1122aa" }],
    embedded_filaments: [
      { slot: 1, name: "Model", color: "#aaaaaa" },
      { slot: 2, name: "Red", color: "#dd2233" },
    ],
  };
}

describe("AuthoredColorSession", () => {
  test("renders only authored 3MF colors and defaults for other formats", () => {
    const session = new AuthoredColorSession();
    session.load(summary(1));

    expect(session.resolveColor({ kind: "model" })).toEqual({
      color: "#AAAAAA",
      source: "Embedded 3MF",
    });
    expect(session.resolveColor({ kind: "part", id: "part-1" })).toEqual({
      color: "#1122AA",
      source: "Embedded 3MF",
    });

    session.load(summary(2, { format: "step" }));
    expect(session.resolveColor({ kind: "model" })).toEqual({
      color: null,
      source: "GoggleLab default",
    });
  });

  test("ignores stale summaries and unknown parts", () => {
    const session = new AuthoredColorSession();
    session.load(summary(2));
    session.load(summary(1));

    expect(session.resolveColor({ kind: "model" }).color).toBe("#AAAAAA");
    expect(session.resolveColor({ kind: "part", id: "missing" }).color).toBe("#AAAAAA");
  });

  test("keeps viewing mode as session state without writing saved assignments", () => {
    const session = new AuthoredColorSession();
    session.load(summary(1));
    session.setMode("model");
    session.load(summary(2));

    expect(session.mode).toBe("model");
    expect(session.resolveColor({ kind: "part", id: "part-1" }).source).toBe("Embedded 3MF");
  });
});
