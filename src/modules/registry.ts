/**
 * The typed registry of product modules and the pure visibility policy.
 *
 * This is the single definition of which modules exist, what they label
 * themselves as, and how they render. It is deliberately data only: no Vue,
 * no Tauri, no DOM — the same loadable-anywhere rule as the module bridges —
 * so the policy below stays directly unit-testable and B2 can persist the
 * exact shape it resolves without translation.
 *
 * Scope rules (B0 audit):
 * - The registry describes the four product-surface modules. Settings and the
 *   updater are application capabilities, not modules, and have no entry here.
 * - `surface` records how a module renders, which is an audited fact, not a
 *   layout instruction: Todo, Weather, and Shortcuts are dashboard sections of
 *   `ProductContent`; Review is an on-demand overlay whose toggle governs its
 *   entry point rather than a card.
 * - Rendering, persistence, and the settings UI are later B slices; nothing
 *   here is wired to the product surface yet.
 */

/** The closed set of product modules. A new module is a new member here. */
export type ModuleId = "todo" | "weather" | "shortcuts" | "review";

/** How a module presents on the product surface. */
export type ModuleSurface = "dashboard" | "overlay";

export interface ModuleDescriptor {
  id: ModuleId;
  /** Catalog key of the module's label; exists in both languages. */
  labelKey: string;
  surface: ModuleSurface;
  /**
   * Every module ships enabled: the literal `true` type is what makes
   * "all modules enabled" a compile-time property of the registry instead of
   * a value someone can flip.
   */
  defaultEnabled: true;
}

export const MODULE_REGISTRY = [
  { id: "todo", labelKey: "modules.todo", surface: "dashboard", defaultEnabled: true },
  { id: "weather", labelKey: "modules.weather", surface: "dashboard", defaultEnabled: true },
  { id: "shortcuts", labelKey: "modules.shortcuts", surface: "dashboard", defaultEnabled: true },
  { id: "review", labelKey: "modules.review", surface: "overlay", defaultEnabled: true },
] as const satisfies readonly ModuleDescriptor[];

/** Effective module visibility: exactly the four known ids, no others. */
export type ModuleVisibility = Readonly<Record<ModuleId, boolean>>;

/**
 * The visibility every installation starts from.
 *
 * All-enabled is not just a product default, it is the upgrade contract: an
 * existing profile whose document predates module preferences must keep the
 * exact v1.2.1 surface, so the absence of a stored preference always
 * resolves to enabled.
 */
export const DEFAULT_MODULE_VISIBILITY: ModuleVisibility = resolveModuleVisibility();

/**
 * Resolves effective visibility from optional user preferences.
 *
 * Semantics: missing preferences (or missing individual keys) resolve to
 * enabled, explicit `false` disables exactly that module, explicit `true`
 * keeps it enabled, and unknown keys are ignored — only the four registry
 * ids are ever read. The input is never mutated, and the result is a fresh
 * object per call, so a caller mutating its copy can never reach the
 * registry, the default constant, or another caller's result.
 */
export function resolveModuleVisibility(
  preferences?: Partial<ModuleVisibility> | null,
): ModuleVisibility {
  const resolved = {} as Record<ModuleId, boolean>;
  for (const { id, defaultEnabled } of MODULE_REGISTRY) {
    resolved[id] = preferences?.[id] ?? defaultEnabled;
  }
  return resolved;
}

/** Whether a module is enabled under an already-resolved visibility. */
export function isModuleEnabled(visibility: ModuleVisibility, id: ModuleId): boolean {
  return visibility[id];
}
