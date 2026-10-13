"use strict";
/* Same storage key as the results dashboard, so a choice here can be passed through. */
(() => {
  const key = "km.theme.v1";
  const media = matchMedia("(prefers-color-scheme: dark)");
  let preference = "system";
  try {
    const saved = localStorage.getItem(key);
    if (["system", "light", "dark"].includes(saved)) preference = saved;
  } catch { /* System default works without storage. */ }
  const resolved = () => preference === "system" ? (media.matches ? "dark" : "light") : preference;
  function apply(announce = false) {
    const mode = resolved();
    const root = document.documentElement;
    root.dataset.theme = mode;
    root.style.colorScheme = mode;
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.content = mode === "dark" ? "#171717" : "#F3F3F1";
    for (const value of ["system", "light", "dark"]) {
      document.getElementById("theme-" + value)?.setAttribute("aria-pressed", String(preference === value));
    }
    const status = document.getElementById("theme-status");
    if (announce && status) status.textContent = mode === "dark" ? "Dark theme selected." : "Light theme selected.";
    document.dispatchEvent(new CustomEvent("km-theme", {detail: mode}));
  }
  apply();
  document.addEventListener("DOMContentLoaded", () => {
    apply();
    for (const value of ["system", "light", "dark"]) {
      document.getElementById("theme-" + value)?.addEventListener("click", () => {
        preference = value;
        try { localStorage.setItem(key, value); } catch { /* This page still updates. */ }
        apply(true);
      });
    }
  });
  media.addEventListener("change", () => { if (preference === "system") apply(); });
  window.kmTheme = () => resolved();
})();
