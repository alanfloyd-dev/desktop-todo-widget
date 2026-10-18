import type { QuickLink } from "../../types";

/**
 * The bridge the shortcuts module runs through.
 *
 * The same injection shape as `WeatherBridge`/`ReviewBridge`: the shell passes
 * the native `invoke`, and tests inject a fake bridge to drive the call
 * deterministically. This module never imports `@tauri-apps` itself, so it
 * stays loadable in the plain browser preview and under node.
 */
export interface ShortcutsBridge {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

/**
 * Opens a Quick Link.
 *
 * The backend hands the URL to the Shell so the system default browser opens
 * it; the browser preview has no Shell and keeps its own `window.open` path
 * in the app shell. `id` is the stable link identity, never the array index.
 */
export function openQuickLink(bridge: ShortcutsBridge, id: string): Promise<void> {
  return bridge.invoke("open_quick_link", { id });
}
