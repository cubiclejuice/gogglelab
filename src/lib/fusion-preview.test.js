import { expect, test } from "bun:test";
import { decodeFusionPreview } from "./fusion-preview.ts";

function response(size = 200, step = "ISO-10303-21;\n") {
  const payload = new TextEncoder().encode(step);
  const buffer = new ArrayBuffer(44 + payload.length);
  const view = new DataView(buffer);
  view.setUint32(0, 0x31443346, true);
  view.setBigUint64(4, BigInt(size), true);
  new Uint8Array(buffer, 12, 32).fill(0xab);
  new Uint8Array(buffer, 44).set(payload);
  return buffer;
}
test("Fusion preview preserves original source size and digest independently of STEP", () => {
  const result = decodeFusionPreview(response());
  expect(result.sourceSize).toBe(200);
  expect(result.sourceDigest).toBe("ab".repeat(32));
  expect(new TextDecoder().decode(result.bytes)).toBe("ISO-10303-21;\n");
});
test("Fusion preview rejects truncated, oversized and malformed responses", () => {
  expect(() => decodeFusionPreview(new ArrayBuffer(44))).toThrow("incomplete");
  expect(() => decodeFusionPreview(response(0))).toThrow("source size");
  expect(() => decodeFusionPreview(response(50 * 1024 * 1024 + 1))).toThrow("source size");
  expect(() => decodeFusionPreview(response(1, "not a STEP file"))).toThrow("STEP");
  const buffer = response();
  new Uint8Array(buffer)[0] = 0;
  expect(() => decodeFusionPreview(buffer)).toThrow("invalid preview");
});
