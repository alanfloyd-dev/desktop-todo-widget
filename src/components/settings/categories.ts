/**
 * The Settings panel's category model (S1-B).
 *
 * Pure data and pure functions: no Vue, no Tauri, no DOM — the same
 * loadable-anywhere rule as the module registry, so the category list, the
 * tab keyboard model, and the deep-link mapping stay directly unit-testable.
 *
 * The five categories and their order are the approved S1-A/S1-B design
 * (decision D6). There is deliberately no GitHub, Quick Links, Updates, or
 * Developer top-level category: those controls live inside Modules and About.
 */

/** The closed set of Settings categories. A new category is a new member here. */
export type SettingsCategoryId = "appearance" | "general" | "modules" | "weather" | "about";

export interface SettingsCategory {
  id: SettingsCategoryId;
  /** Catalog key of the category's label; exists in both languages. */
  labelKey: string;
}

/** The categories in their fixed navigation order. */
export const SETTINGS_CATEGORIES: readonly SettingsCategory[] = [
  { id: "appearance", labelKey: "settings.category.appearance" },
  { id: "general", labelKey: "settings.category.general" },
  { id: "modules", labelKey: "settings.category.modules" },
  { id: "weather", labelKey: "settings.category.weather" },
  { id: "about", labelKey: "settings.category.about" },
];

/** Narrows an unvalidated value (a `<select>` value, for example) to a category id. */
export function isSettingsCategoryId(value: unknown): value is SettingsCategoryId {
  return SETTINGS_CATEGORIES.some((category) => category.id === value);
}

/**
 * The category a contextual deep-link opens.
 *
 * Weather's card entry lands on Weather; GitHub's setup entry lands on
 * Modules, where the username field lives (decision D2). Everything else
 * opens on Appearance.
 */
export function initialCategory(focusWeather: boolean, focusGithub: boolean): SettingsCategoryId {
  if (focusWeather) return "weather";
  if (focusGithub) return "modules";
  return "appearance";
}

/**
 * The tab a keypress moves to, or `null` for keys the tablist ignores.
 *
 * Horizontal tabs follow the WAI-ARIA tabs pattern: ArrowLeft/ArrowRight are
 * horizontal-wrapping (the category order is left-to-right), Home/End jump to
 * the ends, and every other key is left for the browser and the screen reader.
 */
export function nextTabCategory(
  current: SettingsCategoryId,
  key: string,
): SettingsCategoryId | null {
  const ids = SETTINGS_CATEGORIES.map((category) => category.id);
  const index = ids.indexOf(current);
  if (index === -1) return null;
  if (key === "ArrowRight") return ids[(index + 1) % ids.length];
  if (key === "ArrowLeft") return ids[(index - 1 + ids.length) % ids.length];
  if (key === "Home") return ids[0];
  if (key === "End") return ids[ids.length - 1];
  return null;
}
