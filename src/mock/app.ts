// Mock of @tauri-apps/api/app for the demo build — lets the About section render
// a version without the Rust backend (Plan 16). Mirrors the real API shape.
export async function getVersion(): Promise<string> {
  return "1.3.22";
}

export async function getName(): Promise<string> {
  return "Faro";
}

export async function getTauriVersion(): Promise<string> {
  return "2.0.0";
}
