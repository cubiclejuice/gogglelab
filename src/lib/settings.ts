// Settings page (modal). Every control applies immediately -- there's no
// Save button to forget -- and the caller persists + applies each change.

import type { AppearanceMode, Printer, SlicerApp } from "./ipc";
import { DEFAULT_ACCENT } from "./accent";

export interface SettingsModel {
  appearanceMode: AppearanceMode;
  accentColor: string | null;
  useAccentModelColor: boolean;
  defaultColorPreview: boolean;
  defaultFolder: string | null;
  printers: Printer[];
  activePrinterId: string;
  slicers: SlicerApp[];
  defaultSlicerId: string | null;
  units: "mm" | "in";
}
export class SettingsPanel {
  private readonly root: HTMLDialogElement;
  private readonly appearanceSelect: HTMLSelectElement;
  private readonly appearanceError: HTMLElement;
  private readonly folderLabel: HTMLElement;
  private readonly printerSelect: HTMLSelectElement;
  private readonly slicerSelect: HTMLSelectElement;
  private readonly unitsMm: HTMLInputElement;
  private readonly unitsIn: HTMLInputElement;
  private readonly accentInput: HTMLInputElement;
  private readonly accentValue: HTMLElement;
  private readonly accentError: HTMLElement;
  private readonly modelColorToggle: HTMLInputElement;
  private readonly colorPreviewToggle: HTMLInputElement;
  private readonly colorPreviewError: HTMLElement;
  private readonly extensionSectionSlot: HTMLElement;
  private invoker: HTMLElement | null = null;

  onAppearanceMode: (mode: AppearanceMode) => void = () => {};
  onChooseFolder: () => void = () => {};
  onClearFolder: () => void = () => {};
  onPrinter: (id: string) => void = () => {};
  onSlicer: (id: string) => void = () => {};
  onUnits: (u: "mm" | "in") => void = () => {};
  onAccent: (color: string | null) => void = () => {};

