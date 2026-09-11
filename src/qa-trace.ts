import { invoke } from "@tauri-apps/api/core";

/**
 * Minimal QA beacon for product-interaction regression tracing.
 *
 * Phase 7C.3-B3 needed to distinguish "the click/keystroke never reached the
 * WebView" from "the frontend ran but the command failed" for the Desktop-mode
 * add/reorder regressions. This reports only fixed, non-identifying event names
 * to the existing `qa_frontend_input` command; it never sends user text, task
 * titles, or ids.
 *
 * It is a no-op outside Tauri (browser preview) and is deliberately fire-and-forget:
 * a tracing failure must never affect product behaviour.
 */
const NATIVE = "__TAURI_INTERNALS__" in window;

export function qaTrace(kind: string): void {
  if (!NATIVE) return;
  void invoke("qa_frontend_input", { kind }).catch(() => {});
}
