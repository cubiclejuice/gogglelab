import { OcctKernel } from "occt-wasm";
import wasmUrl from "occt-wasm/dist/occt-wasm.wasm?url";
import { convertStepMesh, type StepPreview } from "./step-mesh";

type StepWorkerScope = {
  postMessage(message: StepPreview | { error: string }, transfer?: Transferable[]): void;
  onmessage: ((event: MessageEvent<ArrayBuffer>) => void) | null;
};

const scope = self as unknown as StepWorkerScope;

scope.onmessage = async ({ data: bytes }) => {
  try {
    const digestBytes = await crypto.subtle.digest("SHA-256", bytes);
    const digest = Array.from(new Uint8Array(digestBytes), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const kernel = await OcctKernel.init({ wasm: wasmUrl }).catch(() => {
      throw new Error(
        "The STEP engine could not start. STEP preview requires WebKit compatible with Safari 17.2 or later; check your macOS update and app installation.",
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
