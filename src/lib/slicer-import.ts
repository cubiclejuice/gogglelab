import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { SlicerProfileDraft } from "./ipc";
import type { PrintventorySlicerImportRecord } from "./printventory-import";

interface SlicerImportChoice {
  action: "create" | "skip";
  name?: string;
  path: string;
  trusted: boolean;
}

export function buildSlicerImportDrafts(
  records: readonly PrintventorySlicerImportRecord[],
  choices: ReadonlyMap<number, SlicerImportChoice>,
): SlicerProfileDraft[] {
  const names = new Set<string>();
  const paths = new Set<string>();
  const drafts: SlicerProfileDraft[] = [];
  for (const [index, record] of records.entries()) {
    const choice = choices.get(index);
    if (!choice || !["skip", "create"].includes(choice.action)) {
      throw new Error("Review every imported slicer: choose create or skip.");
    }
    if (choice.action === "skip") continue;
    if (!choice.path.trim()) throw new Error("Choose each slicer app locally before importing.");
    if (!choice.trusted) throw new Error("Confirm trust for every selected slicer app.");
    const name = (choice.name ?? record.name).trim();
    const hasControl = Array.from(name).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
    });
    if (!name || [...name].length > 128 || hasControl) {
      throw new Error("Each slicer needs a name of 1–128 characters without control characters.");
    }
    const pathKey = choice.path.replaceAll("\\", "/").replace(/\/+$/u, "").toLowerCase();
    if (names.has(name.toLowerCase()) || paths.has(pathKey)) {
      throw new Error("Duplicate slicer names or selected apps: rename a profile or skip it.");
    }
    names.add(name.toLowerCase());
    paths.add(pathKey);
    drafts.push({ id: null, name, path: choice.path, trust: true });
  }
  return drafts;
}

export class SlicerImportReviewState {
  private choices = new Map<number, SlicerImportChoice>();
  private completedIds: string[] | null = null;
  private applying = false;
  constructor(readonly records: readonly PrintventorySlicerImportRecord[]) {}

  get completed(): boolean {
    return this.completedIds !== null;
  }
  get summary(): string {
    return !this.completed || this.records.length === 0
      ? ""
      : ` Slicer profiles: ${this.completedIds!.length} created, ${this.records.length - this.completedIds!.length} skipped; default unchanged.`;
  }
  async apply(
    importProfiles: (profiles: SlicerProfileDraft[]) => Promise<string[]>,
  ): Promise<void> {
    if (this.completed) return;
    if (this.applying || !this.ready) throw new Error("Review every slicer before applying.");
    this.applying = true;
    try {
      const drafts = this.drafts;
      this.completedIds = drafts.length ? await importProfiles(drafts) : [];
    } finally {
      this.applying = false;
    }
  }

  get drafts(): SlicerProfileDraft[] {
    return buildSlicerImportDrafts(this.records, this.choices);
  }
  get problem(): string {
    try {
      buildSlicerImportDrafts(this.records, this.choices);
      return "";
    } catch (reason) {
      return reason instanceof Error ? reason.message : String(reason);
    }
  }
  get ready(): boolean {
    return this.problem === "";
  }
  choice(index: number): SlicerImportChoice | undefined {
    const choice = this.choices.get(index);
    return choice ? { ...choice } : undefined;
  }
  choose(index: number, action: "create" | "skip"): void {
    if (this.applying || this.completed) return;
    // Choosing again requires a fresh local selection and trust.
    this.choices.set(index, { action, path: "", trusted: false });
  }
  selectPath(index: number, path: string): void {
    if (this.applying || this.completed) return;
    const choice = this.choices.get(index);
    if (choice?.action === "create") {
      this.choices.set(index, { ...choice, path, trusted: false });
    }
  }
  setName(index: number, name: string): void {
    if (this.applying || this.completed) return;
    const choice = this.choices.get(index);
    if (choice?.action === "create") this.choices.set(index, { ...choice, name });
  }
  setTrust(index: number, trusted: boolean): void {
    if (this.applying || this.completed) return;
    const choice = this.choices.get(index);
    if (choice?.action === "create") {
      this.choices.set(index, { ...choice, trusted: Boolean(choice.path) && trusted });
    }
  }
}

