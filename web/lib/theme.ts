"use client";
import { useCallback, useEffect, useSyncExternalStore } from "react";

export type Theme = "light" | "dark" | "system";
const KEY = "hope-theme";
const listeners = new Set<() => void>();

function stored(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}
function prefersDark() {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}
function subscribe(callback: () => void) {
  listeners.add(callback);
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", callback);
  window.addEventListener("storage", callback);
  return () => {
    listeners.delete(callback);
    media.removeEventListener("change", callback);
    window.removeEventListener("storage", callback);
  };
}
const themeSnapshot = (): Theme => stored();
const darkSnapshot = () => {
  const theme = stored();
  return theme === "dark" || (theme === "system" && prefersDark());
};

/** Theme preference persisted per browser; "system" follows the OS setting live. */
export function useTheme() {
  const theme = useSyncExternalStore<Theme>(
    subscribe,
    themeSnapshot,
    () => "system",
  );
  const dark = useSyncExternalStore(subscribe, darkSnapshot, () => false);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
  }, [dark]);
  const setTheme = useCallback((next: Theme) => {
    try {
      if (next === "system") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, next);
    } catch {
      // Private browsing or blocked storage: the choice still applies to this page.
    }
    for (const listener of listeners) listener();
  }, []);
  return { theme, dark, setTheme };
}
