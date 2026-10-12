import { expect, test } from "bun:test";
import { decodeGeometryBuffer } from "./geometry-wire";

function packet(triangles, positions = []) {
  const buffer = new ArrayBuffer(16 + positions.length * 4);
  const view = new DataView(buffer);
  view.setUint32(0, 0x324c5453, true);
  view.setUint32(8, triangles, true);
  new Float32Array(buffer, 16).set(positions);
  return buffer;
}

test("browser-safe STL2 decoder accepts exact packets and rejects truncated lengths", () => {
  const values = Array.from({ length: 9 }, (_, index) => index + 0.5);
  const decoded = decodeGeometryBuffer(packet(1, values));
  expect(decoded.triangleCount).toBe(1);
  expect([...decoded.positions]).toEqual(values);
  expect(() => decodeGeometryBuffer(packet(2, values))).toThrow("length");
  expect(() => decodeGeometryBuffer(new ArrayBuffer(8))).toThrow("truncated");
});
