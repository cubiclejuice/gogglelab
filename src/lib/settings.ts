// Settings page (modal). Every control applies immediately -- there's no
// Save button to forget -- and the caller persists + applies each change.

import type {
  AppearanceMode,
  Printer,
  SlicerApp,
  SlicerProfileDraft,
  SlicerProfileView,
} from "./ipc";
import { DEFAULT_ACCENT } from "./accent";

export const MAX_SLICER_PROFILE_SUGGESTIONS = 20;

export interface SettingsModel {
  appearanceMode: AppearanceMode;
  accentColor: string | null;
  useAccentModelColor: boolean;
  defaultColorPreview: boolean;
  defaultFolder: string | null;
  printers: Printer[];
  activePrinterId: string;
  slicers: SlicerApp[];
  slicerProfiles: SlicerProfileView[];
  defaultSlicerId: string | null;
  units: "mm" | "in";
}

export interface SlicerDefaultOption {
  value: string;
  label: string;
  disabled: boolean;
}

export interface SlicerProfileDraftEntry {
  key: string;
  profile: SlicerProfileDraft;
  originalPath: string | null;
  requiresTrust: boolean;
  trusted: boolean;
  needsPathReselect: boolean;
}

/** Keep existing profiles first, then add unique local candidates by ID/path. */
export function mergeSlicerProfileDrafts(
  existing: readonly SlicerProfileDraft[],
  additions: readonly SlicerProfileDraft[],
  reservedPaths: readonly string[] = [],
): SlicerProfileDraft[] {
  const merged = existing.map((profile) => ({ ...profile }));
  const ids = new Set(merged.flatMap((profile) => (profile.id ? [profile.id] : [])));
  const paths = new Set(
    [...reservedPaths, ...merged.map((profile) => profile.path)]
      .map(normalizeExecutablePath)
      .filter(Boolean),
  );

  for (const profile of additions) {
    const path = normalizeExecutablePath(profile.path);
    if (!path || (profile.id && ids.has(profile.id)) || paths.has(path)) continue;
    merged.push({ ...profile, id: profile.id ?? null, trust: false });
    if (profile.id) ids.add(profile.id);
    paths.add(path);
  }
  return merged;
}

/** Auto-detection is a review list only; it never creates a saved profile. */
export function reviewSlicerProfileSuggestions(
  candidates: readonly SlicerProfileDraft[],
  existing: readonly SlicerProfileDraft[],
  knownProfiles: readonly SlicerProfileView[],
): SlicerProfileDraft[] {
  const additions = candidates.map((candidate) => ({ ...candidate, id: null, trust: false }));
  return mergeSlicerProfileDrafts(
    existing,
    additions,
    knownProfiles.map((profile) => profile.app_path),
  )
    .slice(existing.length, existing.length + MAX_SLICER_PROFILE_SUGGESTIONS)
    .map((profile) => ({ ...profile, id: null, trust: false }));
}

export function slicerDefaultOptions(
  profiles: readonly SlicerProfileView[],
  defaultId: string | null,
): SlicerDefaultOption[] {
  const options: SlicerDefaultOption[] = [
    { value: "", label: "No default slicer", disabled: false },
  ];
  const seen = new Set<string>();
  for (const profile of profiles) {
    if (!profile.id || seen.has(profile.id)) continue;
    seen.add(profile.id);
    options.push({
      value: profile.id,
      label: profile.available ? profile.display_name : `${profile.display_name} — unavailable`,
      disabled: !profile.available,
    });
  }
  if (defaultId && !seen.has(defaultId)) {
    options.push({
      value: defaultId,
      label: "Saved slicer profile — unavailable",
      disabled: true,
    });
  }
  return options;
}

/** Local state for profile drafts. Rerendered settings models never overwrite edits. */
export class SlicerProfileDraftManager {
  private entriesValue: SlicerProfileDraftEntry[] = [];
  private sourceProfiles: SlicerProfileView[] = [];
  private suggestionsValue: SlicerProfileDraft[] = [];
  private selectedSuggestionIndexes = new Set<number>();
  private nextDraftId = 0;
  private awaitingRefresh: SlicerProfileDraft[] | null = null;
  private lastModelDefaultId: string | null | undefined;

  dirty = false;
  error = "";
  status = "";
  defaultId: string | null = null;
  suggestionsTruncated = false;

  constructor(profiles: readonly SlicerProfileView[] = []) {
    this.sync(profiles);
  }

  get entries(): SlicerProfileDraftEntry[] {
    return this.entriesValue.map((entry) => ({
      ...entry,
      profile: { ...entry.profile },
    }));
  }

