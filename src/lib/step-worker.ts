import { OcctKernel } from "occt-wasm";
import { convertStepMesh, type StepPreview } from "./step-mesh";

type StepWorkerScope = {
  postMessage(message: StepPreview | { error: string }, transfer?: Transferable[]): void;
  onmessage: ((event: MessageEvent<{ step: ArrayBuffer; wasm: ArrayBuffer }>) => void) | null;
};

const scope = self as unknown as StepWorkerScope;

scope.onmessage = async ({ data: { step: bytes, wasm } }) => {
  try {
    const digestBytes = await crypto.subtle.digest("SHA-256", bytes);
    const digest = Array.from(new Uint8Array(digestBytes), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const kernel = await OcctKernel.init({ wasm }).catch(() => {
      throw new Error(
        "The STEP engine could not start. Update the system WebView/runtime and check your CAD-engine installation. STEP requires WebAssembly SIMD, tail calls and exceptions.",
      );
    });
    const shape = kernel.importStep(bytes);
    const mesh = kernel.tessellate(shape, {
      linearDeflection: 0.002,
      angularDeflection: 0.5,
      relative: true,
    });
    const { geometry, stats } = convertStepMesh(mesh);
    scope.postMessage({ geometry, stats, digest }, [geometry.positions.buffer]);
  } catch (error) {
    scope.postMessage({
      error: error instanceof Error ? error.message : "Unknown STEP import error",
    });
  }
};
