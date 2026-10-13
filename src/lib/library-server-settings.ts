import type { DesktopLibraryServerStatus } from "./ipc";

export interface LibraryServerSettingsCallbacks {
  status(): Promise<DesktopLibraryServerStatus>;
  start(allowExternalIntegrations: boolean): Promise<DesktopLibraryServerStatus>;
  stop(): Promise<DesktopLibraryServerStatus>;
  revealAccessKey(): Promise<{ accessKey: string }>;
}

export function libraryServerStatusCopy(status: DesktopLibraryServerStatus): {
  state: string;
  address: string;
  action: string;
  externalIntegrations: string;
} {
  return status.running
    ? {
        state: "Running on this computer",
        address: status.baseUrl ?? "Local address unavailable",
        action: "Stop server",
        externalIntegrations: status.allowExternalIntegrations ? "On for this session" : "Off",
      }
    : {
        state: "Stopped",
        address: "Not accepting connections",
        action: "Start server",
        externalIntegrations: "Off",
      };
}

export function libraryServerExternalIntegrationChoice(
  status: DesktopLibraryServerStatus,
  currentChoice: boolean,
  hasPendingChoice: boolean,
): boolean {
  if (status.running) return status.allowExternalIntegrations;
  return hasPendingChoice ? currentChoice : false;
}

export function shouldRevealAccessKey(
  request: number,
  currentRequest: number,
  running: boolean,
  connected: boolean,
): boolean {
  return request === currentRequest && running && connected;
}

export class LibraryServerSettingsSection {
  readonly element: HTMLElement;
  private readonly state: HTMLElement;
  private readonly address: HTMLElement;
  private readonly action: HTMLButtonElement;
  private readonly error: HTMLElement;
  private readonly reveal: HTMLButtonElement;
  private readonly key: HTMLInputElement;
  private readonly copy: HTMLButtonElement;
  private status: DesktopLibraryServerStatus = {
    running: false,
    baseUrl: null,
    mcpUrl: null,
    allowExternalIntegrations: false,
  };
  private readonly allowExternalInput: HTMLInputElement;
  private readonly integrationStatus: HTMLElement;
  private allowExternalIntegrations = false;
  private hasPendingExternalIntegrationChoice = false;
  private request = 0;

  constructor(private readonly callbacks: LibraryServerSettingsCallbacks) {
    this.element = document.createElement("fieldset");
    this.element.className = "settings-section library-server-settings";
    const legend = document.createElement("legend");
    legend.textContent = "Browser and MCP server";
    this.state = document.createElement("strong");
    this.address = document.createElement("span");
    this.address.className = "value";
    this.action = document.createElement("button");
    this.action.type = "button";
    this.action.className = "btn";
    this.action.addEventListener("click", () => void this.toggle());
    this.allowExternalInput = document.createElement("input");
    this.allowExternalInput.type = "checkbox";
    this.allowExternalInput.checked = false;
    this.allowExternalInput.setAttribute("aria-describedby", "library-server-network-hint");
    this.allowExternalInput.addEventListener(
      "change",
      () => void this.updateExternalIntegrations(),
    );
    const networkLabel = document.createElement("label");
    networkLabel.className = "library-server-network";
    networkLabel.append(
      this.allowExternalInput,
      document.createTextNode("Allow external integration requests from Browser and MCP"),
    );
    this.integrationStatus = document.createElement("span");
    this.integrationStatus.className = "value library-server-integration-status";
    this.integrationStatus.setAttribute("aria-live", "polite");
    const networkHint = document.createElement("div");
    networkHint.className = "hint";
    networkHint.id = "library-server-network-hint";
    networkHint.textContent =
      "Off by default. When enabled, authenticated Browser/MCP clients can contact configured AI providers for Generate suggestions or Spoolman for an explicit sync. AI category sharing is a separate setting. Turning this off blocks new requests; requests already in progress may finish. Stopping the server turns this off.";
    this.reveal = document.createElement("button");
    this.reveal.type = "button";
    this.reveal.className = "btn";
    this.reveal.textContent = "Show access key";
    this.reveal.addEventListener("click", () => void this.showAccessKey());
    this.key = document.createElement("input");
    this.key.readOnly = true;
    this.key.hidden = true;
    this.key.setAttribute("aria-label", "Server access key");
    this.copy = document.createElement("button");
    this.copy.type = "button";
    this.copy.className = "btn";
    this.copy.textContent = "Copy key";
    this.copy.hidden = true;
    this.copy.addEventListener("click", () => void this.copyAccessKey());
    const hint = document.createElement("div");
    hint.className = "hint";
    hint.textContent =
      "Off by default. When started, access is limited to this computer and uses an access key.";
    this.error = document.createElement("div");
    this.error.className = "settings-error";
    this.error.setAttribute("role", "status");
    this.element.append(
      legend,
      this.state,
      this.address,
      this.integrationStatus,
      networkLabel,
      networkHint,
      this.action,
      this.reveal,
      this.key,
      this.copy,
      hint,
      this.error,
    );
    this.render();
    void this.refresh();
  }