  get drafts(): SlicerProfileDraft[] {
    return this.entriesValue.map((entry) => ({
      ...entry.profile,
      trust: entry.requiresTrust && entry.trusted,
    }));
  }

  get suggestions(): SlicerProfileDraft[] {
    return this.suggestionsValue.map((profile) => ({ ...profile }));
  }

  get selectedSuggestionCount(): number {
    return this.selectedSuggestionIndexes.size;
  }

  get canSave(): boolean {
    return (
      this.dirty &&
      this.entriesValue.every(
        (entry) => !entry.needsPathReselect && (!entry.requiresTrust || entry.trusted),
      )
    );
  }

  sync(profiles: readonly SlicerProfileView[]): void {
    this.sourceProfiles = profiles.map((profile) => ({ ...profile }));
    const custom = this.sourceProfiles.filter((profile) => profile.is_custom);
    if (this.awaitingRefresh) {
      if (sameProfileDrafts(profileViewsToDrafts(custom), this.awaitingRefresh)) {
        this.reconcileSavedProfiles(custom);
        this.awaitingRefresh = null;
      }
      return;
    }
    if (this.dirty) return;
    this.replaceFromServer(custom);
  }

  syncDefaultFromModel(defaultId: string | null): void {
    if (this.lastModelDefaultId === undefined || defaultId !== this.lastModelDefaultId) {
      this.defaultId = defaultId;
      this.lastModelDefaultId = defaultId;
    }
  }

  async setDefault(
    id: string | null,
    save: (id: string | null) => Promise<void>,
  ): Promise<boolean> {
    const previous = this.defaultId;
    if (
      id !== null &&
      !this.sourceProfiles.some((profile) => profile.id === id && profile.available)
    ) {
      this.error = "Choose an available slicer profile for the default.";
      this.status = "";
      return false;
    }
    this.error = "";
    this.status = "";
    try {
      await save(id);
      this.defaultId = id;
      this.status = id ? "Default slicer updated." : "Default slicer cleared.";
      return true;
    } catch (error) {
      this.defaultId = previous;
      this.error = `Could not save the default slicer: ${errorMessage(error)}`;
      return false;
    }
  }

  async addFromPicker(choosePath: () => Promise<string | null>): Promise<boolean> {
    this.clearMessage();
    let path: string | null;
    try {
      path = await choosePath();
    } catch (error) {
      this.error = `Could not choose a slicer app: ${errorMessage(error)}`;
      return false;
    }
    if (!path) return false;
    if (this.isDuplicatePath(path)) {
      this.error = "That application is already listed as a slicer profile.";
      return false;
    }
    this.entriesValue.push({
      key: `draft-${++this.nextDraftId}`,
      profile: { id: null, name: nameFromExecutablePath(path), path, trust: false },
      originalPath: null,
      requiresTrust: true,
      trusted: false,
      needsPathReselect: false,
    });
    this.dirty = true;
    this.status = "Custom app added to drafts. Review its name and confirm trust before saving.";
    return true;
  }

  async changePath(key: string, choosePath: () => Promise<string | null>): Promise<boolean> {
    const entry = this.entriesValue.find((candidate) => candidate.key === key);
    if (!entry) return false;
    this.clearMessage();
    let path: string | null;
    try {
      path = await choosePath();
    } catch (error) {
      this.error = `Could not choose a slicer app: ${errorMessage(error)}`;
      return false;
    }
    if (!path) return false;
    if (this.isDuplicatePath(path, key)) {
      this.error = "That application is already listed as a slicer profile.";
      return false;
    }
    entry.profile.path = path;
    let unchanged = false;
    if (entry.needsPathReselect) {
      entry.needsPathReselect = false;
      entry.requiresTrust = true;
      entry.trusted = false;
    } else {
      unchanged = entry.originalPath !== null && sameExecutablePath(path, entry.originalPath);
      entry.requiresTrust = !unchanged;
      entry.trusted = false;
    }
    this.dirty = true;
    this.status = unchanged
      ? "Original application path restored."
      : "Application path changed. Confirm trust before saving.";
    return true;
  }

