import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { useSettings } from "./stores/settingsStore";
import "./styles.css";

// Apply the persisted theme as early as possible to avoid a FOUC.
document.documentElement.setAttribute(
  "data-theme",
  useSettings.getState().appTheme
);

// Keep the html data-theme in sync with the setting store.
useSettings.subscribe((s) => {
  document.documentElement.setAttribute("data-theme", s.appTheme);
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
