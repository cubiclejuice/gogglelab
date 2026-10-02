/** Native Fusion response: F3D1, original u64 byte size, SHA-256, STEP payload. */
export interface FusionPreviewSource {
  bytes: ArrayBuffer;
  sourceSize: number;
  sourceDigest: string;
}
export function decodeFusionPreview(raw: ArrayBuffer): FusionPreviewSource {
  if (raw.byteLength <= 44) throw new Error("Fusion returned an incomplete preview.");
  const view = new DataView(raw);
  if (view.getUint32(0, true) !== 0x31443346) {
    throw new Error("Fusion returned an invalid preview.");
  }
  const sourceSize = Number(view.getBigUint64(4, true));
  if (!Number.isSafeInteger(sourceSize) || sourceSize <= 0 || sourceSize > 50 * 1024 * 1024) {
    throw new Error("Fusion returned an invalid source size.");
  }
  const sourceDigest = Array.from(new Uint8Array(raw, 12, 32), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  const bytes = raw.slice(44);
  if (bytes.byteLength > 25 * 1024 * 1024) throw new Error("Fusion preview exceeds 25 MiB.");
  const header = new TextDecoder().decode(bytes.slice(0, 256)).trimStart();
  if (!header.startsWith("ISO-10303-21;"))
    throw new Error("Fusion returned an invalid STEP preview.");
  return { bytes, sourceSize, sourceDigest };
}