  async detectSuggestions(detect: () => Promise<SlicerProfileDraft[]>): Promise<boolean> {
    this.clearMessage();
    try {
      const candidates = await detect();
      const unique = mergeSlicerProfileDrafts(
        this.drafts,
        candidates.map((candidate) => ({ ...candidate, id: null, trust: false })),
        this.sourceProfiles.map((profile) => profile.app_path),
      ).slice(this.entriesValue.length);
      this.suggestionsTruncated = unique.length > MAX_SLICER_PROFILE_SUGGESTIONS;
      this.suggestionsValue = unique
        .slice(0, MAX_SLICER_PROFILE_SUGGESTIONS)
        .map((profile) => ({ ...profile, id: null, trust: false }));
      this.selectedSuggestionIndexes.clear();
      this.status = this.suggestionsValue.length
        ? `${this.suggestionsValue.length} detected suggestion${this.suggestionsValue.length === 1 ? "" : "s"} ready to review${this.suggestionsTruncated ? `; showing the first ${MAX_SLICER_PROFILE_SUGGESTIONS}` : ""}.`
        : "No new slicer suggestions found.";
      return true;
    } catch (error) {
      this.error = `Could not detect slicer profiles: ${errorMessage(error)}`;
      return false;
    }
  }

  selectSuggestion(index: number, selected: boolean): void {
    if (index < 0 || index >= this.suggestionsValue.length) return;
    if (selected) this.selectedSuggestionIndexes.add(index);
    else this.selectedSuggestionIndexes.delete(index);
  }

  isSuggestionSelected(index: number): boolean {
    return this.selectedSuggestionIndexes.has(index);
  }

  stageSelectedSuggestions(): number {
    const selected = [...this.selectedSuggestionIndexes]
      .sort((left, right) => left - right)
      .map((index) => this.suggestionsValue[index])
      .filter((profile): profile is SlicerProfileDraft => profile !== undefined);
    const additions = mergeSlicerProfileDrafts([], selected, [
      ...this.sourceProfiles.map((profile) => profile.app_path),
      ...this.entriesValue.map((entry) => entry.profile.path),
    ]);
    for (const profile of additions) {
      this.entriesValue.push({
        key: `draft-${++this.nextDraftId}`,
        profile: { ...profile, id: null },
        originalPath: null,
        requiresTrust: true,
        trusted: false,
        needsPathReselect: true,
      });
    }
    const addedPaths = new Set(additions.map((profile) => normalizeExecutablePath(profile.path)));
    this.suggestionsValue = this.suggestionsValue.filter(
      (profile) => !addedPaths.has(normalizeExecutablePath(profile.path)),
    );
    this.selectedSuggestionIndexes.clear();
    if (additions.length) {
      this.dirty = true;
      this.error = "";
      this.status = `${additions.length} suggestion${additions.length === 1 ? "" : "s"} added to drafts. Confirm trust and save to keep them.`;
    }
    return additions.length;
  }

  setName(key: string, name: string): void {
    const entry = this.entriesValue.find((candidate) => candidate.key === key);
    if (!entry || entry.profile.name === name) return;
    entry.profile.name = name;
    this.dirty = true;
    this.clearMessage();
  }

  setTrust(key: string, trusted: boolean): void {
    const entry = this.entriesValue.find((candidate) => candidate.key === key);
    if (!entry || entry.needsPathReselect || !entry.requiresTrust || entry.trusted === trusted) {
      return;
    }
    entry.trusted = trusted;
    this.dirty = true;
    this.clearMessage();
  }

  requiresTrust(key: string): boolean {
    return this.entriesValue.find((candidate) => candidate.key === key)?.requiresTrust ?? false;
  }

  remove(key: string): void {
    const remaining = this.entriesValue.filter((entry) => entry.key !== key);
    if (remaining.length === this.entriesValue.length) return;
    this.entriesValue = remaining;
    this.dirty = true;
    this.clearMessage();
  }

  async save(saveProfiles: (profiles: SlicerProfileDraft[]) => Promise<void>): Promise<boolean> {
    this.error = "";
    this.status = "";
    if (!this.dirty) return false;
    if (this.entriesValue.some((entry) => !entry.profile.name.trim() || !entry.profile.path)) {
      this.error = "Each custom slicer profile needs a name and application path.";
      return false;
    }
    if (this.entriesValue.some((entry) => entry.needsPathReselect)) {
      this.error = "Choose the current app locally before saving an unavailable profile.";
      return false;
    }
    if (!this.entriesValue.every((entry) => !entry.requiresTrust || entry.trusted)) {
      this.error = "Confirm trust for each new or changed app before saving.";
      return false;
    }
    const profiles = this.entriesValue.map((entry) => ({
      ...entry.profile,
      name: entry.profile.name.trim(),
      trust: entry.requiresTrust && entry.trusted,
    }));
    try {
      await saveProfiles(profiles);
    } catch (error) {
      this.error = `Could not save slicer profiles: ${errorMessage(error)}`;
      return false;
    }
    this.entriesValue = this.entriesValue.map((entry, index) => ({
      ...entry,
      profile: profiles[index],
      originalPath: profiles[index].path,
      requiresTrust: false,
      trusted: true,
      needsPathReselect: false,
    }));
    this.dirty = false;
    this.awaitingRefresh = profiles.map((profile) => ({ ...profile }));
    this.status = "Slicer profiles saved.";
    return true;
  }

