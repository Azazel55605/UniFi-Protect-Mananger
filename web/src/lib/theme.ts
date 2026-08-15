/**
 * Appearance: theme and accent.
 *
 * Kept on the device rather than in the server's settings. Appearance is a
 * property of the screen you're looking at — a phone in a dark room and a
 * desktop in daylight want different answers, and syncing them would be the
 * wrong behaviour rather than a missing feature.
 */
import { useCallback, useEffect, useState } from "react";

export type Theme = "system" | "light" | "dark";
export type Accent = "amber" | "cyan" | "violet" | "rose" | "lime";

export const ACCENTS: { id: Accent; label: string }[] = [
  { id: "amber", label: "Amber" },
  { id: "cyan", label: "Cyan" },
  { id: "violet", label: "Violet" },
  { id: "rose", label: "Rose" },
  { id: "lime", label: "Lime" },
];

const THEME_KEY = "pm.theme";
const ACCENT_KEY = "pm.accent";

export function readTheme(): Theme {
  const v = localStorage.getItem(THEME_KEY);
  return v === "light" || v === "dark" ? v : "system";
}

export function readAccent(): Accent {
  const v = localStorage.getItem(ACCENT_KEY);
  return ACCENTS.some((a) => a.id === v) ? (v as Accent) : "amber";
}

/**
 * "system" removes the attribute entirely rather than resolving it to a
 * concrete value, so the CSS media query stays in charge and the page follows
 * the OS if it changes while open.
 */
export function applyAppearance(theme: Theme, accent: Accent) {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.dataset.theme = theme;
  root.dataset.accent = accent;
}

export function useAppearance() {
  const [theme, setThemeState] = useState<Theme>(readTheme);
  const [accent, setAccentState] = useState<Accent>(readAccent);

  useEffect(() => {
    applyAppearance(theme, accent);
  }, [theme, accent]);

  const setTheme = useCallback((next: Theme) => {
    localStorage.setItem(THEME_KEY, next);
    setThemeState(next);
  }, []);

  const setAccent = useCallback((next: Accent) => {
    localStorage.setItem(ACCENT_KEY, next);
    setAccentState(next);
  }, []);

  return { theme, accent, setTheme, setAccent };
}
