import { useEffect, useState } from "react";
import { MonitorCog, Moon, Sun } from "lucide-react";

export type ThemeMode = "light" | "dark" | "system";

const THEME_STORAGE_KEY = "firstshell.theme";

function storedTheme(): ThemeMode {
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);
    return value === "light" || value === "dark" || value === "system" ? value : "system";
  } catch {
    return "system";
  }
}

function systemTheme(): Exclude<ThemeMode, "system"> {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(mode: ThemeMode) {
  const resolved = mode === "system" ? systemTheme() : mode;
  document.documentElement.dataset.theme = mode;
  document.documentElement.dataset.resolvedTheme = resolved;
  document.documentElement.style.colorScheme = resolved;
  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", "light dark");
}

export function useThemeMode(): [ThemeMode, (mode: ThemeMode) => void] {
  const [mode, setMode] = useState<ThemeMode>(storedTheme);

  useEffect(() => {
    applyTheme(mode);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, mode);
    } catch {
      // The selected theme still applies for this process when storage is unavailable.
    }
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (mode !== "system" || !media) return;
    const onChange = () => applyTheme("system");
    media.addEventListener?.("change", onChange);
    return () => media.removeEventListener?.("change", onChange);
  }, [mode]);

  return [mode, setMode];
}

export function ThemeControl({
  mode,
  onChange,
  floating = false,
}: {
  mode: ThemeMode;
  onChange(mode: ThemeMode): void;
  floating?: boolean;
}) {
  const choices = [
    { mode: "light" as const, label: "浅色", icon: Sun },
    { mode: "dark" as const, label: "深色", icon: Moon },
    { mode: "system" as const, label: "跟随系统", icon: MonitorCog },
  ];
  return (
    <div className={`theme-control${floating ? " floating" : ""}`} role="group" aria-label="界面主题">
      {choices.map((choice) => {
        const Icon = choice.icon;
        return (
          <button
            key={choice.mode}
            type="button"
            aria-label={choice.label}
            aria-pressed={mode === choice.mode}
            title={choice.label}
            onClick={() => onChange(choice.mode)}
          >
            <Icon size={14} aria-hidden="true" />
          </button>
        );
      })}
    </div>
  );
}
