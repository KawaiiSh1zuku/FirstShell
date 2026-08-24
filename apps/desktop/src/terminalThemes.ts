import type { TerminalPalette } from "./types";

export interface TerminalThemePreset {
  id: string;
  label: string;
  palette: TerminalPalette;
}

export const TERMINAL_THEME_PRESETS: readonly TerminalThemePreset[] = [
  {
    id: "firstshell-dark",
    label: "FirstShell Dark",
    palette: {
      foreground: "#d7ddd8", background: "#0e100e", cursor: "#4fd18a", cursorAccent: "#0e100e", selectionBackground: "#284734",
      black: "#1b1f1c", red: "#e06c75", green: "#78c58b", yellow: "#d7ba7d", blue: "#6ca9d9", magenta: "#c586c0", cyan: "#62c7c9", white: "#c9d0ca",
      brightBlack: "#657168", brightRed: "#f07d86", brightGreen: "#8fda9f", brightYellow: "#e8cd8e", brightBlue: "#83bde9", brightMagenta: "#d99bd3", brightCyan: "#7bdadd", brightWhite: "#f0f4f1",
    },
  },
  {
    id: "firstshell-light",
    label: "FirstShell Light",
    palette: {
      foreground: "#243128", background: "#f7f9f7", cursor: "#167447", cursorAccent: "#f7f9f7", selectionBackground: "#b9dfc8",
      black: "#263129", red: "#b4232d", green: "#197044", yellow: "#8a6400", blue: "#245fa5", magenta: "#884b91", cyan: "#19727a", white: "#d8ded9",
      brightBlack: "#68746b", brightRed: "#d13b44", brightGreen: "#278b58", brightYellow: "#a77900", brightBlue: "#3478c5", brightMagenta: "#a160aa", brightCyan: "#248c95", brightWhite: "#ffffff",
    },
  },
  {
    id: "solarized-dark",
    label: "Solarized Dark",
    palette: {
      foreground: "#839496", background: "#002b36", cursor: "#93a1a1", cursorAccent: "#002b36", selectionBackground: "#073642",
      black: "#073642", red: "#dc322f", green: "#859900", yellow: "#b58900", blue: "#268bd2", magenta: "#d33682", cyan: "#2aa198", white: "#eee8d5",
      brightBlack: "#586e75", brightRed: "#cb4b16", brightGreen: "#586e75", brightYellow: "#657b83", brightBlue: "#839496", brightMagenta: "#6c71c4", brightCyan: "#93a1a1", brightWhite: "#fdf6e3",
    },
  },
  {
    id: "solarized-light",
    label: "Solarized Light",
    palette: {
      foreground: "#657b83", background: "#fdf6e3", cursor: "#586e75", cursorAccent: "#fdf6e3", selectionBackground: "#eee8d5",
      black: "#073642", red: "#dc322f", green: "#859900", yellow: "#b58900", blue: "#268bd2", magenta: "#d33682", cyan: "#2aa198", white: "#eee8d5",
      brightBlack: "#586e75", brightRed: "#cb4b16", brightGreen: "#586e75", brightYellow: "#657b83", brightBlue: "#839496", brightMagenta: "#6c71c4", brightCyan: "#93a1a1", brightWhite: "#fdf6e3",
    },
  },
  {
    id: "nord",
    label: "Nord",
    palette: {
      foreground: "#d8dee9", background: "#2e3440", cursor: "#88c0d0", cursorAccent: "#2e3440", selectionBackground: "#434c5e",
      black: "#3b4252", red: "#bf616a", green: "#a3be8c", yellow: "#ebcb8b", blue: "#81a1c1", magenta: "#b48ead", cyan: "#88c0d0", white: "#e5e9f0",
      brightBlack: "#4c566a", brightRed: "#bf616a", brightGreen: "#a3be8c", brightYellow: "#ebcb8b", brightBlue: "#81a1c1", brightMagenta: "#b48ead", brightCyan: "#8fbcbb", brightWhite: "#eceff4",
    },
  },
  {
    id: "dracula",
    label: "Dracula",
    palette: {
      foreground: "#f8f8f2", background: "#282a36", cursor: "#f8f8f2", cursorAccent: "#282a36", selectionBackground: "#44475a",
      black: "#21222c", red: "#ff5555", green: "#50fa7b", yellow: "#f1fa8c", blue: "#bd93f9", magenta: "#ff79c6", cyan: "#8be9fd", white: "#f8f8f2",
      brightBlack: "#6272a4", brightRed: "#ff6e6e", brightGreen: "#69ff94", brightYellow: "#ffffa5", brightBlue: "#d6acff", brightMagenta: "#ff92df", brightCyan: "#a4ffff", brightWhite: "#ffffff",
    },
  },
  // TODO(M7 licensing): confirm an explicit upstream Gruvbox license before distribution or remove this preset.
  {
    id: "gruvbox-dark",
    label: "Gruvbox Dark",
    palette: {
      foreground: "#ebdbb2", background: "#282828", cursor: "#fabd2f", cursorAccent: "#282828", selectionBackground: "#504945",
      black: "#282828", red: "#cc241d", green: "#98971a", yellow: "#d79921", blue: "#458588", magenta: "#b16286", cyan: "#689d6a", white: "#a89984",
      brightBlack: "#928374", brightRed: "#fb4934", brightGreen: "#b8bb26", brightYellow: "#fabd2f", brightBlue: "#83a598", brightMagenta: "#d3869b", brightCyan: "#8ec07c", brightWhite: "#ebdbb2",
    },
  },
  {
    id: "one-dark",
    label: "One Dark",
    palette: {
      foreground: "#abb2bf", background: "#282c34", cursor: "#528bff", cursorAccent: "#282c34", selectionBackground: "#3e4451",
      black: "#1e2127", red: "#e06c75", green: "#98c379", yellow: "#d19a66", blue: "#61afef", magenta: "#c678dd", cyan: "#56b6c2", white: "#abb2bf",
      brightBlack: "#5c6370", brightRed: "#e06c75", brightGreen: "#98c379", brightYellow: "#d19a66", brightBlue: "#61afef", brightMagenta: "#c678dd", brightCyan: "#56b6c2", brightWhite: "#ffffff",
    },
  },
  {
    id: "high-contrast",
    label: "High Contrast",
    palette: {
      foreground: "#ffffff", background: "#000000", cursor: "#00ff7f", cursorAccent: "#000000", selectionBackground: "#295f3f",
      black: "#000000", red: "#ff5f5f", green: "#5fff87", yellow: "#ffff5f", blue: "#5fafff", magenta: "#ff5fff", cyan: "#5fffff", white: "#ffffff",
      brightBlack: "#767676", brightRed: "#ff8787", brightGreen: "#87ffaf", brightYellow: "#ffff87", brightBlue: "#87c7ff", brightMagenta: "#ff87ff", brightCyan: "#87ffff", brightWhite: "#ffffff",
    },
  },
];

export const DEFAULT_CUSTOM_PALETTE: TerminalPalette = structuredClone(TERMINAL_THEME_PRESETS[0].palette);

export function resolveTerminalTheme(theme: string | undefined, palette?: TerminalPalette | null): TerminalPalette {
  if (theme === "custom" && palette) return palette;
  return TERMINAL_THEME_PRESETS.find((value) => value.id === theme)?.palette ?? TERMINAL_THEME_PRESETS[0].palette;
}
