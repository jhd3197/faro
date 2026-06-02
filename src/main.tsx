import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { useSettings } from "./stores/settingsStore";
import "./styles.css";

// Apply the persisted theme as early as possible to avoid a FOUC. No crossfade
// on first paint — only on later switches.
document.documentElement.setAttribute(
  "data-theme",
  useSettings.getState().appTheme
);

// Keep the html data-theme in sync with the setting store. On an actual theme
// change, add `.theming` so the (otherwise dormant) crossfade transition runs,
// then drop it once the transition is done.
let prevTheme = useSettings.getState().appTheme;
let themingTimer: ReturnType<typeof setTimeout> | undefined;
useSettings.subscribe((s) => {
  if (s.appTheme === prevTheme) return;
  prevTheme = s.appTheme;
  const el = document.documentElement;
  el.classList.add("theming");
  el.setAttribute("data-theme", s.appTheme);
  if (themingTimer) clearTimeout(themingTimer);
  themingTimer = setTimeout(() => el.classList.remove("theming"), 260);
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
