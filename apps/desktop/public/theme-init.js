(() => {
  let mode = "system";
  try {
    const stored = window.localStorage.getItem("firstshell.theme");
    if (stored === "light" || stored === "dark" || stored === "system") mode = stored;
  } catch {
    // Use the system preference when storage is unavailable.
  }
  const resolved = mode === "system"
    ? (window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light")
    : mode;
  document.documentElement.dataset.theme = mode;
  document.documentElement.dataset.resolvedTheme = resolved;
  document.documentElement.style.colorScheme = resolved;
})();
