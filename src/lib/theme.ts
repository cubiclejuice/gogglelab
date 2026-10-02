import type { AppearanceMode } from "./ipc";

export type EffectiveTheme = "light" | "dark";

const systemTheme =
  typeof window === "undefined" ? undefined : window.matchMedia?.("(prefers-color-scheme: dark)");
let appearance: AppearanceMode = "system";
let effectiveTheme: EffectiveTheme = systemTheme?.matches ? "dark" : "light";

function synchronizeTheme() {
  const nextTheme =
    appearance === "system" ? (systemTheme?.matches ? "dark" : "light") : appearance;
  const changed = nextTheme !== effectiveTheme;
  effectiveTheme = nextTheme;
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = effectiveTheme;
    document.documentElement.style.colorScheme = effectiveTheme;
  }
  if (changed && typeof window !== "undefined") {
    window.dispatchEvent(new Event("gogglelab-theme-change"));
  }
}

/** Explicit appearance stays fixed; System follows macOS appearance changes. */
export function applyTheme(mode: AppearanceMode): void {
  appearance = mode;
  synchronizeTheme();
}

export function getEffectiveTheme(): EffectiveTheme {
  return effectiveTheme;
}

systemTheme?.addEventListener("change", synchronizeTheme);
synchronizeTheme();
