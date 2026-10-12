(function () {
  // A startup cache; the Rust-backed config.json is authoritative after hydration.
  var stored;
  try { stored = localStorage.getItem("hypr-theme"); } catch { stored = null; }
  var theme =
    stored === "light" || stored === "dark" || stored === "system"
      ? stored
      : "system";
  var prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  var isDark =
    theme === "dark" ? true : theme === "light" ? false : prefersDark;
  document.documentElement.classList.toggle("dark", isDark);
})();
