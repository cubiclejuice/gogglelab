/** Latest-operation guard for Library rendering and status messages. */
export class LibraryOperationGuard {
  private revision = 0;

  begin(): number {
    return ++this.revision;
  }

  isCurrent(revision: number): boolean {
    return revision === this.revision;
  }
}
