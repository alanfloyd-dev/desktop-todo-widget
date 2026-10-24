/**
 * The typed dashboard layout model and its pure normalization policy.
 *
 * Data only — no Vue, no Tauri, no DOM — so the resolver below stays directly
 * unit-testable and C2 can persist the exact shape it resolves without
 * translation. The model is a logical preference (order + column span), never
 * pixels: a span is honored when the container is wide enough (C3) and
 * collapses to a single column when it is not. Per C0: one canonical layout
 * serves every window mode, heights stay content-driven, and layout is
 * deliberately independent of module visibility and runtime activity.
 */

/**
 * The dashboard module identities, as a typed projection of the registry, in
 * default presentation order.
 *
 * This tuple is intentionally the ONLY place dashboard ids are listed outside
 * `MODULE_REGISTRY`: it is the subset of the registry whose surface is
 * `dashboard`, and its sequence *is* the default layout order — the registry
 * remains the identity authority (a consistency test in `layout.test.ts`
 * compares the two as a set, so a dashboard module added to or removed from
 * the registry without updating this projection fails the suite). The default
 * order is layout data, which is why it lives here and not in the registry.
 */
export const DASHBOARD_MODULE_IDS = ["weather", "todo", "shortcuts"] as const;

export type DashboardModuleId = (typeof DASHBOARD_MODULE_IDS)[number];

export interface LayoutEntry {
  id: DashboardModuleId;
  /**
   * Dense position in the effective layout, `0..entries.length - 1`. The
   * resolver always re-indexes to this canonical form, so stored documents may
   * carry any finite number and equal values; only the resulting sequence
   * matters.
   */
  order: number;
  /**
   * Logical column preference, not a width. One column in narrow containers;
   * up to two columns where the container allows. Heights are content-driven.
   */
  colSpan: 1 | 2;
}

export interface ModuleLayout {
  /** Layout data format. Only `1` exists; other values resolve to the default. */
  version: 1;
  /** Each known dashboard module exactly once, in render order. */
  entries: LayoutEntry[];
}

/**
 * The layout every installation starts from: the dashboard projection in its
 * presentation order, every module on one column. The date line and footer
 * are shell and are not entries.
 *
 * All-enabled is to visibility what this is to layout: the upgrade contract.
 * An older document without a layout member must keep today's surface, so
 * absence always resolves here.
 */
export const DEFAULT_MODULE_LAYOUT: ModuleLayout = {
  version: 1,
  entries: DASHBOARD_MODULE_IDS.map((id, index) => ({ id, order: index, colSpan: 1 })),
};

/** Reads a stored entry's id, accepting only string ids. */
function storedId(entry: unknown): string | null {
  return typeof entry === "object" && entry !== null && "id" in entry
    ? typeof (entry as { id: unknown }).id === "string"
      ? ((entry as { id: string }).id as string)
      : null
    : null;
}

/** Reads a stored entry's order, accepting only finite numbers. */
function storedOrder(entry: unknown): number | null {
  if (typeof entry !== "object" || entry === null || !("order" in entry)) return null;
  const value = (entry as { order: unknown }).order;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Reads a stored entry's span; anything but 1 or 2 falls back to 1. */
function storedColSpan(entry: unknown): 1 | 2 {
  if (typeof entry !== "object" || entry === null || !("colSpan" in entry)) return 1;
  const value = (entry as { colSpan: unknown }).colSpan;
  return value === 1 || value === 2 ? value : 1;
}

/**
 * Builds the default-respecting effective layout for a dashboard id set.
 *
 * Generalized so the future-module path is exercisable without hardcoding a
 * module that does not exist yet: tests (and any future multi-source caller)
 * pass a different registry projection. Generic over the id type — a
 * projection may contain ids this build's `DashboardModuleId` does not know
 * (a module a future version registered), and the resolved entries carry that
 * same type honestly.
 */
export function resolveDashboardLayout<K extends string>(
  dashboardIds: readonly K[],
  preferences?: unknown,
): {
  version: 1;
  entries: Array<{ id: K; order: number; colSpan: 1 | 2 }>;
} {
  const defaults = () => ({
    version: 1 as const,
    entries: dashboardIds.map((id, index) => ({ id, order: index, colSpan: 1 as const })),
  });

  const shape = preferences as { version?: unknown; entries?: unknown } | null | undefined;
  if (
    typeof shape !== "object" ||
    shape === null ||
    shape.version !== 1 ||
    !Array.isArray(shape.entries)
  ) {
    return defaults();
  }

  // Known entries in input sequence, first occurrence winning, fields
  // normalized on read. Unknown ids and the overlay modules (review) never
  // enter the list; the stored document itself is only ever read here.
  const known = new Set<string>(dashboardIds);
  const seen = new Set<string>();
  const keyed: Array<{ id: string; order: number; colSpan: 1 | 2 }> = [];
  const unkeyed: Array<{ id: string; colSpan: 1 | 2 }> = [];
  for (const raw of shape.entries) {
    const id = storedId(raw);
    if (id === null || !known.has(id) || seen.has(id)) continue;
    seen.add(id);
    const order = storedOrder(raw);
    const colSpan = storedColSpan(raw);
    if (order === null) unkeyed.push({ id, colSpan });
    else keyed.push({ id, order, colSpan });
  }

  // Deterministic sequence: keyed ascending by stored order (numeric only;
  // equal orders keep input sequence), then unkeyed in input sequence, then
  // missing modules appended in projection order. Dense re-index.
  keyed.sort((a, b) => a.order - b.order);
  const resolved: Array<{ id: string; colSpan: 1 | 2 }> = [
    ...keyed.map(({ id, colSpan }) => ({ id, colSpan })),
    ...unkeyed,
  ];
  for (const id of dashboardIds) {
    if (!seen.has(id)) resolved.push({ id, colSpan: 1 });
  }
  return {
    version: 1 as const,
    entries: resolved.map((entry, index) => ({
      // The filter above admitted only ids from `dashboardIds`, so the id is
      // a `K`; the type system cannot see the set membership through `string`.
      id: entry.id as K,
      order: index,
      colSpan: entry.colSpan,
    })),
  };
}

/**
 * Whether a resolved layout actually requests a second column.
 *
 * This is the rendering activation policy (C3b-2): the dashboard stays
 * single-column until a stored entry carries `colSpan: 2`, so the all-ones
 * default layout keeps today's full-width presentation on every window width
 * no matter how wide the container is. A span request widens the canvas for
 * every entry — a `colSpan: 1` module then occupies one of the two columns —
 * and collapses back to one column wherever two columns do not fit, without
 * touching the persisted preferences.
 */
export function hasColumnSpan(layout: ModuleLayout): boolean {
  return layout.entries.some((entry) => entry.colSpan === 2);
}

/**
 * Resolves the effective render layout from optional stored preferences.
 *
 * Semantics: missing/null/malformed input or an unsupported version resolves
 * to `DEFAULT_MODULE_LAYOUT`; valid known entries keep their intended
 * ordering; missing known modules are appended in registry order; duplicates
 * resolve first-wins; invalid orders/spans fall back per the field rules
 * above; review and unknown module ids are excluded from the effective
 * layout. The input is never mutated, and the result is freshly allocated
 * per call with exactly the known dashboard modules, each once. Unknown
 * members of the stored document are ignored here and preserved where they
 * live — in the persisted settings document (C2), never rewritten from this
 * resolved view.
 */
export function resolveModuleLayout(preferences?: unknown): ModuleLayout {
  return resolveDashboardLayout(DASHBOARD_MODULE_IDS, preferences);
}
