import type { ReviewPeriod, ReviewReport } from "../../types";

/**
 * The bridge the review panel runs through.
 *
 * The same injection shape as `UpdateBridge`/`WeatherBridge`: the panel passes
 * the native `invoke` when the Tauri runtime is present, and tests inject a
 * fake bridge to drive the call deterministically. This module never imports
 * `@tauri-apps` itself, so it stays loadable in the plain browser preview and
 * under node.
 */
export interface ReviewBridge {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

/**
 * The review panel's single backend call.
 *
 * `anchorDate` is `null` for the first open of a period — the backend then
 * anchors on the current task day — and the response's own `anchorDate` is
 * what the panel navigates from afterwards.
 */
export function fetchReviewReport(
  bridge: ReviewBridge,
  period: ReviewPeriod,
  anchorDate: string | null,
): Promise<ReviewReport> {
  return bridge.invoke<ReviewReport>("review_report", { period, anchorDate });
}
