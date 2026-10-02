// S8: all four entry points converge on loadModel(), which drives the
// viewport, the triage HUD, and error/loading states (spec §4).

import { getCurrentWindow, type DragDropEvent } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  openModel,
  readStepFile,
  readFusionFile,
  installFusionBridge,
  modelGeometry,
  isAppError,
  describeError,
  detectSlicers,
  getDefaultSlicer,
  setDefaultSlicer,
  getSettings,
  setActivePrinter,
  setDisplayUnits,
  setAppearanceMode,
  setAccentColor,
  setDefaultColorPreview,
  setDefaultFolder,
  listDir,
  detectPrinters,
  moveTreeFileToTrash,
  getDefaultSlicer as getDefaultSlicerId,
  type Printer,
  type AppearanceMode,
  type SlicerApp,
  type IntegrityReport,
  type ModelSummary,
} from "./lib/ipc";
import { Viewport } from "./lib/viewport";
import { Hud } from "./lib/hud";
import { PlateSession } from "./lib/plate-session";
import { PlateControls } from "./lib/plate-controls";
import { previewStep } from "./lib/step-preview";
import { cadPreviewLabel } from "./lib/cad-format";
import { FileTree } from "./lib/filetree";
import { SettingsPanel } from "./lib/settings";
import { applyAccent } from "./lib/accent";
import { applyTheme } from "./lib/theme";
import type { AppExtensions } from "./lib/app-extensions";
import { mountRecoveryControl } from "./lib/recovery";
import { isLibraryRequestCurrent, type LibraryRequestToken } from "./lib/library-load-guard";
import { handoffAvailability } from "./lib/handoff-availability";
import "./styles.css";

