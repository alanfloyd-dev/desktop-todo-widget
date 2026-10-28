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
  /**
   * Collapsed-body preference (D1): the module renders only its header while
   * its content stays mounted. Missing or `false` means expanded, so a
   * document (or a resolved entry) that omits the member is expanded — the
   * flag is additive and never affects order, span, or `hasColumnSpan`.
   */
  collapsed?: boolean;
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
 * Reads a stored entry's collapsed flag; only the literal `true` collapses.
 * Every other value — absent, `false`, a non-boolean — resolves to expanded,
 * the same field-level fallback rule the order and span readers apply.
 */
function storedCollapsed(entry: unknown): boolean {
  if (typeof entry !== "object" || entry === null || !("collapsed" in entry)) return false;
  return (entry as { collapsed: unknown }).collapsed === true;
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
  entries: Array<{ id: K; order: number; colSpan: 1 | 2; collapsed?: boolean }>;
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
  const keyed: Array<{ id: string; order: number; colSpan: 1 | 2; collapsed: boolean }> = [];
  const unkeyed: Array<{ id: string; colSpan: 1 | 2; collapsed: boolean }> = [];
  for (const raw of shape.entries) {
    const id = storedId(raw);
    if (id === null || !known.has(id) || seen.has(id)) continue;
    seen.add(id);
    const order = storedOrder(raw);
    const colSpan = storedColSpan(raw);
    const collapsed = storedCollapsed(raw);
    if (order === null) unkeyed.push({ id, colSpan, collapsed });
    else keyed.push({ id, order, colSpan, collapsed });
  }

  // Deterministic sequence: keyed ascending by stored order (numeric only;
  // equal orders keep input sequence), then unkeyed in input sequence, then
  // missing modules appended in projection order. Dense re-index.
  keyed.sort((a, b) => a.order - b.order);
  const resolved: Array<{ id: string; colSpan: 1 | 2; collapsed: boolean }> = [
    ...keyed.map(({ id, colSpan, collapsed }) => ({ id, colSpan, collapsed })),
    ...unkeyed,
  ];
  for (const id of dashboardIds) {
    if (!seen.has(id)) resolved.push({ id, colSpan: 1, collapsed: false });
  }
  return {
    version: 1 as const,
    entries: resolved.map((entry, index) => ({
      // The filter above admitted only ids from `dashboardIds`, so the id is
      // a `K`; the type system cannot see the set membership through `string`.
      id: entry.id as K,
      order: index,
      colSpan: entry.colSpan,
      // The collapsed member is carried only when it resolves to true, so a
      // layout nothing has collapsed keeps the exact shape it always had.
      ...(entry.collapsed ? { collapsed: true as const } : {}),
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

/**
 * Builds the collapse patch for one module from the RAW stored layout
 * document (D1).
 *
 * This is the load-bearing safety rule of the collapse feature: the update is
 * constructed from the stored document exactly as persisted — never from
 * `resolveModuleLayout()` output, which is a presentation projection that
 * discards unknown root fields, unknown module entries, and unknown per-entry
 * fields. Patching from the projection would silently erase them (the Rust
 * merge keeps unknown members only when the patch itself carries them, and
 * overwrites `order`/`colSpan` for every entry it names).
 *
 * Semantics:
 * - The stored document must provably be a version-1 shape (same gate the
 *   resolver applies). Anything else — an opaque future format, a malformed
 *   object — returns `null` and the caller submits no patch at all: the
 *   stored document stays untouched rather than being rewritten or downgraded.
 * - Every stored entry survives, including unknown ids, and only the FIRST
 *   entry matching `id` (the one the resolver reads) gains `collapsed`.
 *   Each entry's `order`, `colSpan`, and unknown members are carried as-is;
 *   unknown root members ride along on the cloned document.
 * - A known module with no stored entry is appended as a COMPLETE entry —
 *   `id`, the resolver's dense `order`, the resolver's `colSpan`, and
 *   `collapsed` — never a partial `{ id, collapsed }`, which the Rust merge
 *   would treat as erasing the entry's span. The resolver, not the stored
 *   document, supplies the position because a missing entry has none.
 *
 * The input is never mutated; the returned document is a deep clone. The
 * `ModuleLayout` return type names the known members the backend re-merges —
 * unknown members the document carries ride along at runtime by construction.
 */
export function setModuleCollapsed(
  preferences: unknown,
  id: DashboardModuleId,
  collapsed: boolean,
): ModuleLayout | null {
  const shape = preferences as { version?: unknown; entries?: unknown } | null | undefined;
  if (shape === undefined || shape === null) {
    // Nothing is stored at all — there is provably nothing to preserve, so
    // the patch is the complete resolved (default) layout with exactly one
    // collapse change: the generalization of the missing-entry rule below.
    const resolved = resolveModuleLayout(preferences);
    if (!resolved.entries.some((entry) => entry.id === id)) return null;
    return {
      version: 1,
      entries: resolved.entries.map((entry) => ({
        ...entry,
        ...(entry.id === id ? { collapsed } : {}),
      })),
    };
  }
  if (typeof shape !== "object" || shape.version !== 1 || !Array.isArray(shape.entries)) {
    return null;
  }

  // The stored document is JSON end to end (it arrived from the backend's
  // serde serializer), so the JSON round trip is lossless for every field the
  // patch must preserve — and detaches the patch from the live state object.
  const document = JSON.parse(JSON.stringify(preferences)) as ModuleLayout;
  let found = false;
  for (const entry of document.entries) {
    if (entry.id === id && !found) {
      found = true;
      entry.collapsed = collapsed;
    }
  }
  if (!found) {
    const resolvedEntry = resolveModuleLayout(preferences).entries.find(
      (entry) => entry.id === id,
    );
    if (!resolvedEntry) return null;
    document.entries.push({
      id,
      order: resolvedEntry.order,
      colSpan: resolvedEntry.colSpan,
      collapsed,
    });
  }
  return document;
}
