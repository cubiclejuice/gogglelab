// Readout strip (spec §4, S8; visual spec design/Main.dc.html, direction C).
// A 48px strip under the viewport: dimensions with a mm/in switch, volume,
// triangle count, a bed-fit badge that doubles as the printer picker, an
// integrity badge that is visibly pending until the `integrity` event lands
// (R-08), and the slicer handoff button.

import type { ModelSummary, Integrity, IntegrityReport, Printer } from "./ipc";
import { cadPreviewLabel, isCadPreview } from "./cad-format";

const MM_PER_INCH = 25.4;

function formatLength(mm: number, units: "mm" | "in"): string {
  const value = units === "mm" ? mm : mm / MM_PER_INCH;
  const decimals = units === "mm" ? 1 : 2;
  return `${value.toFixed(decimals)}${units}`;
}

function formatNumber(mm: number, units: "mm" | "in"): string {
  const value = units === "mm" ? mm : mm / MM_PER_INCH;
  return value.toFixed(units === "mm" ? 1 : 2);
}

export interface BedFit {
  fits: boolean;
  exceedingAxes: Array<{ axis: "X" | "Y" | "Z"; byMm: number }>;
}

export function checkBedFit(
  dimensions: [number, number, number],
  bedMm: [number, number, number],
): BedFit {
  const axes: Array<"X" | "Y" | "Z"> = ["X", "Y", "Z"];
  const exceedingAxes = axes
    .map((axis, i) => ({ axis, byMm: dimensions[i] - bedMm[i] }))
    .filter((a) => a.byMm > 0);
  return { fits: exceedingAxes.length === 0, exceedingAxes };
}

const ICON_CHECK =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" ' +
  'stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M8 12.5l2.5 2.5L16 9.5"/></svg>';
const ICON_WARN =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" ' +
  'stroke-linecap="round" stroke-linejoin="round"><path d="M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>' +
  '<line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>';
const ICON_BAD =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" ' +
  'stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M9 9l6 6M15 9l-6 6"/></svg>';
const ICON_PENDING =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" ' +
  'stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>';
const ICON_CHEVRON =
  '<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" ' +
  'stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>';

type BadgeTone = "ok" | "warn" | "bad" | "neutral";

function setBadge(
  el: HTMLElement,
  tone: BadgeTone,
  icon: string,
  text: string,
  sub?: string,
  trailing = "",
) {
  el.classList.remove("ok", "warn", "bad");
  if (tone !== "neutral") el.classList.add(tone);
  el.innerHTML =
    `${icon}<span>${escapeHtml(text)}</span>` +
    (sub ? `<span class="sub">· ${escapeHtml(sub)}</span>` : "") +
    trailing;
}

