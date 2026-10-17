export function initializeCapturePopup(root, runtime) {
  const form = root.querySelector("#capture-form");
  const button = root.querySelector("#capture");
  const enrichButton = root.querySelector("#enrich");
  const retryButton = root.querySelector("#retry-preview");
  const status = root.querySelector("#status");
  const fields = {
    title: root.querySelector("#title"),
    designer: root.querySelector("#designer"),
    license: root.querySelector("#license"),
    sourceUrl: root.querySelector("#source-url"),
    notes: root.querySelector("#notes"),
  };
  let enrichmentInProgress = false;

  form.hidden = true;
  button.disabled = true;

  async function loadPreview() {
    form.hidden = true;
    button.disabled = true;
    if (retryButton) retryButton.hidden = true;
    status.textContent = "Loading model details…";
    try {
      const result = await runtime.sendMessage({ type: "capture.preview" });
      if (!result?.ok || !result.page)
        throw new Error(result?.error || "Could not read this model page");
      fields.title.value = result.page.title ?? "";
      fields.designer.value = result.page.designer ?? "";
      fields.license.value = result.page.license ?? "";
      fields.sourceUrl.value = result.page.url ?? "";
      fields.notes.value = result.page.notes ?? "";
      form.hidden = false;
      button.disabled = false;
      if (enrichButton) enrichButton.hidden = result.enrichmentAvailable !== true;
      status.textContent =
        "Review these details. GoggleLab will import them only after you choose Import in the app.";
      return result.page;
    } catch (error) {
      status.textContent = error?.message || "Could not load model details";
      if (retryButton) retryButton.hidden = false;
      return null;
    }
  }

  async function submit(event) {
    event.preventDefault();
    if (enrichmentInProgress) {
      status.textContent = "Wait for catalog enrichment to finish before queueing.";
      return null;
    }
    if (typeof form.checkValidity === "function" && !form.checkValidity()) {
      form.reportValidity?.();
      return null;
    }

    button.disabled = true;
    if (retryButton) retryButton.hidden = true;
    status.textContent = "Queueing capture…";
    const review = Object.fromEntries(
      Object.entries(fields).map(([key, field]) => [key, field.value]),
    );
    try {
      const result = await runtime.sendMessage({ type: "capture.activePage", review });
      if (!result?.ok) throw new Error(result?.error || "Capture could not be queued");
      status.textContent = "Queued. Import it from GoggleLab when you’re ready.";
      return result;
    } catch (error) {
      status.textContent = error?.message || "Capture could not be queued";
      button.disabled = false;
      return null;
    }
  }

  async function enrich() {
    if (!enrichButton || enrichmentInProgress) return null;
    enrichmentInProgress = true;
    enrichButton.disabled = true;
    const queueWasDisabled = button.disabled;
    button.disabled = true;
    status.textContent = "Fetching public catalog metadata…";
    try {
      const result = await runtime.sendMessage({ type: "capture.enrichPage" });
      if (!result?.ok || !result.suggestions) {
        throw new Error(result?.error || "No public metadata was found");
      }
      for (const field of ["title", "designer", "license", "notes"]) {
        const suggestion = result.suggestions[field];
        if (typeof suggestion === "string" && suggestion.trim()) {
          fields[field].value = suggestion;
        }
      }
      status.textContent = "Suggestions loaded. Review them before queueing.";
      return result.suggestions;
    } catch (error) {
      status.textContent = error?.message || "Catalog enrichment failed";
      return null;
    } finally {
      enrichButton.disabled = false;
      button.disabled = queueWasDisabled;
      enrichmentInProgress = false;
    }
  }

  form.addEventListener("submit", submit);
  enrichButton?.addEventListener("click", enrich);
  retryButton?.addEventListener("click", loadPreview);
  const ready = loadPreview();
  return { ready, loadPreview, submit, enrich };
}

const popupController =
  typeof document !== "undefined" && globalThis.chrome?.runtime
    ? initializeCapturePopup(document, chrome.runtime)
    : null;

export const popupReady = popupController?.ready ?? Promise.resolve(null);
