import type {
  ActionPlan,
  ThumbnailCleanupMode,
  ThumbnailCleanupRequest,
  ThumbnailProgress,
} from "./library-client";

export const UNCONFIRMED_MANAGEMENT_OUTCOME =
  "This action could not be fully confirmed. Some work may have completed. Review the current Library before trying again.";

export interface ThumbnailCleanupOutcome {
  files: string | number;
  bytes: string | number;
  thumbnailProgress?: ThumbnailProgress | null;
}
export interface ThumbnailCleanupAdapter {
  review(
    request: ThumbnailCleanupRequest,
  ): Promise<Pick<ActionPlan, "token" | "expiresAt" | "thumbnailReview">>;
  execute(token: string): Promise<ThumbnailCleanupOutcome>;
  discard(token: string): Promise<void>;
}

/** Each bounded review owns a distinct, explicitly confirmed action token. */
export class ThumbnailCleanupDialog {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private readonly status: HTMLElement;
  private readonly cancel: HTMLButtonElement;
  private invoker: HTMLElement | null = null;
  private revision = 0;
  private ownedToken: string | null = null;
  private pending = false;
  private executing = false;

  constructor(
    container: HTMLElement,
    private readonly adapter: ThumbnailCleanupAdapter,
    private readonly callbacks: {
      onChanged?: () => Promise<void>;
      onNotice?: (text: string) => void;
      focusFallback?: () => void;
    } = {},
  ) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-editor-dialog thumbnail-cleanup-dialog";
    this.dialog.setAttribute("aria-label", "Review preview cleanup");
    this.body = document.createElement("section");
    this.status = document.createElement("p");
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");
    this.cancel = this.button("Cancel", () => this.dismiss());
    this.dialog.append(this.body, this.status, this.cancel);
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      if (!this.pending) this.dismiss();
    });
    this.dialog.addEventListener("close", () => {
      this.invalidate();
      if (this.invoker?.isConnected) this.invoker.focus();
      else this.callbacks.focusFallback?.();
      this.invoker = null;
    });
    container.append(this.dialog);
  }

  show(mode: ThumbnailCleanupMode): void {
    if (this.executing) {
      this.callbacks.onNotice?.("A reviewed preview operation is still finishing.");
      return;
    }
    this.invalidate();
    this.invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!this.dialog.open) this.dialog.showModal();
    this.cancel.disabled = false;
    this.cancel.focus();
    void this.review(mode, null);
  }
  dismiss(): void {
    this.invalidate();
    if (this.dialog.open) this.dialog.close();
  }
  private invalidate(): void {
    this.revision++;
    this.pending = false;
    const token = this.ownedToken;
    this.ownedToken = null;
    if (token) void this.discard(token);
  }
  private async discard(token: string): Promise<void> {
    try {
      await this.adapter.discard(token);
    } catch {
      this.callbacks.onNotice?.(
        "Release of the preview review could not be confirmed. It expires after five minutes; the host reclaims expired reviews on its next prune or shutdown.",
      );
    }
  }
  private current(revision: number): boolean {
    return revision === this.revision && this.dialog.open && this.dialog.isConnected;
  }
  private async review(mode: ThumbnailCleanupMode, continuation: string | null): Promise<void> {
    this.invalidate();
    const revision = this.revision;
    this.body.replaceChildren();
    this.status.textContent = "Reviewing previews…";
    try {
      const plan = await this.adapter.review({ mode, continuation });
      if (!this.current(revision)) {
        await this.discard(plan.token);
        return;
      }
      if (!plan.thumbnailReview || plan.thumbnailReview.mode !== mode) {
        await this.discard(plan.token);
        if (this.current(revision))
          this.status.textContent =
            "This host did not return a compatible preview review. No cleanup was requested.";
        return;
      }
      this.ownedToken = plan.token;
      const batch = plan.thumbnailReview;
      const title = document.createElement("h2");
      title.textContent =
        mode === "resetDefaults" ? "Reset default preview entries" : "Review unused previews";
      const description = document.createElement("p");
      description.textContent =
        mode === "resetDefaults"
          ? `Reset ${batch.defaultRecordsToClear} default preview entries. All files and custom previews will be kept. A separate review can check unused files afterward.`
          : `Reviewed ${batch.reviewedFiles} files. Can remove ${batch.eligibleFiles} rebuilt previews (${batch.eligibleBytes} bytes); kept ${batch.preservedFiles}, including ${batch.protectedSourceCopies} protected source copies.`;
      const limits = document.createElement("p");
      limits.textContent = `${batch.partial ? "Review is partial. " : ""}${batch.hasMore ? "More files remain. " : "This ordered review page has ended. "}${batch.remainingFiles === null ? "The remaining count is unknown." : `${batch.remainingFiles} files remain to review.`}`;
      const label =
        mode === "resetDefaults"
          ? `Reset ${batch.defaultRecordsToClear} default preview entries`
          : batch.eligibleFiles
            ? `Remove ${batch.eligibleFiles} ${batch.eligibleFiles === 1 ? "preview" : "previews"}`
            : "Confirm keeping these files";
      const execute = this.button(label, () => void this.execute(revision, plan.token, mode));
      this.body.append(title, description, limits, execute);
      this.status.textContent = `Review expires ${new Date(plan.expiresAt).toLocaleTimeString()}. Confirm this page explicitly or cancel.`;
    } catch {
      if (this.current(revision))
        this.status.textContent =
          "The preview review could not be completed. No cleanup was requested. Close and review again.";
    }
  }
  private async execute(
    revision: number,
    token: string,
    mode: ThumbnailCleanupMode,
  ): Promise<void> {
    if (!this.current(revision) || this.pending || this.executing || this.ownedToken !== token)
      return;
    this.executing = true;
    this.pending = true;
    this.ownedToken = null;
    this.cancel.disabled = true;
    this.body.querySelectorAll<HTMLButtonElement>("button").forEach((button) => {
      button.disabled = true;
    });
    this.status.textContent = "Applying this reviewed page…";
    try {
      const result = await this.adapter.execute(token);
      let refreshFailed = false;
      try {
        await this.callbacks.onChanged?.();
      } catch {
        refreshFailed = true;
      }
      const progress = result.thumbnailProgress;
      const summary =
        progress?.review.mode === "resetDefaults"
          ? `Reset ${progress.defaultRecordsCleared} default preview entries. All files were kept.`
          : `Removed ${result.files} rebuilt previews (${result.bytes} bytes).${progress ? ` Kept ${progress.review.preservedFiles} reviewed files, including ${progress.review.protectedSourceCopies} protected source copies.${progress.stopped ? " Cleanup stopped before completing this page." : ""}${progress.review.partial ? " The review was partial." : ""}` : " Detailed progress was not returned; review again before continuing."}`;
      const message =
        summary +
        (refreshFailed
          ? " The catalog view could not be refreshed; refresh it before continuing."
          : "");
      if (!this.current(revision)) {
        this.callbacks.onNotice?.(message);
        return;
      }
      this.body.replaceChildren();
      this.status.textContent = message;
      if (
        progress &&
        !progress.stopped &&
        progress.review.hasMore &&
        progress.review.continuation
      ) {
        const continuation = progress.review.continuation;
        this.body.append(
          this.button("Review remaining files", () => void this.review(mode, continuation)),
        );
      }
      if (progress?.review.mode === "resetDefaults")
        this.body.append(
          this.button("Review unused previews", () => void this.review("orphans", null)),
        );
    } catch {
      await this.discard(token);
      if (this.current(revision)) this.status.textContent = UNCONFIRMED_MANAGEMENT_OUTCOME;
      else this.callbacks.onNotice?.(UNCONFIRMED_MANAGEMENT_OUTCOME);
    } finally {
      this.executing = false;
      if (this.current(revision)) {
        this.pending = false;
        this.cancel.disabled = false;
        this.cancel.focus();
      }
    }
  }
  private button(text: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = text;
    button.addEventListener("click", action);
    return button;
  }
}
