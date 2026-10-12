import type {
  AiProviderPublicConfig,
  AiSuggestionBatch,
  IntegrationSettings,
  LibraryModel,
  SpoolmanConfig,
  VersionedModelTarget,
} from "./ipc";
import {
  AI_DISCLOSURE,
  aiAfterTags,
  aiDisclosure,
  aiTargets,
  buildAiReviewDecisions,
  normalizeCategorySharing,
} from "./library-filaments";

export class LibraryAiDialog {
  private readonly dialog: HTMLDialogElement;
  private review: AiSuggestionBatch | null = null;
  private invoker: HTMLElement | null = null;
  private requestRevision = 0;
  private selectedProviderId: string | null = null;
  private networkAvailability = { ai: true, spoolman: true };
  onSaveSpoolman: (config: SpoolmanConfig | null) => Promise<void> = async () => {};
  onPullSpoolman: () => Promise<string> = async () => "";
  onSaveProvider: (
    config: AiProviderPublicConfig,
    credential: { action: "keep" | "clear" | "setSession"; secret?: string },
  ) => Promise<void> = async () => {};
  onDeleteProvider: (providerId: string) => Promise<void> = async () => {};
  onSuggest: (providerId: string, targets: VersionedModelTarget[]) => Promise<AiSuggestionBatch> =
    async () => {
      throw new Error("AI provider is unavailable");
    };
  onApply: (
    token: string,
    decisions: Array<{
      modelId: string;
      action: "accept" | "reject";
      acceptedSuggestionIds: string[];
    }>,
    strategy: "append" | "replace" | "merge",
  ) => Promise<void> = async () => {};
  onDiscard: (token: string) => Promise<void> = async () => {};

