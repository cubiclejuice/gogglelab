import {
  type LibraryClient,
  type BrowserPrinter,
  type BrowserPrinterNetworkPolicy,
  type BrowserPrinterDiscoveryPlan,
  type BrowserPrinterDiscoveryJob,
  type BrowserPrinterSuggestion,
  LibraryHttpError,
} from "./library-client";

export function printerNetworkAvailable(policy: BrowserPrinterNetworkPolicy | null): boolean {
  return policy?.enabled === true;
}
export function carbonBroadcastNeeded(printer: Pick<BrowserPrinter, "make" | "model">): boolean {
  const model = printer.model?.trim().toLowerCase() ?? "";
  return (
    (printer.make?.toLowerCase().includes("elegoo") ?? false) &&
    model.includes("centauri carbon") &&
    !model.includes("carbon 2")
  );
}
/** Browser-only review state; host permission and token admission remain authoritative. */
export class PrinterDiscoveryReview {
  revision = 0;
  plan: BrowserPrinterDiscoveryPlan | null = null;
  job: BrowserPrinterDiscoveryJob | null = null;
  edit(): number {
    this.revision += 1;
    this.plan = null;
    this.job = null;
    return this.revision;
  }
  acceptPlan(revision: number, plan: BrowserPrinterDiscoveryPlan): boolean {
    if (revision !== this.revision) return false;
    this.plan = plan;
    return true;
  }
  consumePlan(now = Date.now()): BrowserPrinterDiscoveryPlan {
    const plan = this.plan;
    this.plan = null;
    if (!plan) throw new Error("Review the discovery scope before starting.");
    if (!Number.isFinite(Date.parse(plan.expiresAt)) || Date.parse(plan.expiresAt) <= now)
      throw new Error("This discovery review expired. Review the scope again.");
    return plan;
  }
  acceptJob(revision: number, job: BrowserPrinterDiscoveryJob): boolean {
    if (revision !== this.revision || (this.job && this.job.jobId !== job.jobId)) return false;
    this.job = job;
    return true;
  }
}
function action(text: string, handler: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn";
  button.textContent = text;
  button.addEventListener("click", handler);
  return button;
}
function text(value: string): HTMLParagraphElement {
  const node = document.createElement("p");
  node.textContent = value;
  return node;
}
function explain(error: unknown): string {
  if (error instanceof LibraryHttpError && error.currentVersion)
    return "This printer changed. Refresh the fleet and review its current endpoint before trying again.";
  return error instanceof Error ? error.message : String(error);
}

