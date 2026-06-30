// Mock of @tauri-apps/api/window for the demo build — just enough for the custom
// title bar's window controls (no-ops in a plain browser).
export function getCurrentWindow() {
  return {
    async isMaximized() {
      return false;
    },
    async onResized() {
      return () => {};
    },
    async minimize() {},
    async toggleMaximize() {},
    async close() {},
    async startDragging() {},
  };
}