function escapeHtml(s: string): string {
  return s.replace(
    /[&<>"]/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!,
  );
}

/** "Bambu Lab X1 Carbon · Bambu Studio" → "Bambu Lab X1 Carbon". */
function shortPrinterName(name: string): string {
  const i = name.indexOf(" · ");
  return i > 0 ? name.slice(0, i) : name;
}

export class Hud {
  private root: HTMLElement;
  private dimsEl: HTMLElement;
  private volumeEl: HTMLElement;
  private triCountEl: HTMLElement;
  private bedFitEl: HTMLElement;
  private bedFitLabel: HTMLElement;
  private integrityEl: HTMLElement;
  private unitButtons: Record<"mm" | "in", HTMLButtonElement>;
  private printerSelect: HTMLSelectElement;
  private handoffButton: HTMLButtonElement;

  private units: "mm" | "in" = "mm";
  private currentSummary: ModelSummary | null = null;
  private currentBedMm: [number, number, number] = [256, 256, 256];
  private currentPrinterName = "";
  private currentIntegrity: Integrity | "pending" | "skipped" | null = null;

  onUnitsToggle: (units: "mm" | "in") => void = () => {};
  onPrinterChange: (printerId: string) => void = () => {};
  onHandoff: () => void = () => {};

  constructor(container: HTMLElement) {
    this.root = document.createElement("div");
    this.root.className = "strip";
    container.appendChild(this.root);

    // Dimensions: label + mm/in segmented switch over the big mono value.
    const dims = document.createElement("div");
    dims.className = "readout";
    const dimsHead = document.createElement("div");
    dimsHead.className = "readout-head";
    dimsHead.innerHTML = '<span class="readout-label">Dimensions</span>';
    const seg = document.createElement("div");
    seg.className = "seg";
    seg.setAttribute("role", "group");
    seg.setAttribute("aria-label", "Display units");
    const mkUnit = (u: "mm" | "in") => {
      const b = document.createElement("button");
      b.textContent = u.toUpperCase();
      b.title = u === "mm" ? "Millimetres" : "Inches (display only)";
      b.addEventListener("click", () => {
        if (this.units === u) return;
        this.units = u;
        this.onUnitsToggle(u);
        this.render();
      });
      seg.appendChild(b);
      return b;
    };
    this.unitButtons = { mm: mkUnit("mm"), in: mkUnit("in") };
    dimsHead.appendChild(seg);
    this.dimsEl = document.createElement("div");
    this.dimsEl.className = "readout-value big";
    dims.append(dimsHead, this.dimsEl);

    const readout = (label: string) => {
      const r = document.createElement("div");
      r.className = "readout";
      r.innerHTML = `<span class="readout-label">${label}</span>`;
      const v = document.createElement("div");
      v.className = "readout-value";
      r.appendChild(v);
      return { r, v };
    };
    const volume = readout("Volume");
    this.volumeEl = volume.v;
    const tris = readout("Triangles");
    this.triCountEl = tris.v;
    // Volume and triangle count are the first things dropped when the strip
    // gets narrow (styles.css container queries); bed fit and handoff stay.
    volume.r.classList.add("secondary");
    tris.r.classList.add("secondary");
    const secondaryDivider = divider();
    secondaryDivider.classList.add("secondary");

    // Bed fit badge is also the printer picker: a transparent <select> sits
    // over it so clicking anywhere on the badge opens the native menu.
    this.bedFitEl = document.createElement("div");
    this.bedFitEl.className = "badge picker";
    this.bedFitEl.setAttribute("aria-live", "polite");
    this.bedFitLabel = document.createElement("span");
    this.bedFitLabel.className = "bed-fit-label";
    this.printerSelect = document.createElement("select");
    this.printerSelect.setAttribute("aria-label", "Active printer");
    this.printerSelect.addEventListener("change", () => {
      this.onPrinterChange(this.printerSelect.value);
    });
    this.bedFitEl.append(this.bedFitLabel, this.printerSelect);

    this.integrityEl = document.createElement("div");
    this.integrityEl.className = "badge integrity";
    this.integrityEl.setAttribute("aria-live", "polite");
    const spacer = document.createElement("div");
    spacer.className = "strip-spacer";

    this.handoffButton = document.createElement("button");
    this.handoffButton.className = "handoff";
    this.handoffButton.hidden = true;
    this.handoffButton.addEventListener("click", () => this.onHandoff());

    this.root.append(
      dims,
      secondaryDivider,
      volume.r,
      tris.r,
      divider(),
      this.bedFitEl,
      this.integrityEl,
      spacer,
      this.handoffButton,
    );
    this.render();
  }

  setPrinters(printers: Printer[], activeId: string) {
    this.printerSelect.innerHTML = "";
    for (const p of printers) {
      const opt = document.createElement("option");
      opt.value = p.id;
      opt.textContent = p.name;
      this.printerSelect.appendChild(opt);
    }
    this.printerSelect.value = activeId;
    const active = printers.find((p) => p.id === activeId);
    if (active) {
      this.currentBedMm = active.bed_mm;
      this.currentPrinterName = shortPrinterName(active.name);
    }
    this.render();
  }

  setUnits(units: "mm" | "in") {
    this.units = units;
    this.render();
  }

  /** Label for the private handoff action, or null to hide it in Community. */
  setHandoff(slicerName: string | null, locked = false) {
    if (slicerName === null) {
      this.handoffButton.hidden = true;
      this.handoffButton.classList.remove("locked");
      this.handoffButton.removeAttribute("aria-label");
      return;
    }
    this.handoffButton.hidden = false;
    this.handoffButton.classList.toggle("locked", locked);
    // .long/.short: the container query in styles.css swaps to the short
    // label when the strip is too narrow for the slicer name.
    this.handoffButton.innerHTML =
      `<span class="long">Open in ${escapeHtml(slicerName)}${locked ? " · Pro" : ""}</span>` +
      `<span class="short">${locked ? "Pro" : "Open"}</span><span class="kbd">⌘⏎</span>`;
    this.handoffButton.setAttribute(
      "aria-label",
      locked ? `Unlock Pro to open in ${slicerName}` : `Open in ${slicerName}`,
    );
    this.handoffButton.title = locked
      ? `Open in ${slicerName} with Pro (⌘⏎)`
      : `Open in ${slicerName} (⌘⏎)`;
  }

  show(summary: ModelSummary) {
    this.currentSummary = summary;
    this.currentIntegrity = "pending";
    this.root.classList.remove("loading");
    this.render();
  }

  hide() {
    this.root.classList.add("loading");
    this.render();
  }

  applyIntegrityReport(report: IntegrityReport) {
    this.currentIntegrity = report.status === "Computed" ? report.integrity : "skipped";
    this.render();
  }

  private render() {
    this.unitButtons.mm.setAttribute("aria-pressed", String(this.units === "mm"));
    this.unitButtons.in.setAttribute("aria-pressed", String(this.units === "in"));

    const summary = this.currentSummary;
    const printerSub = this.currentPrinterName || undefined;
    if (!summary || !summary.geometry) {
      this.dimsEl.innerHTML = '<span class="x">—</span>';
      this.volumeEl.textContent = "—";
      this.volumeEl.className = "readout-value";
      this.volumeEl.title = "";
      this.triCountEl.textContent = summary ? summary.triangle_count.toLocaleString() : "—";
      setBadge(this.bedFitLabel, "neutral", "", "Bed", printerSub, ICON_CHEVRON);
      this.bedFitEl.classList.remove("ok", "warn", "bad");
      this.bedFitEl.title = "Active printer";
      this.printerSelect.setAttribute(
        "aria-label",
        `${this.currentPrinterName || "Active printer"} — no model`,
      );
      setBadge(this.integrityEl, "neutral", "", summary ? "No geometry" : "No model");
      this.integrityEl.title = "";
      return;
    }
    const geom = summary.geometry;

    const [x, y, z] = geom.dimensions.map((d) => formatNumber(d, this.units));
    this.dimsEl.innerHTML =
      `${x} <span class="x">×</span> ${y} <span class="x">×</span> ${z}` +
      ` <span class="unit">${this.units}</span>`;
    this.dimsEl.title = geom.dimensions.map((d) => formatLength(d, this.units)).join(" × ");
    this.triCountEl.textContent = summary.triangle_count.toLocaleString();

    if (isCadPreview(summary.format)) {
      const previewLabel = cadPreviewLabel(summary.format);
      this.volumeEl.textContent = "—";
      this.volumeEl.className = "readout-value";
      this.volumeEl.title = "Exact CAD volume is unavailable in a tessellated preview";
      this.bedFitEl.classList.remove("ok", "warn", "bad");
      setBadge(this.bedFitLabel, "neutral", "", "Preview", printerSub, ICON_CHEVRON);
      this.bedFitEl.title = `${previewLabel} dimensions are from the tessellated preview. Click to change printer.`;
      this.printerSelect.setAttribute("aria-label", `Active printer — ${previewLabel}`);
      setBadge(this.integrityEl, "neutral", "", previewLabel);
      this.integrityEl.title = "Exact CAD checks are unavailable in preview mode";
      return;
    }

    // Volume is provisional until integrity lands (R-05).
    const volumeHtml = `${(geom.volume_mm3 / 1000).toFixed(2)} <span class="unit">cm³</span>`;
    this.volumeEl.className = "readout-value";
    if (this.currentIntegrity === "pending" || this.currentIntegrity === null) {
      this.volumeEl.innerHTML = `≈ ${volumeHtml}`;
      this.volumeEl.classList.add("provisional");
      this.volumeEl.title = "Provisional until the mesh check finishes";
    } else if (this.currentIntegrity === "skipped") {
      this.volumeEl.innerHTML = `≈ ${volumeHtml}`;
      this.volumeEl.classList.add("provisional", "warn");
      this.volumeEl.title = "Model too large to verify — volume may be unreliable";
    } else if (
      this.currentIntegrity.watertight &&
      this.currentIntegrity.inconsistent_orientation === 0
    ) {
      this.volumeEl.innerHTML = volumeHtml;
      this.volumeEl.title = "";
    } else {
      this.volumeEl.innerHTML = volumeHtml;
      this.volumeEl.classList.add("unreliable", "warn");
      this.volumeEl.title = "Mesh is not closed — volume unreliable";
    }

    // Bed fit is always evaluated in mm regardless of display units (R-12).
    const fit = checkBedFit(geom.dimensions, this.currentBedMm);
    const bedDetails = fit.exceedingAxes
      .map((a) => `${a.axis} over by ${formatLength(a.byMm, this.units)}`)
      .join(", ");
    this.bedFitEl.classList.remove("ok", "warn", "bad");
    if (fit.fits) {
      setBadge(this.bedFitLabel, "neutral", ICON_CHECK, "Fits", printerSub, ICON_CHEVRON);
      this.bedFitEl.classList.add("ok");
      this.bedFitEl.title = `Fits the ${this.currentPrinterName || "active printer"} bed. Click to change printer.`;
      this.printerSelect.setAttribute(
        "aria-label",
        `${this.currentPrinterName || "Active printer"} — fits`,
      );
    } else {
      const worst = fit.exceedingAxes[0];
      setBadge(
        this.bedFitLabel,
        "neutral",
        ICON_BAD,
        `Exceeds ${worst.axis} by ${formatLength(worst.byMm, this.units)}`,
        printerSub,
        ICON_CHEVRON,
      );
      this.bedFitEl.classList.add("bad");
      this.bedFitEl.title = `${bedDetails}. Click to change printer.`;
      this.printerSelect.setAttribute(
        "aria-label",
        `${this.currentPrinterName || "Active printer"} — ${bedDetails}`,
      );
    }

    // Integrity badge: pending / skipped / computed (R-08, R-07).
    if (this.currentIntegrity === "pending" || this.currentIntegrity === null) {
      setBadge(this.integrityEl, "neutral", ICON_PENDING, "Checking mesh…", "Volume provisional");
      this.integrityEl.title = "Volume is provisional until the mesh check finishes";
    } else if (this.currentIntegrity === "skipped") {
      setBadge(
        this.integrityEl,
        "neutral",
        ICON_PENDING,
        "Not checked",
        bedDetails || "Volume provisional",
      );
      this.integrityEl.title = "Too many triangles to check integrity";
    } else {
      const i = this.currentIntegrity;
      const volumeNote =
        i.watertight && i.inconsistent_orientation === 0 ? "" : "Volume unreliable";
      const bedNote = bedDetails ? `Bed: ${bedDetails}` : "";
      const notes = [volumeNote, bedNote].filter(Boolean).join(" · ");
      if (i.watertight && i.degenerate_triangles === 0 && i.normal_disagreements === 0) {
        setBadge(this.integrityEl, "ok", ICON_CHECK, "Watertight", notes || undefined);
        this.integrityEl.title = notes;
      } else {
        const flags: string[] = [];
        if (i.boundary_edges > 0)
          flags.push(`${i.boundary_edges} open edge${i.boundary_edges === 1 ? "" : "s"}`);
        if (i.non_manifold_edges > 0) flags.push(`${i.non_manifold_edges} non-manifold`);
        if (i.inconsistent_orientation > 0)
          flags.push(`${i.inconsistent_orientation} flipped faces`);
        if (i.degenerate_triangles > 0) flags.push(`${i.degenerate_triangles} degenerate`);
        const text = flags.join(", ") || "Issues found";
        setBadge(this.integrityEl, "warn", ICON_WARN, text, notes || undefined);
        this.integrityEl.title = [text, notes].filter(Boolean).join(" · ");
      }
    }
  }
}

function divider(): HTMLElement {
  const d = document.createElement("div");
  d.className = "divider";
  return d;
}
