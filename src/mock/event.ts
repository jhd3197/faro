// Mock of @tauri-apps/api/event for the demo build. Real listeners get a no-op
// unlisten; `emit` lets the mock `invoke` push events (e.g. terminal output) to
// whoever is listening.
export type UnlistenFn = () => void;

type Handler = (e: { event: string; payload: unknown; id: number }) => void;
const handlers = new Map<string, Set<Handler>>();
let seq = 0;

export async function listen<T>(
  event: string,
  cb: (e: { event: string; payload: T; id: number }) => void
): Promise<UnlistenFn> {
  const set = handlers.get(event) ?? new Set();
  set.add(cb as Handler);
  handlers.set(event, set);
  return () => set.delete(cb as Handler);
}

export function emit(event: string, payload: unknown) {
  const set = handlers.get(event);
  if (!set) return;
  for (const h of set) h({ event, payload, id: seq++ });
}
