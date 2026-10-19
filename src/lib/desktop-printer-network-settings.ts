import type { DesktopLibraryServerStatus, DesktopPrinterNetworkPolicy } from "./ipc";

/** Only shape/bounds are validated here; native code owns private IPv4 admission. */
export function printerNetworkDraftPolicy(
  enabled: boolean,
  scopeText: string,
  allowMulticastDiscovery: boolean,
  allowCarbonBroadcast: boolean,
): DesktopPrinterNetworkPolicy | null {
  if (!enabled) return null;
  if (scopeText.length > 4096)
    throw new Error("Printer scopes must contain at most 4,096 characters.");
  const allowedScopes = scopeText
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean);
  if (!allowedScopes.length) throw new Error("Enter at least one allowed private IPv4 scope.");
  if (allowedScopes.length > 32) throw new Error("Enter at most 32 scope entries, one per line.");
  return { allowedScopes, allowMulticastDiscovery, allowCarbonBroadcast };
}

export class DesktopPrinterNetworkSettings {
  readonly element = document.createElement("fieldset");
  private readonly enabled = document.createElement("input");
  private readonly scopes = document.createElement("textarea");
  private readonly multicast = document.createElement("input");
  private readonly carbon = document.createElement("input");
  private readonly status = document.createElement("p");
  private readonly error = document.createElement("div");
  private readonly apply = document.createElement("button");
  private dirty = false;
  private running = false;
  private busy = false;
  constructor(
    private readonly onApply: (policy: DesktopPrinterNetworkPolicy | null) => Promise<void>,
  ) {
    this.element.className = "settings-section desktop-printer-network-settings";
    const legend = document.createElement("legend");
    legend.textContent = "Printer LAN access from Browser and MCP";
    this.enabled.type = "checkbox";
    this.enabled.checked = false;
    this.enabled.setAttribute("aria-describedby", "desktop-printer-network-hint");
    const enableLabel = document.createElement("label");
    enableLabel.className = "printer-network-checkbox";
    enableLabel.append(
      this.enabled,
      document.createTextNode("Allow printer LAN requests from Browser and MCP"),
    );
    const hint = document.createElement("div");
    hint.id = "desktop-printer-network-hint";
    hint.className = "hint";
    hint.textContent =
      "Off by default and separate from AI/Spoolman access. This computer controls the allowed printer scopes. Authenticated clients can review discovery or explicitly check approved saved printers; clients cannot expand these permissions. Permission lasts for the server session.";
    const scopesLabel = document.createElement("label");
    scopesLabel.className = "printer-network-scope";
    scopesLabel.textContent = "Allowed private IPv4 scopes (one per line, up to 32)";
    this.scopes.maxLength = 4096;
    this.scopes.rows = 4;
    this.scopes.placeholder = "10.250.0.1/32";
    scopesLabel.append(this.scopes);
    const multicastLabel = document.createElement("label");
    multicastLabel.className = "printer-network-checkbox";
    this.multicast.type = "checkbox";
    this.multicast.checked = false;
    this.multicast.setAttribute("aria-describedby", "desktop-printer-multicast-hint");
    multicastLabel.append(
      this.multicast,
      document.createTextNode("Allow Bambu SSDP multicast discovery"),
    );
    const multicastHint = document.createElement("div");
    multicastHint.id = "desktop-printer-multicast-hint";
    multicastHint.className = "hint";
    multicastHint.textContent =
      "Bambu discovery sends multicast on routed interfaces. Devices outside the selected scopes may receive the request; only in-scope replies can produce candidates. This does not enable Bambu MQTT status.";
    const carbonLabel = document.createElement("label");
    carbonLabel.className = "printer-network-checkbox";
    this.carbon.type = "checkbox";
    this.carbon.checked = false;
    this.carbon.setAttribute("aria-describedby", "desktop-printer-carbon-hint");
    carbonLabel.append(
      this.carbon,
      document.createTextNode("Allow Centauri Carbon discovery broadcast"),
    );
    const carbonHint = document.createElement("div");
    carbonHint.id = "desktop-printer-carbon-hint";
    carbonHint.className = "hint";
    carbonHint.textContent =
      "A user-selected Carbon status check can send one UDP broadcast to 255.255.255.255:3000 on the routed interface, then a read-only status request after a matching response. Other LAN devices may receive the broadcast. This permission is separate from multicast; the MainboardID is not retained.";
    this.status.className = "value";
    this.status.setAttribute("aria-live", "polite");
    this.error.className = "settings-error";
    this.error.setAttribute("role", "status");
    this.apply.type = "button";
    this.apply.className = "btn";
    this.apply.textContent = "Apply printer LAN access";
    const changed = () => {
      this.dirty = true;
      this.error.textContent = "";
      this.render();
    };
    this.enabled.addEventListener("change", changed);
    this.scopes.addEventListener("input", changed);
    this.multicast.addEventListener("change", changed);
    this.carbon.addEventListener("change", changed);
    this.apply.addEventListener("click", () => {
      void this.applyDraft();
    });
    this.element.append(
      legend,
      this.status,
      enableLabel,
      hint,
      scopesLabel,
      multicastLabel,
      multicastHint,
      carbonLabel,
      carbonHint,
      this.apply,
      this.error,
    );
    this.render();
  }
  draftPolicy(): DesktopPrinterNetworkPolicy | null {
    return printerNetworkDraftPolicy(
      this.enabled.checked,
      this.scopes.value,
      this.multicast.checked,
      this.carbon.checked,
    );
  }
  sync(status: DesktopLibraryServerStatus, commit = false): void {
    const stopped = this.running && !status.running;
    this.running = status.running;
    const applied = status.running ? (status.printerNetwork ?? null) : null;
    this.status.textContent = applied
      ? `Applied printer LAN access: On for this session · ${applied.allowedScopes.join(", ")} · Bambu multicast ${applied.allowMulticastDiscovery ? "on" : "off"} · Carbon broadcast ${applied.allowCarbonBroadcast ? "on" : "off"}`
      : "Applied printer LAN access: Off";
    if (commit || stopped || !this.dirty) {
      this.enabled.checked = applied !== null;
      this.scopes.value = applied?.allowedScopes.join("\n") ?? "";
      this.multicast.checked = applied?.allowMulticastDiscovery ?? false;
      this.carbon.checked = applied?.allowCarbonBroadcast ?? false;
      this.dirty = false;
    }
    this.render();
  }
  setBusy(busy: boolean): void {
    this.busy = busy;
    this.render();
  }
  private render(): void {
    this.enabled.disabled = this.busy;
    this.scopes.disabled = this.busy || !this.enabled.checked;
    this.multicast.disabled = this.scopes.disabled;
    this.carbon.disabled = this.scopes.disabled;
    this.apply.hidden = !this.running;
    this.apply.disabled = this.busy || !this.dirty;
    this.element.setAttribute("aria-busy", String(this.busy));
  }
  private async applyDraft(): Promise<void> {
    if (this.busy || !this.running || !this.dirty) return;
    try {
      const policy = this.draftPolicy();
      this.error.textContent = "";
      this.setBusy(true);
      await this.onApply(policy);
    } catch (error) {
      this.error.textContent = error instanceof Error ? error.message : String(error);
    } finally {
      this.setBusy(false);
    }
  }
}
