export interface LibraryRequestToken {
  readonly generation: number;
}

export interface LibraryRequestState {
  readonly generation: number;
}

/** Reject responses from a superseded Library request or viewer generation. */
export function isLibraryRequestCurrent(
  request: LibraryRequestToken,
  pending: LibraryRequestToken | null,
  current: LibraryRequestState,
): boolean {
  return pending?.generation === request.generation && current.generation === request.generation;
}
