/**
 * The Settings panel's category model (S1-B).
 *
 * Pure data and pure functions: no Vue, no Tauri, no DOM — the same
 * loadable-anywhere rule as the module registry, so the category list, the
 * tab keyboard model, and the deep-link mapping stay directly unit-testable.
 *
 * The four categories and their order are the approved S1-A/S1-B design
 * (decision D6) with S2-P2 applied: Weather is no longer a category of its
 * own — its configuration is a section of Modules — so a Weather deep link
 * redirects there, and the navigation order reads General first. There is
 * deliberately no GitHub, Quick Links, Updates, or Developer top-level
 * category: those controls live inside Modules and About.
 */

/** The closed set of Settings categories. A new category is a new member here. */
export type SettingsCategoryId = "appearance" | "general" | "modules" | "about";

export interface SettingsCategory {
  id: SettingsCategoryId;
  /** Catalog key of the category's label; exists in both languages. */
  labelKey: string;
}

/**
 * The categories in their fixed navigation order. General leads since the
 * S2-P2 order correction (the panel opens on Appearance by default, but the
 * navigation reads General → Appearance → Modules → About).
 */
export const SETTINGS_CATEGORIES: readonly SettingsCategory[] = [
  { id: "general", labelKey: "settings.category.general" },
  { id: "appearance", labelKey: "settings.category.appearance" },
  { id: "modules", labelKey: "settings.category.modules" },
  { id: "about", labelKey: "settings.category.about" },
];

/** Narrows an unvalidated value (a `<select>` value, for example) to a category id. */
export function isSettingsCategoryId(value: unknown): value is SettingsCategoryId {
  return SETTINGS_CATEGORIES.some((category) => category.id === value);
}

/**
 * The category a contextual deep-link opens.
 *
 * GitHub's setup entry lands on Modules, where the username field lives
 * (decision D2). Weather's card entry lands on Modules too since S2-P2 — the
 * Weather category was merged into it — so the legacy `weather` intent opens
 * Modules and still carries its own focus flag to the Weather section inside.
 * Everything else opens on Appearance.
 */
export function initialCategory(focusWeather: boolean, focusGithub: boolean): SettingsCategoryId {
  if (focusWeather) return "modules";
  if (focusGithub) return "modules";
  return "appearance";
}

/**
 * Shared stepping core for both orientation models: the category `delta`
 * positions away from `current`, wrapping around the fixed order. A current
 * id outside the category set steps to nothing, exactly like the key checks
 * below refuse unknown keys.
 */
function stepCategory(
  current: SettingsCategoryId,
  delta: number,
): SettingsCategoryId | null {
  const ids = SETTINGS_CATEGORIES.map((category) => category.id);
  const index = ids.indexOf(current);
  if (index === -1) return null;
  return ids[(index + delta + ids.length) % ids.length];
}

/**
 * Shared ends core: Home/End jump to the first/last category in both
 * orientations. The same invalid-current guard as stepping applies, so an
 * unknown current id refuses every key uniformly.
 */
function jumpToEndCategory(
  current: SettingsCategoryId,
  key: string,
): SettingsCategoryId | null {
  if (key !== "Home" && key !== "End") return null;
  const ids = SETTINGS_CATEGORIES.map((category) => category.id);
  if (ids.indexOf(current) === -1) return null;
  return key === "Home" ? ids[0] : ids[ids.length - 1];
}

/**
 * The tab a keypress moves to, or `null` for keys the tablist ignores.
 *
 * Horizontal tabs follow the WAI-ARIA tabs pattern: ArrowLeft/ArrowRight are
 * horizontal-wrapping (the category order is left-to-right), Home/End jump to
 * the ends, and every other key — including the vertical arrows — is left for
 * the browser and the screen reader.
 */
export function nextTabCategory(
  current: SettingsCategoryId,
  key: string,
): SettingsCategoryId | null {
  if (key === "ArrowRight") return stepCategory(current, 1);
  if (key === "ArrowLeft") return stepCategory(current, -1);
  return jumpToEndCategory(current, key);
}

/**
 * The vertical twin the navigation rail uses (S2-P1): ArrowUp/ArrowDown step
 * through the same fixed category order with the same wraparound and the same
 * Home/End ends, and the orthogonal horizontal arrows do nothing. One shared
 * core with the horizontal model, so the two presentations can never disagree
 * about order, wraparound, or ends.
 */
export function nextVerticalTabCategory(
  current: SettingsCategoryId,
  key: string,
): SettingsCategoryId | null {
  if (key === "ArrowDown") return stepCategory(current, 1);
  if (key === "ArrowUp") return stepCategory(current, -1);
  return jumpToEndCategory(current, key);
}