  private replaceFromServer(profiles: readonly SlicerProfileView[]): void {
    this.entriesValue = profiles.map((profile) => ({
      key: `saved:${profile.id}`,
      profile: {
        id: profile.id,
        name: profile.display_name,
        path: profile.app_path,
        trust: false,
      },
      originalPath: profile.app_path,
      requiresTrust: !profile.available,
      trusted: false,
      needsPathReselect: !profile.available,
    }));
    this.nextDraftId = 0;
  }

  private reconcileSavedProfiles(profiles: readonly SlicerProfileView[]): void {
    const viewsById = new Map(profiles.map((profile) => [profile.id, profile]));
    const byDraft = new Map(
      profileViewsToDrafts(profiles).map((profile) => [profileSemanticKey(profile), profile]),
    );
    this.entriesValue = this.entriesValue.map((entry) => {
      const saved = byDraft.get(profileSemanticKey(entry.profile));
      if (!saved) return entry;
      const available = saved.id ? (viewsById.get(saved.id)?.available ?? false) : false;
      return {
        ...entry,
        key: `saved:${saved.id}`,
        profile: { ...entry.profile, id: saved.id },
        originalPath: saved.path,
        requiresTrust: !available,
        trusted: false,
        needsPathReselect: !available,
      };
    });
  }

  private isDuplicatePath(path: string, exceptKey?: string): boolean {
    const key = normalizeExecutablePath(path);
    return (
      !key ||
      this.sourceProfiles.some((profile) => normalizeExecutablePath(profile.app_path) === key) ||
      this.entriesValue.some(
        (entry) => entry.key !== exceptKey && normalizeExecutablePath(entry.profile.path) === key,
      )
    );
  }

  private clearMessage(): void {
    this.error = "";
    this.status = "";
  }
}

function profileViewsToDrafts(profiles: readonly SlicerProfileView[]): SlicerProfileDraft[] {
  return profiles.map((profile) => ({
    id: profile.id,
    name: profile.display_name,
    path: profile.app_path,
    trust: false,
  }));
}

function sameProfileDrafts(
  left: readonly SlicerProfileDraft[],
  right: readonly SlicerProfileDraft[],
): boolean {
  const fingerprint = (profiles: readonly SlicerProfileDraft[]) =>
    profiles
      .map((profile) => profileSemanticKey(profile))
      .sort()
      .join("\n");
  return fingerprint(left) === fingerprint(right);
}

function profileSemanticKey(profile: SlicerProfileDraft): string {
  return `${profile.name.trim()}\u0000${normalizeExecutablePath(profile.path)}`;
}

function sameExecutablePath(left: string, right: string): boolean {
  return normalizeExecutablePath(left) === normalizeExecutablePath(right);
}

function normalizeExecutablePath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!normalized) return "";
  return /^(?:[a-z]:\/|\/\/)/i.test(normalized) ? normalized.toLowerCase() : normalized;
}

