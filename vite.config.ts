import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async ({ mode }) => ({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
      // Consume the workspace package straight from its TS source during dev —
      // edit a component once and both the app and the package see it instantly.
      "@faro/file-ui": path.resolve(
        __dirname,
        "packages/file-ui/src/index.ts"
      ),
      // Demo/screenshot build: swap the Tauri API surface for an in-browser mock
      // so the whole UI renders with fictional data and no Rust backend.
      ...(mode === "mock"
        ? {
            "@tauri-apps/api/core": path.resolve(__dirname, "src/mock/core.ts"),
            "@tauri-apps/api/event": path.resolve(__dirname, "src/mock/event.ts"),
            "@tauri-apps/api/path": path.resolve(__dirname, "src/mock/path.ts"),
            "@tauri-apps/api/window": path.resolve(__dirname, "src/mock/window.ts"),
            "@tauri-apps/api/app": path.resolve(__dirname, "src/mock/app.ts"),
            "@tauri-apps/plugin-notification": path.resolve(
              __dirname,
              "src/mock/notification.ts"
            ),
          }
        : {}),
    },
  },
  define: {
    "import.meta.env.VITE_MOCK": JSON.stringify(mode === "mock"),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