  onUseAccentModelColor: (enabled: boolean) => void = () => {};
  onDefaultColorPreview: (enabled: boolean) => void = () => {};
  constructor(container: HTMLElement) {
    this.root = document.createElement("dialog");
    this.root.className = "settings-dialog";
    this.root.setAttribute("aria-labelledby", "settings-title");
    this.root.addEventListener("click", (e) => {
      if (e.target === this.root) this.hide();
    });
    this.root.addEventListener("cancel", (e) => {
      e.preventDefault();
      this.hide();
    });

    const card = document.createElement("div");
    card.className = "sheet settings-card";
    const title = document.createElement("div");
    title.className = "sheet-title";
    const titleText = document.createElement("h2");
    titleText.id = "settings-title";
    titleText.textContent = "Settings";
    const close = button("Done");
    close.addEventListener("click", () => this.hide());
    title.appendChild(titleText);
    card.appendChild(title);

    const appearanceRow = document.createElement("div");
    appearanceRow.className = "settings-row";
    const appearanceLabel = document.createElement("label");
    appearanceLabel.className = "label";
    appearanceLabel.htmlFor = "settings-appearance";
    appearanceLabel.textContent = "Appearance";
    this.appearanceSelect = select();
    this.appearanceSelect.id = "settings-appearance";
    fill(
      this.appearanceSelect,
      [
        ["system", "System"],
        ["light", "Light"],
        ["dark", "Dark"],
      ],
      "system",
    );
    this.appearanceSelect.addEventListener("change", () =>
      this.onAppearanceMode(this.appearanceSelect.value as AppearanceMode),
    );
    appearanceRow.append(appearanceLabel, this.appearanceSelect);
    this.appearanceError = document.createElement("div");
    this.appearanceError.id = "settings-appearance-error";
    this.appearanceError.className = "settings-error";
    this.appearanceError.setAttribute("role", "status");
    this.appearanceSelect.setAttribute("aria-describedby", this.appearanceError.id);
    card.append(appearanceRow, this.appearanceError);

    // Default folder
    const folderRow = row("Startup folder");
    this.folderLabel = document.createElement("span");
    this.folderLabel.className = "value";
    const choose = button("Choose…");
    choose.addEventListener("click", () => this.onChooseFolder());
    const clear = button("Clear");
    clear.addEventListener("click", () => this.onClearFolder());
    folderRow.append(this.folderLabel, choose, clear);
    card.appendChild(folderRow);

    // Default printer
    const printerRow = row("Default printer");
    this.printerSelect = select();
    this.printerSelect.addEventListener("change", () => this.onPrinter(this.printerSelect.value));
    printerRow.appendChild(this.printerSelect);
    card.appendChild(printerRow);

    // Default slicer
    const slicerRow = row("Default slicer");
    this.slicerSelect = select();
    this.slicerSelect.addEventListener("change", () => this.onSlicer(this.slicerSelect.value));
    slicerRow.appendChild(this.slicerSelect);
    card.appendChild(slicerRow);

    // Units
    const unitsRow = row("Units");
    this.unitsMm = radio("units", "mm");
    this.unitsIn = radio("units", "in");
    const mmLabel = labelFor(this.unitsMm, "mm");
    const inLabel = labelFor(this.unitsIn, "inches (display only)");
    this.unitsMm.addEventListener("change", () => this.onUnits("mm"));
    this.unitsIn.addEventListener("change", () => this.onUnits("in"));
    unitsRow.append(mmLabel, inLabel);
    card.appendChild(unitsRow);

    const previewSection = document.createElement("fieldset");
    previewSection.className = "settings-section";
    const previewLegend = document.createElement("legend");
    previewLegend.textContent = "Model appearance";
    const previewLabel = document.createElement("label");
    previewLabel.className = "switch-row";
    this.colorPreviewToggle = document.createElement("input");
    this.colorPreviewToggle.type = "checkbox";
    this.colorPreviewToggle.className = "switch-input";
    this.colorPreviewToggle.addEventListener("change", () =>
      this.onDefaultColorPreview(this.colorPreviewToggle.checked),
    );
    const previewTrack = document.createElement("span");
    previewTrack.className = "switch-track";
    previewTrack.setAttribute("aria-hidden", "true");
    previewLabel.append(
      this.colorPreviewToggle,
      previewTrack,
      document.createTextNode("Color preview by default"),
    );
    const previewHint = document.createElement("div");
    previewHint.className = "hint";
    previewHint.textContent = "The viewport switch can change it for the current session.";
    this.colorPreviewError = document.createElement("div");
    this.colorPreviewError.className = "settings-error";
    this.colorPreviewError.setAttribute("role", "status");
    previewSection.append(previewLegend, previewLabel, previewHint, this.colorPreviewError);
    card.appendChild(previewSection);

    const accentSection = document.createElement("fieldset");
    accentSection.className = "accent-section";
    const legend = document.createElement("legend");
    legend.textContent = "Accent Color";
    const accentRow = document.createElement("div");
    accentRow.className = "settings-row";
    this.accentInput = document.createElement("input");
    this.accentInput.type = "color";
    this.accentInput.className = "accent-picker";
    this.accentInput.setAttribute("aria-label", "Accent color");
    this.accentValue = document.createElement("span");
    this.accentValue.className = "value";
    this.accentInput.addEventListener("input", () => this.onAccent(this.accentInput.value));
    const reset = button("Reset");
    reset.title = "Restore the default green accent";
    reset.addEventListener("click", () => this.onAccent(null));
    accentRow.append(this.accentInput, this.accentValue, reset);
    const modelColorLabel = document.createElement("label");
    modelColorLabel.className = "switch-row";
    this.modelColorToggle = document.createElement("input");
    this.modelColorToggle.type = "checkbox";
    this.modelColorToggle.className = "switch-input";
    this.modelColorToggle.setAttribute("aria-label", "Use accent color for model");
    this.modelColorToggle.addEventListener("change", () =>
      this.onUseAccentModelColor(this.modelColorToggle.checked),
    );
    const switchTrack = document.createElement("span");
    switchTrack.className = "switch-track";
    switchTrack.setAttribute("aria-hidden", "true");
    const switchText = document.createElement("span");
    switchText.textContent = "Use accent color for model";
    modelColorLabel.append(this.modelColorToggle, switchTrack, switchText);
    this.accentError = document.createElement("div");
    this.accentError.className = "accent-error";
    this.accentError.setAttribute("role", "status");
    accentSection.append(legend, accentRow, modelColorLabel, this.accentError);
    card.appendChild(accentSection);

    this.extensionSectionSlot = document.createElement("div");
    this.extensionSectionSlot.className = "settings-extension-sections";
    card.appendChild(this.extensionSectionSlot);

    const hint = document.createElement("div");
    hint.className = "hint";
    hint.textContent =
      "Printers are detected from the installed slicers each launch. Changes apply immediately. ⌘, opens this page.";
    card.appendChild(hint);
    const actions = document.createElement("div");
    actions.className = "settings-actions";
    actions.appendChild(close);
    card.appendChild(actions);

    this.root.appendChild(card);
    container.appendChild(this.root);
  }

