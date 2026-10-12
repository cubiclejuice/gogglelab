const MAGIC = 0x324c5453;

export interface DecodedGeometry {
  triangleCount: number;
  positions: Float32Array;
}

export function normalizeArrayBuffer(raw: ArrayBuffer | Uint8Array | number[]): ArrayBuffer {
  if (raw instanceof ArrayBuffer) return raw;
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

export function decodeGeometryBuffer(raw: ArrayBuffer | Uint8Array | number[]): DecodedGeometry {
  const buffer = normalizeArrayBuffer(raw);
  if (buffer.byteLength < 16) throw new Error("geometry buffer is truncated");
  const header = new DataView(buffer);
  const magic = header.getUint32(0, true);
  if (magic !== MAGIC) throw new Error(`bad geometry buffer magic: 0x${magic.toString(16)}`);
  const triangleCount = header.getUint32(8, true);
  const expected = 16 + triangleCount * 9 * Float32Array.BYTES_PER_ELEMENT;
  if (!Number.isSafeInteger(expected) || buffer.byteLength !== expected)
    throw new Error("geometry buffer length does not match its triangle count");
  return { triangleCount, positions: new Float32Array(buffer, 16, triangleCount * 9) };
}
