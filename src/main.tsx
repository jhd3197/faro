import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { TerminalWindow } from "./components/TerminalWindow";
import { useSettings } from "./stores/settingsStore";
import { applyAccent } from "./lib/accent";
import { sweepStalePopoutBuffers } from "./lib/popout";
import "./styles.css";

// Demo/screenshot build: expose the stores on window.__demo so the headless
// capture script can drive the UI. Stripped from normal builds.
if (import.meta.env.VITE_MOCK) {
  await import("./mock/demo");
}

// Apply the persisted theme as early as possible to avoid a FOUC. No crossfade
// on first paint — only on later switches.
document.documentElement.setAttribute(
  "data-theme",
  useSettings.getState().appTheme
);
// Apply any saved accent override on top of the theme's own accent.
applyAccent(useSettings.getState().accentColor || null);

// Keep the html data-theme in sync with the setting store. On an actual theme
// change, add `.theming` so the (otherwise dormant) crossfade transition runs,
// then drop it once the transition is done.
let prevTheme = useSettings.getState().appTheme;
let prevAccent = useSettings.getState().accentColor;
let themingTimer: ReturnType<typeof setTimeout> | undefined;
useSettings.subscribe((s) => {
  if (s.accentColor !== prevAccent) {
    prevAccent = s.accentColor;
    applyAccent(s.accentColor || null);
  }
  if (s.appTheme === prevTheme) return;
  prevTheme = s.appTheme;
  const el = document.documentElement;
  el.classList.add("theming");
  el.setAttribute("data-theme", s.appTheme);
  if (themingTimer) clearTimeout(themingTimer);
  themingTimer = setTimeout(() => el.classList.remove("theming"), 260);
});

// Secondary windows load this same SPA with `?view=…` (see lib/popout.ts):
// a popped-out terminal renders just the terminal view, not the app shell.
// No StrictMode there — its dev double-mount would close the adopted PTY.
const view = new URLSearchParams(window.location.search).get("view");
// Reap handoff buffers a crashed popout left behind — deferred so it stays off
// the first-paint path and so a reload during an in-flight handoff can't eat
// a buffer its popout hasn't read yet.
if (view !== "terminal") window.setTimeout(sweepStalePopoutBuffers, 10_000);

ReactDOM.createRoot(document.getElementById("root")!).render(
  view === "terminal" ? (
    <TerminalWindow />
  ) : (
    <React.StrictMode>
      <App />
    </React.StrictMode>
  )
);
