import type { Mesh } from "occt-wasm";
import type { DecodedGeometry, GeometryStats } from "./ipc";

const MAX_STEP_TRIANGLES = 2_000_000;

export interface StepPreview {
  geometry: DecodedGeometry;
  stats: GeometryStats;
  digest: string;
}

/** Convert a CAD surface mesh to the viewer's non-indexed triangle format. */
export function convertStepMesh(mesh: Pick<Mesh, "positions" | "indices">): {
  geometry: DecodedGeometry;
  stats: GeometryStats;
} {
  if (mesh.positions.length % 3 !== 0 || mesh.indices.length % 3 !== 0) {
    throw new Error("The STEP tessellator returned invalid geometry.");
  }
  const triangleCount = mesh.indices.length / 3;
  if (triangleCount === 0) throw new Error("This STEP file has no viewable surfaces.");
  if (triangleCount > MAX_STEP_TRIANGLES) {
    throw new Error(`This STEP preview exceeds ${MAX_STEP_TRIANGLES.toLocaleString()} triangles.`);
  }

  const positions = new Float32Array(triangleCount * 9);
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  let area = 0;
  let signedVolume = 0;

  for (let triangle = 0; triangle < triangleCount; triangle++) {
    const corners: number[][] = [];
    for (let corner = 0; corner < 3; corner++) {
      const vertex = mesh.indices[triangle * 3 + corner];
      const offset = vertex * 3;
      if (offset + 2 >= mesh.positions.length) {
        throw new Error("The STEP tessellator returned an invalid vertex index.");
      }
      const xyz = [mesh.positions[offset], mesh.positions[offset + 1], mesh.positions[offset + 2]];
      if (!xyz.every(Number.isFinite)) {
        throw new Error("The STEP tessellator returned a non-finite coordinate.");
      }
      corners.push(xyz);
      for (let axis = 0; axis < 3; axis++) {
        const value = xyz[axis];
        positions[triangle * 9 + corner * 3 + axis] = value;
        min[axis] = Math.min(min[axis], value);
        max[axis] = Math.max(max[axis], value);
      }
    }
    const [a, b, c] = corners;
    const ux = b[0] - a[0],
      uy = b[1] - a[1],
      uz = b[2] - a[2];
    const vx = c[0] - a[0],
      vy = c[1] - a[1],
      vz = c[2] - a[2];
    const nx = uy * vz - uz * vy;
    const ny = uz * vx - ux * vz;
    const nz = ux * vy - uy * vx;
    area += Math.hypot(nx, ny, nz) / 2;
    signedVolume +=
      (a[0] * (b[1] * c[2] - b[2] * c[1]) +
        a[1] * (b[2] * c[0] - b[0] * c[2]) +
        a[2] * (b[0] * c[1] - b[1] * c[0])) /
      6;
  }

  return {
    geometry: { triangleCount, positions },
    stats: {
      bbox: { min, max },
      dimensions: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
      surface_area_mm2: area,
      volume_mm3: Math.abs(signedVolume),
    },
  };
}
