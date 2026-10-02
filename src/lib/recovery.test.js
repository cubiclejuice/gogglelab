import { describe, expect, test } from "bun:test";
import { readRawColorwayValue } from "./recovery";

describe("readRawColorwayValue", () => {
  test("returns the exact stored string without parsing or normalizing it", () => {
    const value = " {not JSON}\ncolorway record  ";
    const storage = { getItem: (key) => (key === "gogglelab.colorways.v1" ? value : null) };

    expect(readRawColorwayValue(storage)).toBe(value);
  });

  test("returns null when the key is absent or storage is unavailable", () => {
    expect(readRawColorwayValue({ getItem: () => null })).toBeNull();
    expect(
      readRawColorwayValue({
        getItem: () => {
          throw new Error("storage blocked");
        },
      }),
    ).toBeNull();
  });
});