  async refresh(): Promise<void> {
    const request = ++this.request;
    try {
      const status = await this.callbacks.status();
      if (request !== this.request) return;
      this.status = status;
      this.allowExternalIntegrations = libraryServerExternalIntegrationChoice(
        status,
        this.allowExternalIntegrations,
        this.hasPendingExternalIntegrationChoice,
      );
      if (status.running) this.hasPendingExternalIntegrationChoice = false;
      if (!status.running) this.clearAccessKey();
      this.error.textContent = "";
      this.render();
    } catch (error) {
      if (request === this.request) this.showError(error);
    }
  }

  private async toggle(): Promise<void> {
    const request = ++this.request;
    this.action.disabled = true;
    this.error.textContent = "";
    try {
      const status = this.status.running
        ? await this.callbacks.stop()
        : await this.callbacks.start(this.allowExternalIntegrations);
      if (request !== this.request) return;
      this.status = status;
      this.allowExternalIntegrations = status.running && status.allowExternalIntegrations;
      this.hasPendingExternalIntegrationChoice = false;
      this.render();
    } catch (error) {
      if (request === this.request) this.showError(error);
    } finally {
      if (request === this.request) this.action.disabled = false;
    }
  }

  private render(): void {
    const copy = libraryServerStatusCopy(this.status);
    this.state.textContent = copy.state;
    this.address.textContent = copy.address;
    this.address.title = this.status.mcpUrl ? `MCP: ${this.status.mcpUrl}` : "";
    this.integrationStatus.textContent = this.status.running
      ? `External integration access: ${copy.externalIntegrations}`
      : `External integration access: ${this.allowExternalIntegrations ? "Selected for the next session" : "Off by default"}`;
    this.allowExternalInput.checked = this.allowExternalIntegrations;
    this.action.textContent = copy.action;
    this.reveal.hidden = !this.status.running;
    if (!this.status.running) this.clearAccessKey();
  }

  private async updateExternalIntegrations(): Promise<void> {
    const allowExternalIntegrations = this.allowExternalInput.checked;
    const request = ++this.request;
    this.error.textContent = "";
    if (!this.status.running) {
      this.allowExternalIntegrations = allowExternalIntegrations;
      this.hasPendingExternalIntegrationChoice = true;
      this.render();
      return;
    }
    this.allowExternalInput.disabled = true;
    try {
      const status = await this.callbacks.start(allowExternalIntegrations);
      if (request !== this.request) return;
      this.status = status;
      this.allowExternalIntegrations = status.running && status.allowExternalIntegrations;
      this.hasPendingExternalIntegrationChoice = false;
      this.render();
    } catch (error) {
      if (request === this.request) {
        this.allowExternalIntegrations = this.status.allowExternalIntegrations;
        this.render();
        this.showError(error);
      }
    } finally {
      if (request === this.request) this.allowExternalInput.disabled = false;
    }
  }

  private async showAccessKey(): Promise<void> {
    const request = this.request;
    this.reveal.disabled = true;
    this.error.textContent = "";
    try {
      const result = await this.callbacks.revealAccessKey();
      if (
        !shouldRevealAccessKey(request, this.request, this.status.running, this.element.isConnected)
      )
        return;
      this.key.value = result.accessKey;
      this.key.hidden = false;
      this.copy.hidden = false;
      this.key.focus();
      this.key.select();
    } catch (error) {
      this.showError(error);
    } finally {
      this.reveal.disabled = false;
    }
  }

  private async copyAccessKey(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.key.value);
      this.error.textContent = "Access key copied.";
    } catch {
      this.error.textContent = "Could not copy. Select the access key and copy it manually.";
      this.key.focus();
      this.key.select();
    }
  }

  private clearAccessKey(): void {
    this.key.value = "";
    this.key.hidden = true;
    this.copy.hidden = true;
  }

  clearSensitive(): void {
    this.request += 1;
    this.clearAccessKey();
  }

  private showError(error: unknown): void {
    this.error.textContent = String((error as { message?: unknown }).message ?? error);
    this.render();
  }
}