function nameFromExecutablePath(path: string): string {
  const leaf =
    path
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .pop() ?? "";
  return leaf.replace(/\.[^.]+$/, "") || leaf || "Custom slicer";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
export class SettingsPanel {
  private readonly root: HTMLDialogElement;
  private readonly appearanceSelect: HTMLSelectElement;
  private readonly appearanceError: HTMLElement;
  private readonly folderLabel: HTMLElement;
  private readonly printerSelect: HTMLSelectElement;
  private readonly slicerProfilesSection: SlicerProfilesSettingsSection;
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
  onSlicer: (id: string | null) => Promise<void> = async () => {};
  onChooseSlicerPath: () => Promise<string | null> = async () => null;
  onDetectSlicerProfiles: () => Promise<SlicerProfileDraft[]> = async () => [];
  onSaveSlicerProfiles: (profiles: SlicerProfileDraft[]) => Promise<void> = async () => {};
  onUnits: (u: "mm" | "in") => void = () => {};
  onAccent: (color: string | null) => void = () => {};

  onUseAccentModelColor: (enabled: boolean) => void = () => {};
  onDefaultColorPreview: (enabled: boolean) => void = () => {};
  onHide: () => void = () => {};
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
    const printerRow = document.createElement("div");
    printerRow.className = "settings-row";
    const printerLabel = document.createElement("label");
    printerLabel.className = "label";
    printerLabel.htmlFor = "settings-default-printer";
    printerLabel.textContent = "Default printer";
    this.printerSelect = select();
    this.printerSelect.id = "settings-default-printer";
    this.printerSelect.addEventListener("change", () => this.onPrinter(this.printerSelect.value));
    printerRow.append(printerLabel, this.printerSelect);
    card.appendChild(printerRow);

    this.slicerProfilesSection = new SlicerProfilesSettingsSection();
    this.slicerProfilesSection.onSlicer = (id) => this.onSlicer(id);
    this.slicerProfilesSection.onChooseSlicerPath = () => this.onChooseSlicerPath();
    this.slicerProfilesSection.onDetectSlicerProfiles = () => this.onDetectSlicerProfiles();
    this.slicerProfilesSection.onSaveSlicerProfiles = (profiles) =>
      this.onSaveSlicerProfiles(profiles);
    card.appendChild(this.slicerProfilesSection.element);

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
    this.onHide();
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
    this.slicerProfilesSection.render(m.slicerProfiles, m.defaultSlicerId);
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

class SlicerProfilesSettingsSection {
  readonly element: HTMLFieldSetElement;
  onSlicer: (id: string | null) => Promise<void> = async () => {};
  onChooseSlicerPath: () => Promise<string | null> = async () => null;
  onDetectSlicerProfiles: () => Promise<SlicerProfileDraft[]> = async () => [];
  onSaveSlicerProfiles: (profiles: SlicerProfileDraft[]) => Promise<void> = async () => {};

  private readonly manager = new SlicerProfileDraftManager();
  private readonly defaultSelect: HTMLSelectElement;
  private readonly builtInList: HTMLUListElement;
  private readonly customList: HTMLUListElement;
  private readonly suggestionList: HTMLUListElement;
  private readonly suggestionHeading: HTMLHeadingElement;
  private readonly suggestionEmpty: HTMLElement;
  private readonly addSelectedButton: HTMLButtonElement;
  private readonly saveButton: HTMLButtonElement;
  private readonly status: HTMLElement;
  private readonly error: HTMLElement;
  private readonly chooseButton: HTMLButtonElement;
  private readonly detectButton: HTMLButtonElement;
  private profiles: SlicerProfileView[] = [];

  constructor() {
    this.element = document.createElement("fieldset");
    this.element.className = "settings-section slicer-profile-settings";
    const legend = document.createElement("legend");
    legend.textContent = "Slicer profiles";

    const defaultRow = document.createElement("div");
    defaultRow.className = "settings-row";
    const defaultLabel = document.createElement("label");
    defaultLabel.className = "label";
    defaultLabel.htmlFor = "settings-default-slicer";
    defaultLabel.textContent = "Default slicer";
    this.defaultSelect = select();
    this.defaultSelect.id = "settings-default-slicer";
    this.defaultSelect.addEventListener("change", () => void this.updateDefault());
    defaultRow.append(defaultLabel, this.defaultSelect);

    const hint = document.createElement("div");
    hint.id = "settings-slicer-profile-hint";
    hint.className = "hint";
    hint.textContent =
      "Detected slicers are read-only. Add custom apps with the local app picker; confirm trust before saving new or changed apps.";
    const trustHint = document.createElement("div");
    trustHint.id = "settings-slicer-trust-hint";
    trustHint.className = "hint";
    trustHint.textContent =
      "Trust applies only to this locally selected app path. Imported paths are not added through this control.";

    const builtInHeading = document.createElement("h3");
    builtInHeading.textContent = "Detected slicers";
    this.builtInList = document.createElement("ul");
    this.builtInList.className = "slicer-profile-list";
    const builtInEmpty = document.createElement("p");
    builtInEmpty.className = "hint";
    builtInEmpty.textContent = "No built-in slicers detected.";
    builtInEmpty.dataset.slicerEmpty = "built-in";

    const customHeading = document.createElement("h3");
    customHeading.textContent = "Custom slicers";
    this.customList = document.createElement("ul");
    this.customList.className = "slicer-profile-list";
    const customEmpty = document.createElement("p");
    customEmpty.className = "hint";
    customEmpty.textContent = "No custom slicer profiles saved.";
    customEmpty.dataset.slicerEmpty = "custom";

    const actions = document.createElement("div");
    actions.className = "settings-actions slicer-profile-actions";
    this.chooseButton = button("Choose app to add custom slicer…");
    this.chooseButton.dataset.slicerFocus = "choose";
    this.chooseButton.addEventListener("click", () => void this.addCustomProfile());
    this.detectButton = button("Review auto-detected suggestions");
    this.detectButton.dataset.slicerFocus = "detect";
    this.detectButton.addEventListener("click", () => void this.reviewDetectedProfiles());
    actions.append(this.chooseButton, this.detectButton);

    this.suggestionHeading = document.createElement("h3");
    this.suggestionHeading.textContent = "Detected suggestions · review before adding";
    this.suggestionHeading.hidden = true;
    this.suggestionList = document.createElement("ul");
    this.suggestionList.className = "slicer-profile-list slicer-profile-suggestions";
    this.suggestionList.hidden = true;
    this.suggestionEmpty = document.createElement("p");
    this.suggestionEmpty.className = "hint";
    this.suggestionEmpty.textContent = "Select suggestions to add them to your unsaved drafts.";
    this.suggestionEmpty.hidden = true;
    this.addSelectedButton = button("Add selected suggestions to drafts");
    this.addSelectedButton.disabled = true;
    this.addSelectedButton.dataset.slicerFocus = "stage-suggestions";
    this.addSelectedButton.addEventListener("click", () => this.stageSuggestions());
    this.addSelectedButton.hidden = true;

    this.saveButton = button("Save profile changes");
    this.saveButton.disabled = true;
    this.saveButton.dataset.slicerFocus = "save-profiles";
    this.saveButton.addEventListener("click", () => void this.saveProfiles());

    this.status = document.createElement("div");
    this.status.id = "settings-slicer-status";
    this.status.className = "settings-status";
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");
    this.status.setAttribute("aria-atomic", "true");
    this.error = document.createElement("div");
    this.error.id = "settings-slicer-error";
    this.error.className = "settings-error";
    this.error.setAttribute("role", "alert");
    this.error.setAttribute("aria-live", "assertive");
    this.error.setAttribute("aria-atomic", "true");
    this.defaultSelect.setAttribute(
      "aria-describedby",
      `${hint.id} ${this.status.id} ${this.error.id}`,
    );

    this.element.append(
      legend,
      defaultRow,
      hint,
      trustHint,
      builtInHeading,
      this.builtInList,
      builtInEmpty,
      customHeading,
      this.customList,
      customEmpty,
      actions,
      this.suggestionHeading,
      this.suggestionList,
      this.suggestionEmpty,
      this.addSelectedButton,
      this.saveButton,
      this.status,
      this.error,
    );
    this.render([], null);
  }

  render(profiles: readonly SlicerProfileView[], defaultId: string | null): void {
    this.profiles = profiles.map((profile) => ({ ...profile }));
    this.manager.sync(this.profiles);
    this.manager.syncDefaultFromModel(defaultId);
    this.renderDefaultSelect();
    this.renderListsPreservingFocus();
    this.renderMessages();
  }

  private renderDefaultSelect(): void {
    const options = slicerDefaultOptions(this.profiles, this.manager.defaultId);
    this.defaultSelect.replaceChildren();
    for (const option of options) {
      const element = document.createElement("option");
      element.value = option.value;
      element.textContent = option.label;
      element.disabled = option.disabled;
      this.defaultSelect.appendChild(element);
    }
    this.defaultSelect.value = this.manager.defaultId ?? "";
  }

  private renderListsPreservingFocus(): void {
    const active = this.element.contains(document.activeElement)
      ? (document.activeElement as HTMLElement)
      : null;
    const focusKey = active?.dataset.slicerFocus;
    const selectionStart = active instanceof HTMLInputElement ? active.selectionStart : null;
    const selectionEnd = active instanceof HTMLInputElement ? active.selectionEnd : null;
    const builtIns = this.profiles.filter((profile) => !profile.is_custom);
    const builtInEmpty = this.element.querySelector<HTMLElement>('[data-slicer-empty="built-in"]');
    if (builtInEmpty) builtInEmpty.hidden = builtIns.length > 0;
    this.builtInList.replaceChildren(...builtIns.map((profile) => this.renderBuiltIn(profile)));

    const entries = this.manager.entries;
    const customEmpty = this.element.querySelector<HTMLElement>('[data-slicer-empty="custom"]');
    if (customEmpty) customEmpty.hidden = entries.length > 0;
    this.customList.replaceChildren(...entries.map((entry) => this.renderCustom(entry)));

    const suggestions = this.manager.suggestions;
    this.suggestionHeading.hidden = suggestions.length === 0;
    this.suggestionList.hidden = suggestions.length === 0;
    this.suggestionEmpty.hidden = suggestions.length === 0;
    this.addSelectedButton.hidden = suggestions.length === 0;
    this.suggestionList.replaceChildren(
      ...suggestions.map((profile, index) => this.renderSuggestion(profile, index)),
    );

    if (focusKey) {
      const replacement = findSlicerFocusControl(this.element, focusKey);
      replacement?.focus();
      if (
        replacement instanceof HTMLInputElement &&
        selectionStart !== null &&
        selectionEnd !== null
      ) {
        replacement.setSelectionRange(selectionStart, selectionEnd);
      } else if (!replacement && focusKey.startsWith("remove:")) {
        this.chooseButton.focus();
      }
    }
  }

  private renderBuiltIn(profile: SlicerProfileView): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "slicer-profile-item";
    const name = document.createElement("strong");
    name.textContent = profile.display_name;
    const availability = document.createElement("span");
    availability.className = "value";
    availability.textContent = profile.available
      ? "Detected · read-only"
      : "Unavailable · read-only";
    item.append(name, availability);
    if (profile.app_path) item.append(this.pathDisplay(profile.app_path));
    return item;
  }

  private renderCustom(entry: SlicerProfileDraftEntry): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "slicer-profile-item slicer-profile-custom";
    const nameLabel = document.createElement("label");
    nameLabel.className = "label";
    nameLabel.htmlFor = `settings-slicer-profile-name-${entry.key}`;
    nameLabel.textContent = "Profile name";
    const name = document.createElement("input");
    name.type = "text";
    name.id = nameLabel.htmlFor;
    name.maxLength = 80;
    name.value = entry.profile.name;
    name.dataset.slicerFocus = `name:${entry.key}`;
    name.addEventListener("input", () => {
      this.manager.setName(entry.key, name.value);
      this.renderMessages();
    });

    const pathLabel = document.createElement("span");
    pathLabel.className = "label";
    pathLabel.textContent = "Application";
    const profilePath = this.pathDisplay(entry.profile.path);
    profilePath.setAttribute("aria-label", `Application path: ${entry.profile.path}`);

    const saved = entry.profile.id
      ? this.profiles.find((profile) => profile.id === entry.profile.id)
      : undefined;
    const availability = document.createElement("span");
    availability.className = "value";
    availability.textContent = entry.needsPathReselect
      ? entry.originalPath === null
        ? "Select app locally before trust"
        : "Unavailable · choose current app path"
      : entry.requiresTrust
        ? entry.trusted
          ? "Trust confirmed · unsaved app path"
          : "Trust confirmation required"
        : entry.profile.id === null
          ? this.manager.dirty
            ? "Unsaved profile"
            : "Saved profile; refreshing"
          : saved?.available
            ? "Available"
            : "Unavailable";

    const row = document.createElement("div");
    row.className = "settings-row slicer-profile-name-row";
    row.append(nameLabel, name);
    const pathRow = document.createElement("div");
    pathRow.className = "settings-row slicer-profile-path-row";
    pathRow.append(pathLabel, profilePath);
    const actions = document.createElement("div");
    actions.className = "settings-actions slicer-profile-item-actions";
    const changePath = button("Choose different app…");
    changePath.setAttribute(
      "aria-label",
      `Choose a different app for ${entry.profile.name || "slicer profile"}`,
    );
    changePath.dataset.slicerFocus = `path:${entry.key}`;
    changePath.addEventListener("click", () => void this.changeProfilePath(entry.key, changePath));
    const remove = button("Remove");
    remove.setAttribute("aria-label", `Remove ${entry.profile.name || "unnamed slicer profile"}`);
    remove.dataset.slicerFocus = `remove:${entry.key}`;
    remove.addEventListener("click", () => {
      this.manager.remove(entry.key);
      this.renderListsPreservingFocus();
      this.renderMessages();
    });
    actions.append(changePath, remove);
    item.append(row, pathRow, availability, actions);

    if (entry.requiresTrust) {
      const trustLabel = document.createElement("label");
      trustLabel.className = "switch-row slicer-profile-trust";
      const trust = document.createElement("input");
      trust.type = "checkbox";
      trust.checked = entry.trusted;
      trust.disabled = entry.needsPathReselect;
      trust.dataset.slicerFocus = `trust:${entry.key}`;
      trust.setAttribute("aria-describedby", "settings-slicer-trust-hint");
      trust.addEventListener("change", () => {
        this.manager.setTrust(entry.key, trust.checked);
        this.renderCustomListWithFocus();
        this.renderMessages();
      });
      trustLabel.append(
        trust,
        document.createTextNode(
          "Trust this app so GoggleLab can launch it when you choose Slicer handoff",
        ),
      );
      item.appendChild(trustLabel);
    } else if (entry.profile.id) {
      const trusted = document.createElement("span");
      trusted.className = "hint";
      trusted.textContent = "Trusted for slicer handoff";
      item.appendChild(trusted);
    }
    return item;
  }

  private renderSuggestion(profile: SlicerProfileDraft, index: number): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "slicer-profile-item slicer-profile-suggestion";
    const label = document.createElement("label");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = this.manager.isSuggestionSelected(index);
    checkbox.dataset.slicerFocus = `suggestion:${index}`;
    checkbox.setAttribute("aria-label", `Select detected app ${profile.name}`);
    checkbox.addEventListener("change", () => {
      this.manager.selectSuggestion(index, checkbox.checked);
      this.renderMessages();
    });
    const name = document.createElement("strong");
    name.textContent = profile.name;
    label.append(checkbox, name);
    item.append(label, this.pathDisplay(profile.path));
    return item;
  }

  private pathDisplay(path: string): HTMLElement {
    const code = document.createElement("code");
    code.className = "slicer-profile-path";
    code.textContent = path;
    code.title = path;
    return code;
  }

  private async updateDefault(): Promise<void> {
    const selected = this.defaultSelect.value || null;
    this.defaultSelect.disabled = true;
    await this.manager.setDefault(selected, this.onSlicer);
    this.defaultSelect.disabled = false;
    this.renderDefaultSelect();
    this.renderMessages();
  }

  private async addCustomProfile(): Promise<void> {
    this.chooseButton.disabled = true;
    const added = await this.manager.addFromPicker(this.onChooseSlicerPath);
    this.chooseButton.disabled = false;
    this.renderListsPreservingFocus();
    this.renderMessages();
    if (added) {
      const entry = this.manager.entries.at(-1);
      if (entry?.profile.id === null) {
        findSlicerFocusControl(this.element, `name:${entry.key}`)?.focus();
      }
    }
  }

  private async changeProfilePath(key: string, control: HTMLButtonElement): Promise<void> {
    control.disabled = true;
    await this.manager.changePath(key, this.onChooseSlicerPath);
    control.disabled = false;
    this.renderListsPreservingFocus();
    this.renderMessages();
  }

  private async reviewDetectedProfiles(): Promise<void> {
    this.detectButton.disabled = true;
    await this.manager.detectSuggestions(this.onDetectSlicerProfiles);
    this.detectButton.disabled = false;
    this.renderListsPreservingFocus();
    this.renderMessages();
  }

  private stageSuggestions(): void {
    this.manager.stageSelectedSuggestions();
    this.renderListsPreservingFocus();
    this.renderMessages();
  }

  private async saveProfiles(): Promise<void> {
    this.saveButton.disabled = true;
    await this.manager.save(this.onSaveSlicerProfiles);
    this.renderListsPreservingFocus();
    this.renderMessages();
  }

  private renderCustomListWithFocus(): void {
    const active = this.element.contains(document.activeElement)
      ? (document.activeElement as HTMLElement)
      : null;
    const focusKey = active?.dataset.slicerFocus;
    this.customList.replaceChildren(
      ...this.manager.entries.map((entry) => this.renderCustom(entry)),
    );
    const replacement = focusKey ? findSlicerFocusControl(this.element, focusKey) : null;
    replacement?.focus();
  }

  private renderMessages(): void {
    this.status.textContent = this.manager.status;
    this.error.textContent = this.manager.error;
    if (
      this.manager.dirty &&
      this.manager.entries.some((entry) => entry.requiresTrust && !entry.trusted) &&
      !this.manager.error
    ) {
      this.status.textContent = "Confirm trust for each new or changed app before saving.";
    }
    this.addSelectedButton.disabled = this.manager.selectedSuggestionCount === 0;
    this.saveButton.disabled = !this.manager.canSave;
    if (this.manager.suggestionsTruncated && !this.manager.status) {
      this.status.textContent = `Showing at most ${MAX_SLICER_PROFILE_SUGGESTIONS} detected suggestions.`;
    }
  }
}

function findSlicerFocusControl(root: HTMLElement, key: string): HTMLElement | null {
  return (
    [...root.querySelectorAll<HTMLElement>("[data-slicer-focus]")].find(
      (control) => control.dataset.slicerFocus === key,
    ) ?? null
  );
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
