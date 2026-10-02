export const DEFAULT_ACCENT = "#2f5f4c";

/** Keep arbitrary user colors legible, including very pale and dark choices. */
export function applyAccent(color: string | null) {
  const style = document.documentElement.style;
  const tokens = [
    "--accent-fill",
    "--accent-fill-hover",
    "--accent-text",
    "--accent",
    "--focus",
    "--sel-bg",
    "--sel-text",
  ];
  if (!color || !/^#[0-9a-f]{6}$/i.test(color)) {
    tokens.forEach((token) => style.removeProperty(token));
    window.dispatchEvent(new Event("gogglelab-accent-change"));
    return;
  }
  const rgb = [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
  const linear = rgb.map((v) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  const luminance = linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
  const text = luminance > 0.179 ? "#000000" : "#ffffff";
  const target = text === "#000000" ? 255 : 0;
  const hover =
    "#" +
    rgb
      .map((v) =>
        Math.round(v * 0.88 + target * 0.12)
          .toString(16)
          .padStart(2, "0"),
      )
      .join("");
  style.setProperty("--accent-fill", color);
  style.setProperty("--accent-fill-hover", hover);
  style.setProperty("--accent-text", text);
  style.setProperty("--accent", `color-mix(in srgb, ${color} 65%, var(--text))`);
  style.setProperty("--focus", "var(--accent)");
  style.setProperty("--sel-bg", `color-mix(in srgb, ${color} 18%, var(--sidebar))`);
  style.setProperty("--sel-text", "var(--text)");
  window.dispatchEvent(new Event("gogglelab-accent-change"));
}
