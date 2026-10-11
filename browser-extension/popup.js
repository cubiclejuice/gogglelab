const button = document.querySelector("#capture");
const status = document.querySelector("#status");
button.addEventListener("click", async () => {
  button.disabled = true;
  status.textContent = "Capturing…";
  try {
    const result = await chrome.runtime.sendMessage({ type: "capture.activePage" });
    status.textContent = result?.ok ? "Queued for explicit GoggleLab import." : result?.error || "Capture failed.";
  } finally {
    button.disabled = false;
  }
});
