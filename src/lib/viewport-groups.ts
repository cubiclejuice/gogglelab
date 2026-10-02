import type { PartSummary } from "./ipc";

const MAX_GROUPS = 1024;
const VERTICES_PER_TRIANGLE = 3;

export interface ViewportGroup {
  startVertex: number;
  vertexCount: number;
  partId: string;
}

/**
 * Convert contiguous, plate-local triangle ranges into Three.js vertex groups.
 *
 * The returned ranges cover the complete position buffer exactly once. Invalid
 * metadata returns null so callers can fall back to a model-wide material.
 */
export function planGroups(
  triangleCount: number,
  parts: readonly PartSummary[],
): ViewportGroup[] | null {
  if (!Number.isSafeInteger(triangleCount) || triangleCount < 0) return null;
  if (!Array.isArray(parts) || parts.length > MAX_GROUPS) return null;

  if (triangleCount === 0) return parts.length === 0 ? [] : null;
  if (parts.length === 0) return null;

  // Every triangle contributes three vertices; reject counts that cannot be
  // converted to safe JavaScript integer offsets before doing any arithmetic.
  if (triangleCount > Math.floor(Number.MAX_SAFE_INTEGER / VERTICES_PER_TRIANGLE)) {
    return null;
  }

  const groups: ViewportGroup[] = [];
  const partIds = new Set<string>();
  let coveredTriangles = 0;

  for (const part of parts) {
    if (!part || typeof part !== "object") return null;

    const { id, triangle_start: triangleStart, triangle_count: partTriangleCount } = part;
    if (typeof id !== "string" || id.trim().length === 0 || partIds.has(id)) return null;
    if (!Number.isSafeInteger(triangleStart) || triangleStart < 0) return null;
    if (!Number.isSafeInteger(partTriangleCount) || partTriangleCount <= 0) return null;

    // Requiring the next range to begin at the cursor rejects both gaps and
    // overlaps while preserving the source order in the resulting groups.
    if (triangleStart !== coveredTriangles) return null;
    if (partTriangleCount > triangleCount - coveredTriangles) return null;

    const startVertex = triangleStart * VERTICES_PER_TRIANGLE;
    const vertexCount = partTriangleCount * VERTICES_PER_TRIANGLE;
    if (!Number.isSafeInteger(startVertex) || !Number.isSafeInteger(vertexCount)) return null;

    groups.push({ startVertex, vertexCount, partId: id });
    partIds.add(id);
    coveredTriangles += partTriangleCount;
  }

  return coveredTriangles === triangleCount ? groups : null;
}
