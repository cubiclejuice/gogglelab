export class BoundedTaskQueue {
  private active = 0;
  private readonly pending: Array<{ start: () => void; reject: (reason: unknown) => void }> = [];

  constructor(private readonly limit: number) {
    if (!Number.isInteger(limit) || limit < 1) throw new Error("Task queue limit must be positive");
  }

  run<T>(task: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const start = () => {
        this.active += 1;
        void task()
          .then(resolve, reject)
          .finally(() => {
            this.active -= 1;
            this.pending.shift()?.start();
          });
      };
      if (this.active < this.limit) start();
      else this.pending.push({ start, reject });
    });
  }

  cancelPending(reason: unknown = new DOMException("Thumbnail request superseded", "AbortError")) {
    for (const pending of this.pending.splice(0)) pending.reject(reason);
  }
}

export function isThumbnailRequestCurrent(
  requestGeneration: number,
  currentGeneration: number,
  aborted: boolean,
): boolean {
  return requestGeneration === currentGeneration && !aborted;
}

export function shouldRetryStaleThumbnail(
  retried: boolean,
  errorCode: string | undefined,
  requestCurrent: boolean,
): boolean {
  return !retried && errorCode === "notFound" && requestCurrent;
}
