/** Coalesce invalidations without losing changes that arrive during a scan. */
export class RefreshQueue {
  private waiting: Array<{ resolve: () => void; reject: (error: unknown) => void }> = [];
  private running = false;

  constructor(private scan: () => Promise<void>) {}

  request(): Promise<void> {
    const result = new Promise<void>((resolve, reject) => this.waiting.push({ resolve, reject }));
    if (!this.running) {
      this.running = true;
      queueMicrotask(() => void this.drain());
    }
    return result;
  }

  private async drain() {
    while (this.waiting.length > 0) {
      const batch = this.waiting.splice(0);
      try {
        await this.scan();
        for (const waiter of batch) waiter.resolve();
      } catch (error) {
        for (const waiter of batch) waiter.reject(error);
      }
    }
    this.running = false;
  }
}
