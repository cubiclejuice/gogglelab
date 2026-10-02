export type HandoffAvailability = "hidden" | "locked" | "ready";

import { isCadPreview } from "./cad-format";

export interface HandoffAvailabilityInput {
  hasAction: boolean;
  hasSlicer: boolean;
  hasModel: boolean;
  format: "binary" | "ascii" | "3mf" | "step" | "f3d" | null;
  entitled: boolean;
}

export function handoffAvailability(input: HandoffAvailabilityInput): HandoffAvailability {
  if (
    !input.hasAction ||
    !input.hasSlicer ||
    !input.hasModel ||
    input.format === null ||
    isCadPreview(input.format)
  ) {
    return "hidden";
  }
  return input.entitled ? "ready" : "locked";
}
