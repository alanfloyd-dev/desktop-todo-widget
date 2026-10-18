import type { QuickLink } from "../../types";

/**
 * Pure Quick Link list semantics, gathered from the Settings editor draft.
 *
 * The panel keeps owning the draft state and the editor flow; everything here
 * is locale-free and directly unit testable. The backend re-validates every
 * patch, so these checks only keep the obvious mistakes next to the field.
 */

/**
 * Stable id for a new link.
 *
 * `crypto.randomUUID` is available in the WebView2 runtime and in every browser
 * the preview supports; the fallback keeps a non-secure preview origin working
 * without pulling an id library in for one call site.
 */
export function newLinkId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `link-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * Client-side URL check, mirroring the Rust validator.
 *
 * Returns a translation key (`null` when the URL is acceptable) instead of
 * finished copy, so the logic stays locale-free; the caller resolves the key
 * through the app's single i18n instance. The backend is still the authority —
 * it re-validates the patch and returns an error the panel surfaces — but
 * catching it here keeps the message next to the field instead of a round trip
 * away.
 */
export function quickLinkUrlErrorKey(url: string): string | null {
  const trimmed = url.trim();
  if (!trimmed) return "quickLinks.error.urlRequired";
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return "quickLinks.error.invalidUrl";
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return "quickLinks.error.invalidUrl";
  }
  return null;
}

/**
 * Moves a link one row up or down.
 *
 * In-place swap rather than a splice-and-insert: the identity of every row is its
 * `id`, so the simplest correct move is the one that only changes positions.
 * Out-of-range moves (the first row up, the last row down, an unknown id) are
 * no-ops.
 */
export function moveLink(links: QuickLink[], id: string, delta: -1 | 1): void {
  const index = links.findIndex((link) => link.id === id);
  const target = index + delta;
  if (index < 0 || target < 0 || target >= links.length) return;
  [links[index], links[target]] = [links[target], links[index]];
}
