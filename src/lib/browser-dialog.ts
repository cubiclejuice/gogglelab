export function isBrowserDialogCurrent(
  request: number,
  currentRequest: number,
  open: boolean,
  connected: boolean,
): boolean {
  return request === currentRequest && open && connected;
}
