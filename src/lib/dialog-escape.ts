export function pathContainsOpenDialog(path: EventTarget[]): boolean {
  return path.some((target) => target instanceof HTMLDialogElement && target.open);
}
