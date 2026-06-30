// Mock of @tauri-apps/api/path for the demo build.
export async function downloadDir(): Promise<string> {
  return "C:\\Users\\demo\\Downloads";
}
export async function homeDir(): Promise<string> {
  return "C:\\Users\\demo";
}
