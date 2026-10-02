export interface LibraryRequestToken {
  readonly generation: number;
  readonly entitlementRevision: number;
}

export interface LibraryRequestState {
  readonly generation: number;
  readonly entitlementRevision: number;
  readonly entitled: boolean;
}

/** Reject responses admitted under a request, viewer, or entitlement state that changed while awaiting. */
export function isLibraryRequestCurrent(
  request: LibraryRequestToken,
  pending: LibraryRequestToken | null,
  current: LibraryRequestState,
): boolean {
  return (
    pending?.generation === request.generation &&
    pending.entitlementRevision === request.entitlementRevision &&
    current.generation === request.generation &&
    current.entitlementRevision === request.entitlementRevision &&
    current.entitled
  );
}