  constructor(container: HTMLElement) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "library-ai-dialog";
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      void this.close();
    });
    container.append(this.dialog);
  }

  show(
    settings: IntegrationSettings,
    models: LibraryModel[],
    invoker?: HTMLElement,
    networkAvailability = { ai: true, spoolman: true },
  ): void {
    this.requestRevision += 1;
    const previousToken = this.review?.reviewToken;
    this.review = null;
    if (previousToken) void this.onDiscard(previousToken).catch(() => {});
    if (
      this.selectedProviderId &&
      !settings.aiProviders.some((provider) => provider.id === this.selectedProviderId)
    ) {
      this.selectedProviderId = null;
    }
    this.selectedProviderId ??= settings.aiProviders[0]?.id ?? null;
    this.networkAvailability = networkAvailability;
    this.invoker =
      invoker ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    this.render(settings, models);
    if (!this.dialog.open) this.dialog.showModal();
    this.dialog.querySelector<HTMLButtonElement>("button")?.focus();
  }

  private render(settings: IntegrationSettings, models: LibraryModel[]): void {
    this.dialog.replaceChildren();
    const header = document.createElement("header");
    const title = document.createElement("h2");
    title.textContent = "Integrations and reviewed AI tags";
    header.append(
      title,
      this.button("Close", () => void this.close()),
    );
    const error = document.createElement("div");
    error.className = "library-editor-error";
    error.setAttribute("role", "alert");
    const spool = this.spoolmanSection(settings.spoolman, error, this.networkAvailability.spoolman);
    const ai = this.aiSection(settings, models, error, this.networkAvailability.ai);
    this.dialog.append(header, error, spool, ai);
  }

  private spoolmanSection(
    config: SpoolmanConfig | null,
    error: HTMLElement,
    networkEnabled: boolean,
  ): HTMLElement {
    const section = document.createElement("section");
    const title = document.createElement("h3");
    title.textContent = "Spoolman explicit pull";
    const url = this.input("Spoolman base URL", config?.baseUrl ?? "");
    const scope = this.select("Endpoint scope", [
      ["local", "Local/private HTTP"],
      ["remote", "Remote HTTPS"],
    ]);
    scope.value = config?.endpointScope ?? "local";
    const page = this.input("Page size", String(config?.pageSize ?? 100), "number");
    const timeout = this.input("Timeout seconds", String(config?.timeoutSeconds ?? 15), "number");
    const save = this.button(
      "Save configuration",
      () =>
        void this.runBusy(save, error, () =>
          this.onSaveSpoolman({
            baseUrl: url.value,
            endpointScope: scope.value as "local" | "remote",
            pageSize: Number(page.value),
            timeoutSeconds: Number(timeout.value),
          }),
        ),
    );
    const pull = this.button(
      "Pull now",
      () =>
        void this.runBusy(pull, error, async () => {
          error.textContent = await this.onPullSpoolman();
        }),
    );
    pull.disabled = !networkEnabled;
    if (!networkEnabled) {
      const notice = document.createElement("p");
      notice.setAttribute("role", "status");
      notice.textContent = "Spoolman network pulls are disabled on this host.";
      section.append(title, notice, url, scope, page, timeout, save, pull);
      return section;
    }
    section.append(title, url, scope, page, timeout, save, pull);
    return section;
  }

  private aiSection(
    settings: IntegrationSettings,
    models: LibraryModel[],
    error: HTMLElement,
    networkEnabled: boolean,
  ): HTMLElement {
    const section = document.createElement("section");
    const title = document.createElement("h3");
    title.textContent = `AI tag review · ${models.length} explicitly selected`;
    const disclosure = document.createElement("p");
    disclosure.textContent = AI_DISCLOSURE;
    const providerSelect = this.select("Saved provider", [
      ["__new__", "New provider"],
      ...settings.aiProviders.map((provider): [string, string] => [
        provider.id,
        provider.label || provider.id,
      ]),
    ]);
    providerSelect.value = this.selectedProviderId ?? "__new__";
    providerSelect.addEventListener("change", () => {
      this.selectedProviderId = providerSelect.value === "__new__" ? null : providerSelect.value;
      this.render(settings, models);
    });
    const existing = settings.aiProviders.find(
      (provider) => provider.id === this.selectedProviderId,
    );
    const id = this.input("Provider ID", existing?.id ?? "");
    const label = this.input("Provider label", existing?.label ?? "");
    const kind = this.select("Provider kind", [
      ["openaiCompatible", "OpenAI compatible"],
      ["anthropic", "Anthropic"],
      ["gemini", "Gemini"],
    ]);
    kind.value = existing?.kind ?? "openaiCompatible";
    const baseUrl = this.input("Base URL", existing?.baseUrl ?? "");
    const endpointScope = this.select("Endpoint scope", [
      ["local", "Local/private endpoint"],
      ["remote", "Remote HTTPS endpoint"],
    ]);
    endpointScope.value = existing?.endpointScope ?? "local";
    const model = this.input("Model", existing?.model ?? "");
    const secret = this.input("Session-only API key", "", "password");
    const maxTags = this.input("Maximum tags", String(existing?.maxTags ?? 6), "number");
    const detail = this.select("Detail", [
      ["brief", "Brief"],
      ["balanced", "Balanced"],
      ["detailed", "Detailed"],
    ]);
    detail.value = existing?.detail ?? "balanced";
    const categoryMode = this.select("Categories", [
      ["none", "No categories"],
      ["existingOnly", "Existing only"],
      ["allowNew", "Allow new"],
    ]);
    categoryMode.value = existing?.categoryMode ?? "existingOnly";
    const categorySharing = this.select("Category sharing", [
      ["local", "Keep categories local"],
      ["provider", "Share categories with configured provider"],
    ]);
    categorySharing.value = normalizeCategorySharing(existing?.categorySharing);
    disclosure.textContent = aiDisclosure(categorySharing.value as "local" | "provider");
    categorySharing.addEventListener("change", () => {
      disclosure.textContent = aiDisclosure(categorySharing.value as "local" | "provider");
    });
    const concurrency = this.input("Concurrency", String(existing?.concurrency ?? 1), "number");
    const timeout = this.input("Timeout seconds", String(existing?.timeoutSeconds ?? 30), "number");
    const structuredOutput = this.select("Structured output", [
      ["jsonSchema", "JSON schema"],
      ["jsonObject", "JSON object"],
      ["plainJson", "Plain JSON"],
    ]);
    structuredOutput.value = existing?.structuredOutput ?? "plainJson";
    const strategy = this.select("Apply strategy", [
      ["append", "Append: union accepted tags"],
      ["merge", "Merge: replace represented categories"],
      ["replace", "Replace: exact accepted tags"],
    ]);
    const save = this.button(
      "Save provider",
      () =>
        void this.runBusy(save, error, async () => {
          await this.onSaveProvider(
            {
              id: id.value,
              label: label.value,
              kind: kind.value as AiProviderPublicConfig["kind"],
              baseUrl: baseUrl.value || null,
              endpointScope: endpointScope.value as "local" | "remote",
              model: model.value,
              credentialStorage:
                secret.value || existing?.credentialStorage === "session" ? "session" : "none",
              hasCredential: existing?.hasCredential ?? false,
              maxTags: Number(maxTags.value),
              detail: detail.value as AiProviderPublicConfig["detail"],
              categoryMode: categoryMode.value as AiProviderPublicConfig["categoryMode"],
              categorySharing: categorySharing.value as AiProviderPublicConfig["categorySharing"],
              concurrency: Number(concurrency.value),
              timeoutSeconds: Number(timeout.value),
              structuredOutput:
                structuredOutput.value as AiProviderPublicConfig["structuredOutput"],
            },
            secret.value ? { action: "setSession", secret: secret.value } : { action: "keep" },
          );
          secret.value = "";
        }),
    );
    const clearCredential = this.button(
      "Clear credential",
      () =>
        void this.runBusy(clearCredential, error, () =>
          this.onSaveProvider(
            {
              ...(existing ?? {
                id: id.value,
                label: label.value,
                kind: kind.value as AiProviderPublicConfig["kind"],
                baseUrl: baseUrl.value || null,
                endpointScope: endpointScope.value as "local" | "remote",
                model: model.value,
                credentialStorage: "none",
                hasCredential: false,
                maxTags: Number(maxTags.value),
                detail: detail.value as AiProviderPublicConfig["detail"],
                categoryMode: categoryMode.value as AiProviderPublicConfig["categoryMode"],
                categorySharing: categorySharing.value as AiProviderPublicConfig["categorySharing"],
                concurrency: Number(concurrency.value),
                timeoutSeconds: Number(timeout.value),
                structuredOutput:
                  structuredOutput.value as AiProviderPublicConfig["structuredOutput"],
              }),
              credentialStorage: "none",
              hasCredential: false,
            },
            { action: "clear" },
          ),
        ),
    );
    const deleteProvider = this.button(
      "Delete provider",
      () =>
        void this.runBusy(deleteProvider, error, async () => {
          if (!id.value || !window.confirm(`Delete provider “${label.value || id.value}”?`)) return;
          await this.onDeleteProvider(id.value);
          this.selectedProviderId = null;
          this.render(
            {
              ...settings,
              aiProviders: settings.aiProviders.filter((provider) => provider.id !== id.value),
            },
            models,
          );
        }),
    );
    const generate = this.button(
      "Generate suggestions for selected models",
      () => void this.generateReview(id.value, models, section, strategy, error, generate),
    );
    generate.disabled = !networkEnabled;
    if (!networkEnabled) {
      const notice = document.createElement("p");
      notice.setAttribute("role", "status");
      notice.textContent = "AI provider requests are disabled on this host.";
      section.append(notice);
    }
    section.append(
      title,
      disclosure,
      providerSelect,
      id,
      label,
      kind,
      baseUrl,
      endpointScope,
      model,
      secret,
      maxTags,
      detail,
      categoryMode,
      categorySharing,
      concurrency,
      timeout,
      structuredOutput,
      strategy,
      save,
      clearCredential,
      deleteProvider,
      generate,
    );
    return section;
  }

  private async generateReview(
    providerId: string,
    models: LibraryModel[],
    section: HTMLElement,
    strategy: HTMLSelectElement,
    error: HTMLElement,
    button: HTMLButtonElement,
  ): Promise<void> {
    if (!models.length) {
      error.textContent = "Select one or more models in the catalog first.";
      return;
    }
    const revision = ++this.requestRevision;
    button.disabled = true;
    error.textContent = "";
    try {
      const batch = await this.onSuggest(providerId, aiTargets(models));
      if (revision !== this.requestRevision || !this.dialog.open) {
        await this.onDiscard(batch.reviewToken).catch(() => {});
        return;
      }
      if (this.review) await this.onDiscard(this.review.reviewToken).catch(() => {});
      this.review = batch;
      this.renderReview(section, strategy, error);
    } catch (reason) {
      if (revision === this.requestRevision) {
        error.textContent = String((reason as { message?: unknown })?.message ?? reason);
      }
    } finally {
      if (revision === this.requestRevision) button.disabled = false;
    }
  }

  private renderReview(
    section: HTMLElement,
    strategy: HTMLSelectElement,
    error: HTMLElement,
  ): void {
    section.querySelector(".library-ai-review")?.remove();
    const batch = this.review;
    if (!batch) return;
    const review = document.createElement("div");
    review.className = "library-ai-review";
    const decisions = new Map<string, HTMLInputElement[]>();
    const actions = new Map<string, { accept: HTMLInputElement; reject: HTMLInputElement }>();
    for (const target of batch.targets) {
      const group = document.createElement("fieldset");
      const legend = document.createElement("legend");
      legend.textContent = target.displayName;
      const before = document.createElement("p");
      const existing = target.existingTags;
      const formatTags = (tags: Array<{ name: string; category: string | null }>) =>
        tags.map((tag) => (tag.category ? `${tag.category}: ${tag.name}` : tag.name)).join(", ");
      before.textContent = `Before: ${formatTags(existing) || "No tags"}`;
      const after = document.createElement("p");
      after.textContent = "After review: unchanged until suggestions are accepted";
      group.append(legend, before, after);
      const checks: HTMLInputElement[] = [];
      const updateAfter = () => {
        if (actions.get(target.modelId)?.reject.checked) {
          after.textContent = `After: ${formatTags(existing) || "No tags"} (unchanged)`;
          return;
        }
        const acceptedIds = new Set(
          checks.filter((check) => check.checked).map((check) => check.value),
        );
        const result = aiAfterTags(
          existing,
          target.suggestions,
          acceptedIds,
          strategy.value as "append" | "replace" | "merge",
        );
        after.textContent = `After: ${formatTags(result) || "No tags"}`;
      };
      for (const suggestion of target.suggestions) {
        const label = document.createElement("label");
        const check = document.createElement("input");
        check.type = "checkbox";
        check.value = suggestion.suggestionId;
        checks.push(check);
        check.addEventListener("change", () => {
          if (check.checked) actions.get(target.modelId)!.accept.checked = true;
          updateAfter();
        });
        label.append(
          check,
          document.createTextNode(
            `${suggestion.category ? `${suggestion.category}: ` : ""}${suggestion.name}`,
          ),
        );
        group.append(label);
      }
      decisions.set(target.modelId, checks);
      const accept = document.createElement("input");
      accept.type = "radio";
      accept.name = `ai-decision-${target.modelId}`;
      accept.value = "accept";
      accept.setAttribute("aria-label", `Accept reviewed suggestions for ${target.displayName}`);
      const acceptLabel = document.createElement("label");
      acceptLabel.append(accept, document.createTextNode(" Accept reviewed selection"));
      const reject = document.createElement("input");
      reject.type = "radio";
      reject.name = `ai-decision-${target.modelId}`;
      reject.value = "reject";
      reject.setAttribute("aria-label", `Reject all suggestions for ${target.displayName}`);
      const rejectLabel = document.createElement("label");
      rejectLabel.append(
        reject,
        document.createTextNode(" Reject this model; keep its tags unchanged"),
      );
      accept.addEventListener("change", () => {
        for (const check of checks) check.disabled = false;
        updateAfter();
      });
      reject.addEventListener("change", () => {
        for (const check of checks) check.disabled = reject.checked;
        updateAfter();
      });
      strategy.addEventListener("change", updateAfter);
      actions.set(target.modelId, { accept, reject });
      group.append(acceptLabel, rejectLabel);
      review.append(group);
    }
    for (const failure of batch.failures) {
      const row = document.createElement("p");
      row.textContent = `${failure.modelId}: ${failure.message}`;
      review.append(row);
    }
    const apply = this.button(
      "Apply accepted suggestions",
      () =>
        void this.runBusy(apply, error, async () => {
          if (
            strategy.value === "replace" &&
            !window.confirm(
              "Replace makes accepted tags the exact set for each accepted model. Explicitly rejected models remain unchanged. Continue?",
            )
          ) {
            return;
          }
          const accepted = new Map(
            [...decisions].map(([modelId, checks]) => [
              modelId,
              checks.filter((check) => check.checked).map((check) => check.value),
            ]),
          );
          const actionValues = new Map<string, "accept" | "reject">();
          for (const [modelId, inputs] of actions) {
            if (inputs.accept.checked) actionValues.set(modelId, "accept");
            if (inputs.reject.checked) actionValues.set(modelId, "reject");
          }
          await this.onApply(
            batch.reviewToken,
            buildAiReviewDecisions(batch, accepted, actionValues),
            strategy.value as "append" | "replace" | "merge",
          );
          this.review = null;
          review.remove();
          error.textContent = "Reviewed tag decisions applied.";
        }),
    );
    review.append(apply);
    section.append(review);
  }

  private input(label: string, value: string, type = "text"): HTMLInputElement {
    const input = document.createElement("input");
    input.type = type;
    input.value = value;
    input.placeholder = label;
    input.setAttribute("aria-label", label);
    return input;
  }

  private select(label: string, options: Array<[string, string]>): HTMLSelectElement {
    const select = document.createElement("select");
    select.setAttribute("aria-label", label);
    for (const [value, text] of options) select.add(new Option(text, value));
    return select;
  }

  private button(label: string, action: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn";
    button.textContent = label;
    button.addEventListener("click", action);
    return button;
  }

  private async run(error: HTMLElement, action: () => Promise<void>): Promise<void> {
    error.textContent = "";
    try {
      await action();
    } catch (reason) {
      error.textContent = String((reason as { message?: unknown })?.message ?? reason);
    }
  }

  private async runBusy(
    button: HTMLButtonElement,
    error: HTMLElement,
    action: () => Promise<void>,
  ): Promise<void> {
    button.disabled = true;
    try {
      await this.run(error, action);
    } finally {
      button.disabled = false;
    }
  }

  private async close(): Promise<void> {
    this.requestRevision += 1;
    const token = this.review?.reviewToken;
    this.review = null;
    if (token) await this.onDiscard(token).catch(() => {});
    if (this.dialog.open) this.dialog.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  async dismiss(): Promise<void> {
    await this.close();
  }
}