/** Explicit user actions are the only entry points that can start printer LAN work. */
export class BrowserPrinterNetwork {
  readonly element = document.createElement("fieldset");
  private policy: BrowserPrinterNetworkPolicy | null = null;
  private state = new PrinterDiscoveryReview();
  private active = true;
  private pending = false;
  private statusPending = false;
  private pollTimer: number | null = null;
  private liveJobId: string | null = null;
  private pollFailures = 0;
  private readonly range = document.createElement("input");
  private readonly includeBambu = document.createElement("input");
  private readonly message = text("");
  private readonly reviewArea = document.createElement("div");
  private readonly suggestionsArea = document.createElement("div");
  private readonly policyNotice = text("Loading printer network availability…");
  private readonly reviewButton = action("Review discovery scope", () => {
    void this.review();
  });
  private readonly startButton = action("Start reviewed discovery", () => {
    void this.start();
  });
  private readonly cancelButton = action("Cancel discovery", () => {
    void this.cancel();
  });
  private readonly retryPollButton = action("Refresh discovery progress", () => {
    if (this.liveJobId && !this.pending) {
      this.pollFailures = 0;
      this.render();
      void this.poll(this.state.revision, this.liveJobId);
    }
  });
  private readonly selected = new Set<string>();
  constructor(
    private readonly client: LibraryClient,
    private readonly onSaved: (notice: string) => Promise<void>,
    private readonly onControlsChanged: () => void,
  ) {
    this.element.className = "browser-printer-network";
    const legend = document.createElement("legend");
    legend.textContent = "Printer network";
    this.message.setAttribute("role", "status");
    this.message.setAttribute("aria-live", "polite");
    const rangeLabel = document.createElement("label");
    rangeLabel.className = "printer-network-scope";
    rangeLabel.textContent = "Private IPv4 discovery scope";
    this.range.type = "text";
    this.range.maxLength = 512;
    this.range.placeholder = "10.7.8.1-10.7.8.2";
    rangeLabel.append(this.range);
    const bamLabel = document.createElement("label");
    bamLabel.className = "printer-network-checkbox";
    this.includeBambu.type = "checkbox";
    bamLabel.append(
      this.includeBambu,
      document.createTextNode("Include Bambu SSDP multicast discovery"),
    );
    const edit = () => {
      this.state.edit();
      this.selected.clear();
      this.reviewArea.replaceChildren();
      this.suggestionsArea.replaceChildren();
      this.message.textContent = "Scope changed. Review it before starting discovery.";
      this.render();
    };
    this.range.addEventListener("input", edit);
    this.includeBambu.addEventListener("change", edit);
    this.element.append(
      legend,
      this.policyNotice,
      rangeLabel,
      bamLabel,
      text(
        "Bambu SSDP multicast is optional and off initially. It sends on routed interfaces, so devices outside the selected scope may receive it. Only in-scope replies can produce printer candidates.",
      ),
      this.reviewButton,
      this.reviewArea,
      this.startButton,
      this.cancelButton,
      this.retryPollButton,
      this.message,
      this.suggestionsArea,
    );
    this.render();
  }
  async loadPolicy(available: boolean): Promise<void> {
    this.policy = null;
    this.render();
    this.onControlsChanged();
    if (!available) {
      this.policyNotice.textContent =
        "Printer networking is unavailable on this host or for this access key. Printer profiles and maintenance remain available.";
      this.render();
      return;
    }
    try {
      const policy = await this.client.getPrinterNetworkPolicy();
      if (!this.active) return;
      this.policy = policy;
      this.policyNotice.textContent = policy.enabled
        ? `Host printer networking is enabled. Review up to ${policy.maxDiscoveryAddresses.toLocaleString()} addresses; discovery returns at most ${policy.maxSuggestions.toLocaleString()} suggestions and runs for up to ${Math.ceil(policy.maximumDurationMs / 1000)} seconds. Unicast probes support Moonraker and PrusaLink. Bambu MQTT status remains deferred.`
        : "Printer networking is disabled by the host operator. The operator must configure allowed scopes and protocols on the host.";
    } catch (error) {
      if (this.active)
        this.policyNotice.textContent = `Printer networking unavailable: ${explain(error)}`;
    }
    if (this.active) {
      this.render();
      this.onControlsChanged();
    }
  }
  dismiss(): void {
    this.active = false;
    this.state.edit();
    if (this.pollTimer !== null) window.clearTimeout(this.pollTimer);
    this.pollTimer = null;
    const jobId = this.liveJobId;
    this.liveJobId = null;
    if (jobId) void this.client.cancelPrinterDiscovery(jobId).catch(() => {});
  }
  private render(): void {
    const enabled = printerNetworkAvailable(this.policy);
    const running = this.liveJobId !== null;
    this.range.disabled = !enabled || !this.policy?.discoveryAvailable || this.pending || running;
    this.includeBambu.disabled = this.range.disabled || !this.policy?.multicastDiscoveryAllowed;
    this.includeBambu.title = this.policy?.multicastDiscoveryAllowed
      ? "Multicast may reach devices outside the requested scope; only in-scope replies are accepted."
      : "Bambu multicast discovery is not permitted by this host policy.";
    this.reviewButton.disabled = this.range.disabled || !this.range.value.trim();
    this.startButton.hidden = !this.state.plan;
    this.startButton.disabled = !enabled || !this.state.plan || this.pending || running;
    this.cancelButton.hidden = !running;
    this.cancelButton.disabled = this.pending;
    this.retryPollButton.hidden = !running || this.pollFailures < 3;
    this.retryPollButton.disabled = this.pending;
    this.element.setAttribute("aria-busy", String(this.pending));
  }
  private async review(): Promise<void> {
    if (
      this.pending ||
      this.liveJobId ||
      !this.policy?.discoveryAvailable ||
      !printerNetworkAvailable(this.policy)
    )
      return;
    const revision = this.state.edit();
    this.pending = true;
    this.reviewArea.replaceChildren();
    this.suggestionsArea.replaceChildren();
    this.selected.clear();
    this.message.textContent = "Reviewing scope on the host; no printer requests are sent.";
    this.render();
    try {
      const plan = await this.client.planPrinterDiscovery(
        this.range.value.trim(),
        this.includeBambu.checked,
      );
      if (!this.active || !this.state.acceptPlan(revision, plan)) return;
      this.reviewArea.append(
        text(
          `Reviewed scope: ${plan.normalizedRange} · ${plan.addressCount.toLocaleString()} ${plan.addressCount === 1 ? "address" : "addresses"}. Unicast Moonraker/PrusaLink requests${plan.includeBambuDiscovery ? " and Bambu SSDP multicast" : "; no Bambu multicast"}.`,
        ),
        text(`Review expires ${plan.expiresAt}.`),
      );
      if (plan.multicastDisclosure) this.reviewArea.append(text(plan.multicastDisclosure));
      this.message.textContent =
        "Review the exact scope and reach above, then choose Start reviewed discovery.";
    } catch (error) {
      if (this.active && revision === this.state.revision)
        this.message.textContent = explain(error);
    } finally {
      if (this.active) {
        this.pending = false;
        this.render();
      }
    }
  }
  private async start(): Promise<void> {
    if (this.pending || this.liveJobId || !printerNetworkAvailable(this.policy)) return;
    let plan: BrowserPrinterDiscoveryPlan;
    try {
      plan = this.state.consumePlan();
    } catch (error) {
      this.message.textContent = explain(error);
      this.render();
      return;
    }
    const revision = this.state.revision;
    this.pending = true;
    this.render();
    this.message.textContent = "Starting the reviewed discovery…";
    try {
      const job = await this.client.startPrinterDiscovery(plan.planToken, true);
      if (!this.active || revision !== this.state.revision) {
        if (job.status === "running")
          void this.client.cancelPrinterDiscovery(job.jobId).catch(() => {});
        return;
      }
      this.acceptJob(revision, job);
    } catch (error) {
      if (this.active && revision === this.state.revision)
        this.message.textContent = `Discovery did not start: ${explain(error)} Review the scope again before retrying.`;
    } finally {
      if (this.active) {
        this.pending = false;
        this.render();
      }
    }
  }
  private acceptJob(revision: number, job: BrowserPrinterDiscoveryJob): void {
    if (!this.state.acceptJob(revision, job)) return;
    this.pollFailures = 0;
    if (job.status === "running") {
      this.liveJobId = job.jobId;
      this.message.textContent = `Searching ${job.normalizedRange}…`;
      this.pollTimer = window.setTimeout(() => {
        void this.poll(revision, job.jobId);
      }, 750);
    } else {
      this.liveJobId = null;
      this.message.textContent =
        job.status === "completed"
          ? `${job.suggestions.length} suggestion(s) ready to review. Nothing has been saved or approved.`
          : job.status === "cancelled"
            ? "Discovery cancelled. No suggestions were saved."
            : `Discovery failed${job.errorCode ? `: ${job.errorCode}` : ""}.`;
      if (job.status === "completed") this.renderSuggestions(job.suggestions);
    }
    this.render();
  }
  private async poll(revision: number, jobId: string): Promise<void> {
    this.pollTimer = null;
    if (!this.active || revision !== this.state.revision || this.liveJobId !== jobId) return;
    try {
      const job = await this.client.getPrinterDiscovery(jobId);
      if (this.active && revision === this.state.revision && this.liveJobId === jobId)
        this.acceptJob(revision, job);
    } catch (error) {
      if (!this.active || revision !== this.state.revision) return;
      this.pollFailures += 1;
      this.message.textContent = `Could not read discovery progress: ${explain(error)}. You can cancel this job.`;
      this.render();
      if (this.pollFailures < 3)
        this.pollTimer = window.setTimeout(() => {
          void this.poll(revision, jobId);
        }, 1500);
    }
  }
  private async cancel(): Promise<void> {
    const jobId = this.liveJobId;
    if (!jobId || this.pending) return;
    this.pending = true;
    if (this.pollTimer !== null) window.clearTimeout(this.pollTimer);
    this.pollTimer = null;
    const revision = this.state.edit();
    this.render();
    try {
      const job = await this.client.cancelPrinterDiscovery(jobId);
      if (!this.active || revision !== this.state.revision) return;
      this.liveJobId = null;
      this.acceptJob(revision, { ...job, status: "cancelled", suggestions: [] });
    } catch (error) {
      if (this.active)
        this.message.textContent = `Could not confirm cancellation: ${explain(error)}. Retry cancel; the host deadline still bounds this job.`;
    } finally {
      if (this.active) {
        this.pending = false;
        this.render();
      }
    }
  }
  private renderSuggestions(suggestions: readonly BrowserPrinterSuggestion[]): void {
    this.suggestionsArea.replaceChildren();
    this.selected.clear();
    if (!suggestions.length) return;
    for (const suggestion of suggestions.slice(0, this.policy?.maxSuggestions ?? 32)) {
      const label = document.createElement("label");
      label.className = "printer-network-checkbox";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.addEventListener("change", () => {
        if (box.checked) this.selected.add(suggestion.key);
        else this.selected.delete(suggestion.key);
        save.disabled = !this.selected.size;
      });
      label.append(
        box,
        document.createTextNode(
          `${suggestion.name} · ${suggestion.connection.address} · ${suggestion.source}`,
        ),
      );
      this.suggestionsArea.append(label);
    }
    const save = action("Save selected as unapproved profiles", () => {
      void this.saveSuggestions(suggestions, save);
    });
    save.disabled = true;
    this.suggestionsArea.append(
      text(
        "Saving creates profiles only. Review each saved endpoint separately before a status check.",
      ),
      save,
    );
  }
  private async saveSuggestions(
    suggestions: readonly BrowserPrinterSuggestion[],
    save: HTMLButtonElement,
  ): Promise<void> {
    if (this.pending) return;
    const selected = suggestions.filter((item) => this.selected.has(item.key));
    if (!selected.length) return;
    this.pending = true;
    save.disabled = true;
    this.render();
    const failures: string[] = [];
    let saved = 0;
    for (const suggestion of selected) {
      if (!this.active) break;
      try {
        await this.client.createPrinter({
          nickname: suggestion.name,
          make:
            suggestion.source === "bambu_mqtt_tls"
              ? "Bambu Lab"
              : suggestion.source === "prusalink"
                ? "Prusa"
                : null,
          model: suggestion.model,
          printerType: suggestion.printerType,
          firmware: null,
          host: suggestion.connection.address,
          webUrl: null,
          bedMm: suggestion.bedMm,
          notes: "",
        });
        saved += 1;
        this.selected.delete(suggestion.key);
      } catch (error) {
        failures.push(`${suggestion.name}: ${explain(error)}`);
      }
    }
    if (this.active) {
      this.pending = false;
      this.suggestionsArea.replaceChildren();
      const remaining = selected.filter((item) => this.selected.has(item.key));
      this.renderSuggestions(remaining);
      this.message.textContent = `Saved ${saved} unapproved profile(s).${failures.length ? ` Could not save ${failures.join("; ")}` : ""}`;
      this.render();
      await this.onSaved(`Saved ${saved} unapproved printer profile(s).`);
    }
  }
  printerActions(printer: BrowserPrinter): HTMLElement {
    const group = document.createElement("div");
    group.className = "browser-printer-network-actions";
    if (!printerNetworkAvailable(this.policy) || !this.policy?.statusChecksAvailable) return group;
    if (!printer.host && !printer.webUrl) return group;
    const approval = action(
      printer.endpointApproved ? "Revoke endpoint approval" : "Review and approve endpoint",
      () => {
        void this.approve(printer);
      },
    );
    approval.disabled = this.statusPending;
    group.append(approval);
    const carbon = carbonBroadcastNeeded(printer);
    let consent: HTMLInputElement | null = null;
    if (carbon) {
      const label = document.createElement("label");
      label.className = "printer-network-checkbox browser-printer-carbon-consent";
      consent = document.createElement("input");
      consent.type = "checkbox";
      consent.disabled =
        !this.policy.carbonBroadcastAllowed || !printer.endpointApproved || this.statusPending;
      label.append(
        consent,
        document.createTextNode(
          "Find Carbon MainboardID with one UDP broadcast to 255.255.255.255:3000 on the routed interface. Other LAN devices may receive it; only this printer's matching reply is accepted. The ID is not retained.",
        ),
      );
      group.append(label);
      if (!this.policy.carbonBroadcastAllowed)
        group.append(text("Carbon discovery broadcast is not permitted by the host operator."));
    }
    const test = action("Check saved printer status", () => {
      void this.check(printer, carbon && consent?.checked === true);
    });
    test.disabled =
      this.statusPending || !printer.endpointApproved || (carbon && !consent?.checked);
    consent?.addEventListener("change", () => {
      test.disabled = this.statusPending || !printer.endpointApproved || !consent?.checked;
    });
    group.append(test);
    return group;
  }
  private async approve(printer: BrowserPrinter): Promise<void> {
    if (this.statusPending || !this.policy?.statusChecksAvailable) return;
    const approved = !printer.endpointApproved;
    if (
      !window.confirm(
        `${approved ? "Approve" : "Revoke approval for"} the saved endpoint for ${printer.nickname}: ${printer.host ?? printer.webUrl}?${approved ? " This permits explicit read-only status checks within the host policy. No request is sent by approving." : ""}`,
      )
    )
      return;
    this.statusPending = true;
    this.onControlsChanged();
    try {
      await this.client.setPrinterEndpointApproval(printer.id, printer.metadataVersion, approved);
      if (this.active)
        await this.onSaved(
          approved
            ? "Saved endpoint approved. Choose a status check to contact it."
            : "Endpoint approval revoked.",
        );
    } catch (error) {
      if (this.active) this.message.textContent = explain(error);
    } finally {
      this.statusPending = false;
      if (this.active) this.onControlsChanged();
    }
  }
  private async check(printer: BrowserPrinter, discoverMainboardId: boolean): Promise<void> {
    if (
      this.statusPending ||
      !printer.endpointApproved ||
      !this.policy?.statusChecksAvailable ||
      (discoverMainboardId && !this.policy.carbonBroadcastAllowed)
    )
      return;
    if (carbonBroadcastNeeded(printer) && !discoverMainboardId) return;
    const disclosure = discoverMainboardId
      ? " Send one UDP broadcast to 255.255.255.255:3000 on the routed interface, then one read-only SDCP Cmd 0 request after a matching reply. Other LAN devices may receive the broadcast."
      : " Send bounded read-only requests to the saved endpoint.";
    if (
      !window.confirm(
        `Check ${printer.nickname} at ${printer.host ?? printer.webUrl}?${disclosure}`,
      )
    )
      return;
    this.statusPending = true;
    this.onControlsChanged();
    this.message.textContent = `Checking ${printer.nickname}…`;
    try {
      const response = await this.client.testPrinterConnection(
        printer.id,
        printer.metadataVersion,
        discoverMainboardId,
        true,
      );
      if (!this.active) return;
      const notice = `${printer.nickname}: ${response.result}; job ${response.printer.jobStatus}.${response.statusEventObserved ? " Status event observed." : " No status event observed."}`;
      this.message.textContent = notice;
      await this.onSaved(notice);
    } catch (error) {
      if (this.active) this.message.textContent = explain(error);
    } finally {
      this.statusPending = false;
      if (this.active) this.onControlsChanged();
    }
  }
}