export function startApp(extensions: AppExtensions): void {
  const APP_NAME = "GoggleLab";

  const app = document.querySelector<HTMLDivElement>("#app")!;

  // Left: folder sidebar. Right: a column -- the viewport (with overlay and
  // sheets layered on it) above the readout strip (design/Main.dc.html).
  const tree = new FileTree(
    app,
    undefined,
    extensions.onZipExtracted,
    extensions.createModelsFolder,
  );
  window.addEventListener(
    "beforeunload",
    () => {
      stepController?.abort();
      tree.dispose();
      viewport.dispose();
      plateControls.dispose();
      colorFeature.dispose();
      extensions.dispose?.();
    },
    { once: true },
  );
  const libraryPanel = extensions.createLibraryPanel?.(app) ?? null;
  const libraryService = extensions.library;
  let activeEntitlements = extensions.currentEntitlements?.() ?? {
    library: false,
    colorways: false,
    slicerHandoff: false,
    revision: 0,
  };
  const main = document.createElement("div");
  main.className = "main";
  app.appendChild(main);

  const viewportWrap = document.createElement("div");
  viewportWrap.className = "viewport-wrap";
  main.appendChild(viewportWrap);

  const viewportEl = document.createElement("div");
  viewportEl.className = "viewport-canvas";
  viewportWrap.appendChild(viewportEl);

  const overlay = document.createElement("div");
  overlay.className = "overlay";
  viewportWrap.appendChild(overlay);
  const sidebarReveal = document.createElement("button");
  sidebarReveal.type = "button";
  sidebarReveal.className = "btn sidebar-reveal";
  sidebarReveal.textContent = "Show sidebar";
  sidebarReveal.setAttribute("aria-controls", "sidebar");
  sidebarReveal.setAttribute("aria-label", "Show sidebar");
  sidebarReveal.title = "Show sidebar (⌘B)";
  sidebarReveal.addEventListener("click", () => {
    tree.setVisible(true);
    tree.focusTree();
  });
  viewportWrap.appendChild(sidebarReveal);
  tree.onVisibilityChanged = (visible) => {
    sidebarReveal.classList.toggle("visible", !visible);
  };
  sidebarReveal.classList.toggle("visible", !tree.isVisible);

  // Cold start modal: no file to open and no remembered folder. Native dialog
  // focus management keeps the entry actions reachable without hiding the app.
  const folderModal = document.createElement("dialog");
  folderModal.className = "folder-dialog";
  folderModal.setAttribute("aria-labelledby", "folder-modal-title");
  folderModal.addEventListener("click", (e) => {
    if (e.target === folderModal) hideFolderModal();
  });
  folderModal.addEventListener("cancel", (e) => {
    e.preventDefault();
    hideFolderModal();
  });
  const modalCard = document.createElement("div");
  modalCard.className = "sheet folder-card";
  const appIdentity = document.createElement("div");
  appIdentity.className = "folder-brand";
  appIdentity.textContent = APP_NAME;
  const modalTitle = document.createElement("h2");
  modalTitle.id = "folder-modal-title";
  modalTitle.textContent = "Open a 3D model";
  const modalHint = document.createElement("div");
  modalHint.className = "hint folder-hint";
  modalHint.textContent =
    "Choose a folder to browse, open one file, or drop a model into the outlined zone.";
  const modalActions = document.createElement("div");
  modalActions.className = "folder-actions";
  const modalButton = document.createElement("button");
  modalButton.type = "button";
  modalButton.className = "btn primary folder-button";
  modalButton.textContent = "Choose Folder…";
  modalButton.addEventListener("click", () => tree.onPickFolder());
  const modalOpenFile = document.createElement("button");
  modalOpenFile.type = "button";
  modalOpenFile.textContent = "Open File…";
  modalOpenFile.addEventListener("click", () => void openFileDialog());
  modalActions.append(modalButton, modalOpenFile);
  const dropZone = document.createElement("div");
  dropZone.className = "drop-zone";
  dropZone.textContent = "Drop a model here";
  modalCard.append(appIdentity, modalTitle, modalHint, modalActions, dropZone);
  folderModal.appendChild(modalCard);
  main.appendChild(folderModal);
  function showFolderModal() {
    if (!folderModal.open) folderModal.showModal();
  }
  function hideFolderModal() {
    if (folderModal.open) folderModal.close();
  }
  async function openFileDialog() {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "3D models", extensions: ["stl", "3mf", "step", "stp", "f3d"] }],
    });
    if (typeof path === "string") {
      hideFolderModal();
      void loadModel(path);
    }
  }

  type OverlayState =
    | { kind: "hint"; text: string }
    | { kind: "progress"; text: string }
    | { kind: "error"; text: string; retry?: () => void; connectFusion?: () => void }
    | null;

  function setOverlay(state: OverlayState) {
    overlay.replaceChildren();
    overlay.className = "overlay";
    overlay.removeAttribute("role");
    overlay.removeAttribute("aria-live");
    if (!state) return;
    overlay.classList.add(state.kind, "visible");
    overlay.setAttribute("role", state.kind === "error" ? "alert" : "status");
    overlay.setAttribute("aria-live", state.kind === "error" ? "assertive" : "polite");
    const message = document.createElement("div");
    message.className = "overlay-message";
    if (state.kind === "error") {
      const icon = document.createElement("span");
      icon.className = "overlay-error-icon";
      icon.textContent = "!";
      message.appendChild(icon);
    }
    message.appendChild(document.createTextNode(state.text));
    overlay.appendChild(message);
    if (state.kind === "error" && state.connectFusion) {
      const connect = document.createElement("button");
      connect.type = "button";
      connect.className = "btn";
      connect.textContent = "Connect Fusion";
      connect.addEventListener("click", state.connectFusion);
      overlay.appendChild(connect);
    }
    if (state.kind === "error" && state.retry) {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "btn";
      retry.textContent = "Try Again";
      retry.addEventListener("click", state.retry);
      overlay.appendChild(retry);
    }
  }

  const viewport = new Viewport(viewportEl);
  const hud = new Hud(main); // appends the strip after viewportWrap
  const plateSession = new PlateSession(modelGeometry);
  const plateControls = new PlateControls();
  const colorFeature = extensions.createColorFeature();
  colorFeature.setEntitlements?.(activeEntitlements);
  libraryPanel?.setLocked?.(!activeEntitlements.library);
  let colorwayModeRevision = 0;
  viewportWrap.append(plateControls.root, plateControls.warning, colorFeature.root);
  colorFeature.onChange = () => viewport.setColorway(colorFeature.mode, colorFeature.resolveColor);
  colorFeature.onChange();
  let renderedPlateView = "";
  let renderedOverviewGeneration: number | null = null;
  plateSession.onChange = () => {
    const view = plateSession.view;
    plateControls.update(view, plateSession.busy);
    if (!view) {
      renderedPlateView = "";
      renderedOverviewGeneration = null;
      viewport.loadMesh(null);
      hud.hide();
      return;
    }
    colorFeature.selectPlate(view.plate.id);
    const { summary, plate, overview } = view;
    const key = `${summary.generation}:${plate.id}:${overview}`;
    if (key !== renderedPlateView) {
      if (overview) {
        if (renderedOverviewGeneration === summary.generation) viewport.setSelectedPlate(plate.id);
        else viewport.loadPlates(view.plates, plate.id);
        renderedOverviewGeneration = summary.generation;
      } else {
        viewport.loadMesh(view.plates[0]);
        renderedOverviewGeneration = null;
      }
      renderedPlateView = key;
    }
    hud.show({ ...summary, triangle_count: plate.triangle_count, geometry: plate.geometry });
    if (view.integrity) hud.applyIntegrityReport(view.integrity);
    setOverlay(
      !overview && !plate.geometry
        ? { kind: "hint", text: "No printable geometry on this plate." }
        : null,
    );
  };
  function changePlateView(change: Promise<void>) {
    void change.catch((e: unknown) =>
      plateControls.showError(isAppError(e) ? describeError(e) : String(e)),
    );
  }
  plateControls.onSelect = (id) => changePlateView(plateSession.select(id));
  plateControls.onOverview = (overview) => changePlateView(plateSession.setOverview(overview));
  viewport.onPlateSelect = (id) => changePlateView(plateSession.select(id));

  // Settings page (cmd-,). State it edits lives in the same variables the rest
  // of main.ts already uses, so a change here is applied exactly the way the
  // HUD picker / handoff button would apply it.
  const settingsPanel = new SettingsPanel(main);
  settingsPanel.addExtensionSection(extensions.createSettingsSection?.() ?? null);
  let defaultFolder: string | null = null;
  let displayUnits: "mm" | "in" = "mm";
  let appearanceMode: AppearanceMode = "system";
  let appearanceRevision = 0;
  let appearanceSave = Promise.resolve();
  let activePrinterId = "";
  let accentColor: string | null = null;
  let useAccentModelColor = localStorage.getItem("useAccentModelColor") !== "false";
  let defaultColorPreview = true;
  let defaultColorPreviewRevision = 0;
  let defaultColorPreviewSave = Promise.resolve();
  let accentSave = Promise.resolve();
  let accentRevision = 0;
  viewport.setUseAccentModelColor(useAccentModelColor);

  const gearButton = document.createElement("button");
  gearButton.className = "btn icon";
  gearButton.title = "Settings (⌘,)";
  gearButton.setAttribute("aria-label", "Settings");
  gearButton.innerHTML =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" ' +
    'stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/>' +
    '<path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>';
  gearButton.addEventListener("click", () => openSettings(gearButton));
  tree.addFooterControl(gearButton);
  mountRecoveryControl(app, (control) => tree.addFooterControl(control));
  extensions.mountLicenseControl?.(app, (control) => tree.addFooterControl(control));

  if (libraryPanel) {
    const libraryButton = document.createElement("button");
    libraryButton.className = "btn";
    libraryButton.textContent = "Library";
    libraryButton.setAttribute("aria-label", "Open managed library");
    libraryButton.addEventListener("click", () => {
      libraryPanel.show(libraryButton);
      void refreshLibrary();
    });
    tree.addFooterControl(libraryButton);
  }

  function refreshSettingsPanel() {
    if (!settingsPanel.isOpen) return;
    settingsPanel.render({
      defaultFolder,
      printers,
      activePrinterId,
      slicers,
      defaultSlicerId: activeSlicerId,
      units: displayUnits,
      appearanceMode,
      accentColor,
      useAccentModelColor,
      defaultColorPreview,
    });
  }

  function openSettings(invoker?: HTMLElement) {
    settingsPanel.show(
      {
        defaultFolder,
        printers,
        activePrinterId,
        slicers,
        defaultSlicerId: activeSlicerId,
        units: displayUnits,
        appearanceMode,
        accentColor,
        useAccentModelColor,
        defaultColorPreview,
      },
      invoker,
    );
  }
  settingsPanel.onChooseFolder = () => tree.onPickFolder();
  settingsPanel.onClearFolder = () => {
    defaultFolder = null;
    void setDefaultFolder(null);
    refreshSettingsPanel();
  };
  settingsPanel.onPrinter = (id) => applyPrinter(id);
  settingsPanel.onSlicer = (id) => {
    activeSlicerId = id;
    void setDefaultSlicer(id);
    void refreshColorwayPalette();
    updateHandoffButton();
  };
  settingsPanel.onUnits = (u) => {
    displayUnits = u;
    hud.setUnits(u);
    void setDisplayUnits(u);
  };
  settingsPanel.onAppearanceMode = (mode) => {
    appearanceMode = mode;
    applyTheme(mode);
    settingsPanel.setAppearanceModeError("");
    const revision = ++appearanceRevision;
    appearanceSave = appearanceSave
      .then(() => setAppearanceMode(mode))
      .catch(() => {
        if (revision === appearanceRevision) {
          settingsPanel.setAppearanceModeError(
            "Could not save appearance. Choose it again to retry.",
          );
        }
      });
  };
  settingsPanel.onAccent = (color) => {
    accentColor = color;
    applyAccent(color);
    settingsPanel.setAccent(color);
    settingsPanel.setAccentError("");
    const revision = ++accentRevision;
    // Serialize writes so a slow earlier choice cannot overwrite the latest one.
    accentSave = accentSave
      .then(() => setAccentColor(color))
      .catch(() => {
        if (revision === accentRevision) {
          settingsPanel.setAccentError("Could not save accent color. Choose it again to retry.");
        }
      });
  };
  settingsPanel.onUseAccentModelColor = (enabled) => {
    useAccentModelColor = enabled;
    localStorage.setItem("useAccentModelColor", String(enabled));
    viewport.setUseAccentModelColor(enabled);
  };
  settingsPanel.onDefaultColorPreview = (enabled) => {
    defaultColorPreview = enabled;
    colorwayModeRevision++;
    colorFeature.setMode(enabled ? "color" : "model");
    if (enabled) void refreshColorwayPalette();
    settingsPanel.setColorPreviewError("");
    const revision = ++defaultColorPreviewRevision;
    // Save in order so a quick second toggle remains the persisted choice.
    defaultColorPreviewSave = defaultColorPreviewSave
      .then(() => setDefaultColorPreview(enabled))
      .catch(() => {
        if (revision === defaultColorPreviewRevision) {
          settingsPanel.setColorPreviewError(
            "Could not save this default. Change it again to retry.",
          );
        }
      });
  };

  function applyPrinter(id: string) {
    activePrinterId = id;
    void setActivePrinter(id);
    const active = printers.find((p) => p.id === id);
    if (active) viewport.setBed(active.bed_mm);
    hud.setPrinters(printers, id);
  }

  let slicers: SlicerApp[] = [];
  let activeSlicerId: string | null = null;
  let loadedGeneration: number | null = null; // the generation currently on the viewport, if any
  let loadedSourcePath: string | null = null;
  let loadingSourcePath: string | null = null;
  let loadedFormat: ModelSummary["format"] | null = null;
  let colorwayPaletteRevision = 0;
  async function refreshColorwayPalette() {
    const revision = ++colorwayPaletteRevision;
    const slicerId = activeSlicerId;
    const getPalette = extensions.getSlicerPalette;
    if (
      !getPalette ||
      !colorFeature.setActivePalette ||
      (colorFeature.canUseActivePalette && !colorFeature.canUseActivePalette())
    ) {
      return;
    }
    if (!slicerId) {
      colorFeature.setActivePalette(null);
      return;
    }
    const palette = await getPalette(slicerId).catch(() => null);
    if (revision !== colorwayPaletteRevision || !colorFeature.canUseActivePalette?.()) return;
    colorFeature.setActivePalette(palette);
  }

  function updateHandoffButton() {
    const active = slicers.find((s) => s.id === activeSlicerId);
    const availability = handoffAvailability({
      hasAction: typeof extensions.requestHandoff === "function",
      hasSlicer: active !== undefined,
      hasModel: loadedGeneration !== null,
      format: loadedFormat,
      entitled: activeEntitlements.slicerHandoff,
    });
    hud.setHandoff(
      availability === "hidden" ? null : active!.display_name,
      availability === "locked",
    );
  }

  /** Window title: "bracket_v3.stl — GoggleLab" while a file is open. */
  function setWindowTitle(fileName: string | null) {
    const title = fileName ? `${fileName} — ${APP_NAME}` : APP_NAME;
    document.title = title;
    void getCurrentWindow()
      .setTitle(title)
      .catch(() => {});
  }

  async function initSlicers() {
    slicers = await detectSlicers();
    const saved = await getDefaultSlicer();
    activeSlicerId =
      (saved && slicers.some((s) => s.id === saved) ? saved : slicers[0]?.id) ?? null;
    if (activeSlicerId && activeSlicerId !== saved) {
      void setDefaultSlicer(activeSlicerId);
    }
    updateHandoffButton();
    void refreshColorwayPalette();
  }
  initSlicers();

  // Printer list = detected from installed slicers (refreshed every launch,
  // read-only) + manual entries from settings.json. Active printer: whatever
  // was chosen last if it still exists, else the detected printer from the
  // default slicer, else the first one, else a generic 256mm bed so bed-fit
  // always has *something* to evaluate against.
  let printers: Printer[] = [];
  async function buildPrinterList(): Promise<{ list: Printer[]; activeId: string }> {
    const [settings, detected, defaultSlicer] = await Promise.all([
      getSettings(),
      detectPrinters().catch(() => []),
      getDefaultSlicerId(),
    ]);
    const list: Printer[] = [
      ...detected.map((d) => ({
        id: d.id,
        name: `${d.name} · ${d.source_display}`,
        bed_mm: d.bed_mm,
      })),
      ...settings.printers,
    ];
    if (list.length === 0) {
      list.push({ id: "generic-256", name: "Generic 256 mm bed", bed_mm: [256, 256, 256] });
    }
    const fromDefaultSlicer = detected.find((d) => d.source_slicer === defaultSlicer)?.id;
    const activeId = list.some((p) => p.id === settings.active_printer)
      ? settings.active_printer
      : (fromDefaultSlicer ?? list[0].id);
    return { list, activeId };
  }

  async function initSettings() {
    const settings = await getSettings();
    defaultFolder = settings.default_folder;
    displayUnits = settings.display_units;
    if (appearanceRevision === 0) {
      appearanceMode = settings.appearance_mode;
      applyTheme(appearanceMode);
    }
    if (defaultColorPreviewRevision === 0) {
      defaultColorPreview = settings.default_color_preview;
    }
    if (colorwayModeRevision === 0) {
      colorFeature.setMode(defaultColorPreview ? "color" : "model");
    }
    refreshSettingsPanel();
    if (accentRevision === 0) {
      accentColor = settings.accent_color ?? null;
      applyAccent(accentColor);
    }
    hud.setUnits(displayUnits);
    const { list, activeId } = await buildPrinterList();
    printers = list;
    activePrinterId = activeId;
    hud.setPrinters(printers, activeId);
    const active = printers.find((p) => p.id === activeId);
    if (active) viewport.setBed(active.bed_mm);
    if (activeId !== settings.active_printer) void setActivePrinter(activeId);
  }
  initSettings();

  hud.onUnitsToggle = (units) => {
    displayUnits = units;
    void setDisplayUnits(units);
  };
  hud.onPrinterChange = (printerId) => applyPrinter(printerId);

  // The session buffers reports by file generation and plate, including reports
  // that beat the summary or geometry response to the frontend.
  listen<IntegrityReport>("integrity", (event) => {
    plateSession.receiveIntegrity(event.payload);
  });

  async function handoff() {
    const active = slicers.find((s) => s.id === activeSlicerId);
    const availability = handoffAvailability({
      hasAction: typeof extensions.requestHandoff === "function",
      hasSlicer: active !== undefined,
      hasModel: loadedGeneration !== null,
      format: loadedFormat,
      entitled: activeEntitlements.slicerHandoff,
    });
    if (availability === "hidden") return;
    if (availability === "locked") {
      extensions.onHandoffLocked?.();
      return;
    }
    const generation = loadedGeneration!;
    const slicerId = activeSlicerId!;
    try {
      await extensions.requestHandoff!(generation, slicerId);
    } catch (e) {
      if (isAppError(e) && e.kind !== "Superseded") {
        setOverlay({ kind: "error", text: describeError(e) });
      }
    }
  }
  hud.onHandoff = handoff;

  // Sidebar: the root follows whatever file was last opened (its parent
  // folder), or a folder picked explicitly. Both are OS-supplied paths, which
  // is what lets list_dir refuse anything outside them.
  tree.onOpenFile = (path) => loadModel(path);
  tree.onTrashFile = async (path) => {
    await moveTreeFileToTrash(path);
    if (path !== loadedSourcePath && path !== loadingSourcePath) return;
    const gen = ++generation;
    stepController?.abort();
    stepController = null;
    loadingSourcePath = null;
    loadedSourcePath = null;
    loadedGeneration = null;
    loadedFormat = null;
    plateSession.reset(gen);
    colorFeature.reset(gen);
    clearProgressTimer();
    hud.hide();
    setWindowTitle(null);
    updateHandoffButton();
    setOverlay({ kind: "hint", text: "Model moved to Trash." });
  };
  tree.onRootChanged = () => hideFolderModal();
  tree.onPickFolder = () => {
    openDialog({ directory: true, multiple: false }).then((dir) => {
      if (typeof dir !== "string") return;
      defaultFolder = dir;
      void setDefaultFolder(dir);
      void tree.setRoot(dir);
      refreshSettingsPanel();
    });
  };

  // The one handle for "which load is current" (spec §2, R-01/R-02). Every
  // entry point below increments this and passes its own value through; the
  // backend is the actual arbiter of staleness, this is just the frontend's
  // half of the contract (defence in depth, not the mechanism).
  let generation = 0;
  let pendingLibraryLoad: LibraryRequestToken | null = null;
  let libraryRefreshSequence = 0;
  let activeLibraryRefresh: LibraryRequestToken | null = null;
  let progressTimer: number | undefined;
  let stepController: AbortController | null = null;

  function cancelCadPreview() {
    stepController?.abort();
    stepController = null;
  }

  function clearProgressTimer() {
    if (progressTimer !== undefined) {
      window.clearTimeout(progressTimer);
      progressTimer = undefined;
    }
  }

  function invalidatePendingLibraryLoad() {
    const pending = pendingLibraryLoad;
    if (!pending || pending.generation !== generation) return;
    pendingLibraryLoad = null;
    const gen = ++generation;
    stepController?.abort();
    stepController = null;
    loadingSourcePath = null;
    loadedSourcePath = null;
    loadedGeneration = null;
    loadedFormat = null;
    plateSession.reset(gen);
    colorFeature.reset(gen);
    clearProgressTimer();
    hud.hide();
    setWindowTitle(null);
    updateHandoffButton();
    setOverlay({ kind: "hint", text: "Library access changed while the model was loading." });
  }

  function scheduleProgress(text: string, gen: number) {
    clearProgressTimer();
    progressTimer = window.setTimeout(() => {
      progressTimer = undefined;
      if (gen === generation) setOverlay({ kind: "progress", text });
    }, 150);
  }

  async function showSummary(summary: ModelSummary, gen: number) {
    if (gen !== generation) return;
    await plateSession.load(summary);
    if (gen !== generation) return;
    clearProgressTimer();
    colorFeature.load(summary);
    void refreshColorwayPalette();
    setWindowTitle(summary.file_name);
    loadedGeneration = gen;
    loadedFormat = summary.format;
    updateHandoffButton();
  }

  async function showCadPreview(path: string, gen: number) {
    const start = performance.now();
    const format = /\.f3d$/i.test(path) ? "f3d" : "step";
    const label = cadPreviewLabel(format);
    const controller = new AbortController();
    stepController = controller;
    const fusionSource = format === "f3d" ? await readFusionFile(path, controller.signal) : null;
    const bytes = fusionSource ? fusionSource.bytes : await readStepFile(path, gen);
    if (gen !== generation) return;
    const byteLength = fusionSource?.sourceSize ?? bytes.byteLength;
    scheduleProgress(
      format === "f3d" ? "Preparing Fusion preview…" : "Preparing STEP preview…",
      gen,
    );
    const preview = await previewStep(bytes, controller.signal);
    if (gen !== generation) return;
    stepController = null;
    const fileName = path.split("/").pop() ?? path;
    const summary: ModelSummary = {
      generation: gen,
      file_name: fileName,
      file_size_bytes: byteLength,
      format,
      triangle_count: preview.geometry.triangleCount,
      geometry: preview.stats,
      parse_ms: Math.round(performance.now() - start),
      plates: [
        {
          id: 0,
          name: label,
          triangle_count: preview.geometry.triangleCount,
          geometry: preview.stats,
          parts: [],
        },
      ],
      materials: [],
      embedded_filaments: [],
      part_warning: null,
      plate_metadata: false,
      plate_warning: null,
      source_identity: `path:${path}`,
      source_digest: fusionSource?.sourceDigest ?? preview.digest,
    };
    viewport.loadMesh({
      id: 0,
      name: label,
      geometry: preview.geometry,
      stats: preview.stats,
      parts: [],
    });
    hud.show(summary);
    colorFeature.load(summary);
    void refreshColorwayPalette();
    clearProgressTimer();
    setOverlay(null);
    setWindowTitle(fileName);
    loadedGeneration = gen;
    loadedFormat = format;
    loadedSourcePath = path;
    updateHandoffButton();
  }

  async function connectFusionForPreview(path: string, gen: number) {
    if (gen !== generation) return;
    setOverlay({ kind: "progress", text: "Installing the Fusion helper…" });
    try {
      await installFusionBridge();
      if (gen !== generation) return;
      setOverlay({
        kind: "error",
        text: "Helper installed. Save your open Fusion designs and restart Fusion. Press Shift+S, select GoggleLabFusionBridge and enable Run and Run on Startup, then return here and try again.",
        retry: () => void loadModel(path),
      });
    } catch (error) {
      if (gen !== generation) return;
      setOverlay({
        kind: "error",
        text: isAppError(error) ? describeError(error) : String(error),
        connectFusion: () => void connectFusionForPreview(path, gen),
        retry: () => void loadModel(path),
      });
    }
  }

  async function loadModel(path: string) {
    const gen = ++generation;
    pendingLibraryLoad = null;
    cancelCadPreview();
    loadedGeneration = null;
    loadedSourcePath = null;
    loadingSourcePath = path;
    loadedFormat = null;
    colorFeature.reset(gen);
    plateSession.reset(gen);
    void tree.fileOpened(path);
    hud.hide();
    updateHandoffButton();
    setOverlay(null);
    scheduleProgress(/\.f3d$/i.test(path) ? "Converting in Fusion…" : "Loading…", gen);

    try {
      if (/\.(step|stp|f3d)$/i.test(path)) {
        await showCadPreview(path, gen);
      } else {
        const summary = await openModel(path, gen);
        await showSummary(summary, gen);
        if (gen === generation && loadedGeneration === gen) loadedSourcePath = path;
      }
      if (gen === generation) loadingSourcePath = null;
    } catch (e) {
      if (gen !== generation) return; // a superseded load's own error is not ours to show
      loadingSourcePath = null;
      if (isAppError(e) && e.kind === "Superseded") return;
      clearProgressTimer();
      const filename = path.split("/").pop() ?? path;
      const detail = isAppError(e) ? describeError(e) : String(e);
      setOverlay({
        kind: "error",
        text: `Could not open ${filename}.\n${detail}`,
        retry: () => void loadModel(path),
        connectFusion:
          isAppError(e) && e.kind === "Fusion" && e.setup_required
            ? () => void connectFusionForPreview(path, gen)
            : undefined,
      });
      cancelCadPreview();
      loadedGeneration = null;
      loadedFormat = null;
      setWindowTitle(null);
      updateHandoffButton();
    }
  }

  async function loadLibraryModel(modelId: string) {
    if (
      !libraryPanel ||
      !libraryService ||
      !activeEntitlements.library ||
      !libraryService.isEnabled()
    )
      return;
    const gen = ++generation;
    const entitlementRevision = activeEntitlements.revision;
    const request: LibraryRequestToken = { generation: gen, entitlementRevision };
    pendingLibraryLoad = request;
    stepController?.abort();
    stepController = null;
    loadedGeneration = null;
    loadedSourcePath = null;
    loadingSourcePath = null;
    loadedFormat = null;
    colorFeature.reset(gen);
    plateSession.reset(gen);
    updateHandoffButton();
    setOverlay(null);
    scheduleProgress("Loading from Library…", gen);
    try {
      const summary = await libraryService.openModel(modelId, gen);
      if (
        !isLibraryRequestCurrent(request, pendingLibraryLoad, {
          generation,
          entitlementRevision: activeEntitlements.revision,
          entitled: activeEntitlements.library && libraryService.isEnabled(),
        })
      ) {
        return;
      }
      await showSummary(summary, gen);
    } catch (e) {
      if (gen !== generation) return;
      if (isAppError(e) && e.kind === "Superseded") return;
      clearProgressTimer();
      setOverlay({
        kind: "error",
        text: isAppError(e) ? describeError(e) : String(e),
        retry: () => void loadLibraryModel(modelId),
      });
      loadedGeneration = null;
      loadedFormat = null;
      setWindowTitle(null);
      updateHandoffButton();
    } finally {
      if (pendingLibraryLoad?.generation === gen) pendingLibraryLoad = null;
    }
  }

  async function refreshLibrary() {
    if (!libraryPanel || !libraryService) return;
    const request: LibraryRequestToken = {
      generation: ++libraryRefreshSequence,
      entitlementRevision: activeEntitlements.revision,
    };
    activeLibraryRefresh = request;
    if (!activeEntitlements.library || !libraryService.isEnabled()) {
      libraryPanel.setLocked?.(true);
      libraryPanel.setStatus(
        "Import a valid Pro license to use the managed Library. Your existing data stays on this device.",
      );
      return;
    }
    libraryPanel.setLocked?.(false);
    try {
      const [folders, models] = await Promise.all([
        libraryService.listFolders(),
        libraryService.listModels(),
      ]);
      if (
        !isLibraryRequestCurrent(request, activeLibraryRefresh, {
          generation: libraryRefreshSequence,
          entitlementRevision: activeEntitlements.revision,
          entitled: activeEntitlements.library && libraryService.isEnabled(),
        })
      ) {
        return;
      }
      libraryPanel.render(folders, models);
      libraryPanel.setStatus("Managed copies are stored locally and survive source-file changes.");
    } catch (e) {
      if (
        !isLibraryRequestCurrent(request, activeLibraryRefresh, {
          generation: libraryRefreshSequence,
          entitlementRevision: activeEntitlements.revision,
          entitled: activeEntitlements.library && libraryService.isEnabled(),
        })
      ) {
        return;
      }
      libraryPanel.setStatus(isAppError(e) ? describeError(e) : String(e));
    } finally {
      if (activeLibraryRefresh === request) activeLibraryRefresh = null;
    }
  }

  async function importIntoLibrary() {
    if (!libraryPanel || !libraryService || !libraryService.isEnabled()) return;
    const entitlementRevision = activeEntitlements.revision;
    if (!activeEntitlements.library) return;
    const selected = await openDialog({
      multiple: true,
      filters: [{ name: "3D models", extensions: ["stl", "3mf"] }],
    });
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
    if (
      paths.length === 0 ||
      entitlementRevision !== activeEntitlements.revision ||
      !activeEntitlements.library ||
      !libraryService.isEnabled()
    ) {
      return;
    }
    libraryPanel.setStatus(`Importing ${paths.length} model${paths.length === 1 ? "" : "s"}…`);
    let imported = 0;
    const failures: string[] = [];
    for (const path of paths) {
      if (
        entitlementRevision !== activeEntitlements.revision ||
        !activeEntitlements.library ||
        !libraryService.isEnabled()
      ) {
        return;
      }
      try {
        await libraryService.importFile(path);
        imported += 1;
      } catch (e) {
        const filename = path.split("/").pop() ?? path;
        const detail = isAppError(e) ? describeError(e) : String(e);
        failures.push(`${filename}: ${detail}`);
      }
    }
    await refreshLibrary();
    if (
      entitlementRevision !== activeEntitlements.revision ||
      !activeEntitlements.library ||
      !libraryService.isEnabled()
    ) {
      return;
    }
    const summary = `${imported} imported · ${failures.length} failed`;
    libraryPanel.setStatus(failures.length ? `${summary}\n${failures.join("\n")}` : `${summary}.`);
  }

  if (libraryPanel) {
    libraryPanel.onImport = () => void importIntoLibrary();
    libraryPanel.onOpenModel = (modelId) => {
      libraryPanel.hide();
      void loadLibraryModel(modelId);
    };
  }

  const unsubscribeEntitlements =
    extensions.subscribeEntitlements?.((entitlements) => {
      const pending = pendingLibraryLoad;
      activeEntitlements = entitlements;
      updateHandoffButton();
      colorwayPaletteRevision += 1;
      colorFeature.setEntitlements?.(entitlements);
      if (!entitlements.library) activeLibraryRefresh = null;
      if (
        pending &&
        (!entitlements.library || pending.entitlementRevision !== entitlements.revision)
      ) {
        invalidatePendingLibraryLoad();
      }
      libraryPanel?.setLocked?.(!entitlements.library);
      if (entitlements.colorways) void refreshColorwayPalette();
      if (entitlements.library) void refreshLibrary();
      else if (libraryPanel) {
        libraryPanel.setStatus(
          "Import a valid Pro license to use the managed Library. Your existing data stays on this device.",
        );
      }
    }) ?? (() => {});
  window.addEventListener("beforeunload", unsubscribeEntitlements, { once: true });
  type TopmostOverlay = "none" | "settings" | "library" | "coldStart";

  function topmostOverlay(): TopmostOverlay {
    if (settingsPanel.isOpen) return "settings";
    if (libraryPanel?.isOpen) return "library";
    if (folderModal.open) return "coldStart";
    return "none";
  }
  function closeTopmostOverlay(overlayName: Exclude<TopmostOverlay, "none">) {
    if (overlayName === "settings") settingsPanel.hide();
    else if (overlayName === "library") libraryPanel?.hide();
    else hideFolderModal();
  }

  // One dispatcher owns app shortcuts. Native dialog modality and form controls
  // get first refusal so menu navigation never becomes model navigation.
  window.addEventListener("keydown", (e) => {
    const modal = topmostOverlay();
    if (modal !== "none") {
      if (e.key === "Escape") {
        e.preventDefault();
        closeTopmostOverlay(modal);
      }
      return;
    }

    const mod = e.metaKey || e.ctrlKey;
    const key = e.key.toLowerCase();
    if (mod && e.shiftKey && key === "o") {
      e.preventDefault();
      tree.onPickFolder();
    } else if (mod && key === ",") {
      e.preventDefault();
      openSettings(gearButton);
    } else if (mod && key === "f") {
      e.preventDefault();
      tree.setVisible(true);
      tree.focusSearch();
    } else if (mod && key === "b") {
      e.preventDefault();
      tree.setVisible(!tree.isVisible);
    } else if (mod && key === "enter") {
      e.preventDefault();
      void handoff();
    } else if (mod && key === "o") {
      e.preventDefault();
      void openFileDialog();
    }
  });

  // 2. Drag-drop -- native OS drag events targeting the window, not HTML5 DOM
  // drag-and-drop; the native event reports enter/over/leave/drop states.
  let dragActive = false;
  getCurrentWindow().onDragDropEvent((event: { payload: DragDropEvent }) => {
    const { payload } = event;
    if (payload.type === "enter" || payload.type === "over") {
      if (!dragActive) {
        dragActive = true;
        viewportWrap.classList.add("drag-over");
        setOverlay({ kind: "hint", text: "Drop to open" });
      }
    } else if (payload.type === "leave") {
      dragActive = false;
      viewportWrap.classList.remove("drag-over");
      setOverlay(null);
    } else if (payload.type === "drop") {
      dragActive = false;
      viewportWrap.classList.remove("drag-over");
      if (payload.paths.length > 0) void loadModel(payload.paths[0]);
      else setOverlay(null);
    }
  });

  // 3. Finder "Open With" / double-click -- warm case: the app is already
  // running and the OS delivers RunEvent::Opened directly, which the backend
  // re-emits as "opened".
  listen<string[]>("opened", (event) => {
    if (event.payload.length > 0) void loadModel(event.payload[0]);
  });

  // 4. Cold start: CLI arg, and Finder-open URLs that arrived before this
  // listener existed. Both are drained once at boot.
  async function bootFromColdStart() {
    const openedPaths = await invoke<string[]>("opened_urls");
    if (openedPaths.length > 0) {
      await loadModel(openedPaths[0]);
      return;
    }
    const cliPath = await invoke<string | null>("cli_arg_path");
    if (cliPath) {
      await loadModel(cliPath);
      return;
    }
    setOverlay({ kind: "hint", text: "Pick a file from the sidebar, drop one here, or press ⌘O." });
    const settings = await getSettings();
    // A remembered folder may have been moved or deleted since last time.
    const remembered = settings.default_folder;
    const stillExists =
      remembered !== null &&
      (await listDir(remembered, remembered).then(
        () => true,
        () => false,
      ));
    if (remembered && stillExists) {
      await tree.setRoot(remembered);
    } else {
      showFolderModal();
    }
  }
  bootFromColdStart();
}