  get isOpen(): boolean {
    return this.root.open;
  }
  show(model: SettingsModel, invoker?: HTMLElement) {
    this.render(model);
    const active = invoker ?? document.activeElement;
    this.invoker = active instanceof HTMLElement ? active : null;
    if (!this.root.open) this.root.showModal();
  }
  hide() {
    if (this.root.open) this.root.close();
    const invoker = this.invoker;
    this.invoker = null;
    invoker?.focus();
  }

  render(m: SettingsModel) {
    this.appearanceSelect.value = m.appearanceMode;
    this.setAccent(m.accentColor);
    this.modelColorToggle.checked = m.useAccentModelColor;
    this.colorPreviewToggle.checked = m.defaultColorPreview;
    this.folderLabel.textContent = m.defaultFolder ?? "None — ask on launch";
    this.folderLabel.title = m.defaultFolder ?? "";
    fill(
      this.printerSelect,
      m.printers.map((p) => [p.id, p.name]),
      m.activePrinterId,
    );
    fill(
      this.slicerSelect,
      m.slicers.length
        ? m.slicers.map((s) => [s.id, s.display_name])
        : [["", "No slicer detected"]],
      m.defaultSlicerId ?? "",
    );
    this.unitsMm.checked = m.units === "mm";
    this.unitsIn.checked = m.units === "in";
  }

  setAppearanceModeError(message: string) {
    this.appearanceError.textContent = message;
  }

  setAccent(color: string | null) {
    this.accentInput.value = color ?? DEFAULT_ACCENT;
    this.accentValue.textContent = color?.toUpperCase() ?? "Default green";
  }

  setAccentError(message: string) {
    this.accentError.textContent = message;
  }

  setColorPreviewError(message: string) {
    this.colorPreviewError.textContent = message;
  }

  addExtensionSection(section: HTMLElement | null): void {
    if (section) this.extensionSectionSlot.appendChild(section);
  }
}

function row(label: string): HTMLElement {
  const r = document.createElement("div");
  r.className = "settings-row";
  const l = document.createElement("span");
  l.textContent = label;
  l.className = "label";
  r.appendChild(l);
  return r;
}
function button(text: string): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = text;
  b.className = "btn";
  return b;
}
function select(): HTMLSelectElement {
  const s = document.createElement("select");
  s.className = "select";
  return s;
}
function fill(s: HTMLSelectElement, opts: [string, string][], value: string) {
  s.innerHTML = "";
  for (const [v, t] of opts) {
    const o = document.createElement("option");
    o.value = v;
    o.textContent = t;
    s.appendChild(o);
  }
  s.value = value;
}
function radio(name: string, value: string): HTMLInputElement {
  const r = document.createElement("input");
  r.type = "radio";
  r.name = name;
  r.value = value;
  return r;
}
function labelFor(input: HTMLInputElement, text: string): HTMLLabelElement {
  const l = document.createElement("label");
  l.className = "settings-choice";
  l.append(input, document.createTextNode(text));
  return l;
}
