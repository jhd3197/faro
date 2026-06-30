/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Demo/screenshot build — swaps the Tauri API surface for an in-browser mock. */
  readonly VITE_MOCK?: boolean;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
