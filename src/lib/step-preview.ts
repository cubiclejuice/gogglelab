import type { StepPreview } from "./step-mesh";

const STEP_TIMEOUT_MS = 60_000;

/** Run the CAD kernel off the UI thread and stop abandoned/long-running loads. */
export function previewStep(bytes: ArrayBuffer, signal: AbortSignal): Promise<StepPreview> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./step-worker.ts", import.meta.url), { type: "module" });
    let settled = false;
    const cleanup = () => {
      if (settled) return false;
      settled = true;
      window.clearTimeout(timer);
      signal.removeEventListener("abort", onAbort);
      worker.terminate();
      return true;
    };
    const onAbort = () => {
      if (cleanup()) reject(new DOMException("STEP load canceled", "AbortError"));
    };
    const timer = window.setTimeout(() => {
      if (cleanup()) reject(new Error("STEP preview timed out after 60 seconds."));
    }, STEP_TIMEOUT_MS);
    signal.addEventListener("abort", onAbort, { once: true });
    worker.onmessage = ({ data }: MessageEvent<StepPreview | { error: string }>) => {
      if (!cleanup()) return;
      if ("error" in data) reject(new Error(data.error));
      else resolve(data);
    };
    worker.onerror = () => {
      if (cleanup()) reject(new Error("The STEP preview worker could not start."));
    };
    if (signal.aborted) onAbort();
    else worker.postMessage(bytes, [bytes]);
  });
}
