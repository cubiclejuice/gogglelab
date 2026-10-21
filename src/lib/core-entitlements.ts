import type { ProEntitlements } from "./app-extensions";

/** Free features stay enabled regardless of Pro license state. */
export function withCoreEntitlements(
  entitlements?: Partial<ProEntitlements> | null,
): ProEntitlements {
  return {
    library: true,
    colorways: entitlements?.colorways === true,
    slicerHandoff: true,
    revision: entitlements?.revision ?? 0,
  };
}