/** A bounded review section; JSON paths are text only and never picker defaults. */
export class SlicerImportReview {
  readonly element = document.createElement("section");
  state = new SlicerImportReviewState([]);
  private busy = false;
  private picking = false;
  private message = "";
  constructor(private readonly onChange: () => void) {
    this.element.className = "recovery-slicer-import";
  }
  reset(records: readonly PrintventorySlicerImportRecord[]): void {
    this.state = new SlicerImportReviewState(records);
    this.message = "";
    this.render();
  }
  get ready(): boolean {
    return !this.picking && (this.state.completed || this.state.ready);
  }
  get reviewData(): readonly unknown[] {
    return this.state.records;
  }
  get summary(): string {
    return this.state.summary;
  }
  setBusy(busy: boolean): void {
    this.busy = busy;
    this.render();
  }
  async apply(
    importProfiles: (profiles: SlicerProfileDraft[]) => Promise<string[]>,
  ): Promise<void> {
    if (!this.ready) throw new Error("Review every slicer and confirm local app trust first.");
    await this.state.apply(importProfiles);
    this.render();
  }
  private render(): void {
    this.element.hidden = this.state.records.length === 0;
    this.element.replaceChildren();
    const heading = document.createElement("h4");
    heading.textContent = "Slicer profiles";
    const hint = document.createElement("p");
    hint.textContent =
      "Imported paths are hints from another computer. Choose and trust each app locally, or skip it. Existing profiles and your default stay as saved. Nothing launches during import.";
    const status = document.createElement("p");
    status.setAttribute("role", "status");
    status.textContent = this.message || this.summary || this.state.problem;
    this.element.append(heading, hint, status);
    for (const [index, record] of this.state.records.entries()) {
      const choice = this.state.choice(index);
      const fieldset = document.createElement("fieldset");
      fieldset.disabled = this.busy || this.picking || this.state.completed;
      const legend = document.createElement("legend");
      legend.textContent = record.name;
      const importedPath = document.createElement("p");
      importedPath.className = "hint";
      importedPath.style.overflowWrap = "anywhere";
      importedPath.textContent = `Imported path hint: ${record.path}`;
      const label = document.createElement("label");
      label.textContent = "Import action ";
      const action = document.createElement("select");
      const placeholder = new Option("Choose an action", "");
      placeholder.disabled = true;
      action.add(placeholder);
      action.add(new Option("Create with a locally selected app", "create"));
      action.add(new Option("Skip this slicer", "skip"));
      action.value = choice?.action ?? "";
      action.addEventListener("change", () => {
        if (action.value === "create" || action.value === "skip") {
          this.state.choose(index, action.value);
          this.message = "";
          this.render();
          this.onChange();
        }
      });
      label.append(action);
      fieldset.append(legend, importedPath, label);
      if (choice?.action === "create") {
        const nameLabel = document.createElement("label");
        nameLabel.textContent = "Profile name ";
        const name = document.createElement("input");
        name.value = choice.name ?? record.name;
        name.maxLength = 128;
        name.addEventListener("input", () => {
          this.state.setName(index, name.value);
          status.textContent = this.state.problem;
          this.onChange();
        });
        nameLabel.append(name);
        const picked = document.createElement("p");
        picked.style.overflowWrap = "anywhere";
        picked.textContent = choice.path || "No local app selected";
        const pick = document.createElement("button");
        pick.type = "button";
        pick.className = "btn";
        pick.textContent = "Choose local slicer app…";
        pick.addEventListener("click", () => void this.pick(index));
        const trustLabel = document.createElement("label");
        const trust = document.createElement("input");
        trust.type = "checkbox";
        trust.checked = choice.trusted;
        trust.disabled = !choice.path;
        trust.addEventListener("change", () => {
          this.state.setTrust(index, trust.checked);
          status.textContent = this.state.problem;
          this.onChange();
        });
        trustLabel.append(
          trust,
          document.createTextNode(" Trust this selected app for slicer handoff"),
        );
        fieldset.append(nameLabel, picked, pick, trustLabel);
      }
      this.element.append(fieldset);
    }
  }
  private async pick(index: number): Promise<void> {
    if (this.busy || this.picking || this.state.completed) return;
    this.picking = true;
    this.message = "";
    this.render();
    this.onChange();
    try {
      const selected = await openDialog({ multiple: false });
      if (typeof selected === "string") this.state.selectPath(index, selected);
    } catch (reason) {
      this.message = `Could not select a local app: ${String(reason)}`;
    } finally {
      this.picking = false;
      this.render();
      this.onChange();
    }
  }
}
