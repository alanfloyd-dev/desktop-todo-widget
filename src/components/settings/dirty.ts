/**
 * Unsaved-change detection for the Settings panel (S1-B).
 *
 * The panel edits a draft of every persisted preference and writes the whole
 * draft back on Save. `settingsDraftIsDirty` answers "would Save change
 * anything?" by comparing the draft with the authoritative settings the panel
 * opened on, field by field, under the same normalization the backend applies
 * when it persists:
 *
 * - fields the backend trims on save (`displayName`, `githubUsername`, the
 *   weather label/timezone/country/admin1) compare trimmed, so trailing
 *   whitespace alone is not a change;
 * - the language preference compares normalized, exactly as the draft is
 *   initialized;
 * - module visibility compares resolved through the registry policy, so a
 *   legacy or hand-shaped stored object reads the same way the draft does.
 *
 * UI-only state — the active category, which appearance profile is being
 * edited, weather search results, update-check phases, diagnostics — is not
 * part of the snapshot and can never make the panel dirty. Nothing here
 * writes anything: it is a pure read of two plain objects.
 */
import { normalizeLanguage, type Language } from "../../i18n";
import { MODULE_REGISTRY, resolveModuleVisibility } from "../../modules/registry";
import type {
  AppearanceProfiles,
  AppearanceSettings,
  ModuleVisibility,
  ProductSettings,
  ProductWindowMode,
  QuickLink,
  TemperatureUnit,
} from "../../types";

/**
 * The draft fields the Save patch persists — and only those. The shape is
 * the Settings panel's `save` payload minus nothing and plus nothing, so the
 * dirty check and the write stay the same set of fields by construction.
 */
export interface SettingsDraftSnapshot {
  dayRollover: string;
  weatherLocationLabel: string;
  weatherLatitude: number | null;
  weatherLongitude: number | null;
  weatherTimezone: string;
  weatherCountry: string;
  weatherAdmin1: string;
  temperatureUnit: TemperatureUnit;
  language: Language;
  appearance: string;
  appearanceProfiles: AppearanceProfiles;
  displayName: string;
  avatarAssetId: string | null;
  quickLinks: readonly QuickLink[];
  moduleVisibility: ModuleVisibility;
  githubUsername: string;
}

const PROFILE_MODES: readonly ProductWindowMode[] = ["sidebar", "floating", "desktop"];

/**
 * Projects the authoritative settings onto the snapshot shape, applying the
 * same normalization the draft side is compared under.
 */
export function settingsSnapshot(settings: ProductSettings): SettingsDraftSnapshot {
  return {
    dayRollover: settings.dayRollover,
    weatherLocationLabel: settings.weatherLocationLabel,
    weatherLatitude: settings.weatherLatitude,
    weatherLongitude: settings.weatherLongitude,
    weatherTimezone: settings.weatherTimezone,
    weatherCountry: settings.weatherCountry,
    weatherAdmin1: settings.weatherAdmin1,
    temperatureUnit: settings.temperatureUnit,
    language: normalizeLanguage(settings.language),
    appearance: settings.appearance,
    appearanceProfiles: settings.appearanceProfiles,
    displayName: settings.displayName,
    avatarAssetId: settings.avatarAssetId,
    quickLinks: settings.quickLinks,
    moduleVisibility: resolveModuleVisibility(settings.moduleVisibility),
    githubUsername: settings.githubUsername,
  };
}

/** Every persisted member of one appearance profile, compared by value. */
function appearanceEqual(a: AppearanceSettings, b: AppearanceSettings): boolean {
  return (
    a.backgroundType === b.backgroundType &&
    a.solidColor === b.solidColor &&
    a.glassTintColor === b.glassTintColor &&
    a.glassTintOpacity === b.glassTintOpacity &&
    a.blurPx === b.blurPx &&
    a.overlayStrength === b.overlayStrength &&
    a.gradientStartColor === b.gradientStartColor &&
    a.gradientEndColor === b.gradientEndColor &&
    a.gradientAngle === b.gradientAngle &&
    a.imageAssetId === b.imageAssetId &&
    a.imageFit === b.imageFit &&
    a.imagePosition === b.imagePosition &&
    a.backgroundOpacity === b.backgroundOpacity &&
    a.textContrast === b.textContrast &&
    a.customTextColor === b.customTextColor &&
    a.sampledLuminance === b.sampledLuminance
  );
}

/** Quick Links compare by position: identity, name, and URL, in display order. */
function quickLinksEqual(a: readonly QuickLink[], b: readonly QuickLink[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((link, index) => {
    const other = b[index];
    return link.id === other.id && link.name === other.name && link.url === other.url;
  });
}

/** Only the registry's known module ids are compared, like the resolver reads them. */
function visibilityEqual(a: ModuleVisibility, b: ModuleVisibility): boolean {
  return MODULE_REGISTRY.every(({ id }) => a[id] === b[id]);
}

/**
 * Whether saving `draft` would persist anything different from `original`.
 *
 * Both sides are compared under the backend's save-time normalization, so
 * restoring an edited value to its original clears the result, and whitespace
 * the backend would strip never counts as a change.
 */
export function settingsDraftIsDirty(
  draft: SettingsDraftSnapshot,
  original: SettingsDraftSnapshot,
): boolean {
  if (draft.dayRollover !== original.dayRollover) return true;
  if (draft.weatherLocationLabel.trim() !== original.weatherLocationLabel.trim()) return true;
  if (draft.weatherLatitude !== original.weatherLatitude) return true;
  if (draft.weatherLongitude !== original.weatherLongitude) return true;
  if (draft.weatherTimezone.trim() !== original.weatherTimezone.trim()) return true;
  if (draft.weatherCountry.trim() !== original.weatherCountry.trim()) return true;
  if (draft.weatherAdmin1.trim() !== original.weatherAdmin1.trim()) return true;
  if (draft.temperatureUnit !== original.temperatureUnit) return true;
  if (normalizeLanguage(draft.language) !== normalizeLanguage(original.language)) return true;
  if (draft.appearance !== original.appearance) return true;
  if (draft.displayName.trim() !== original.displayName.trim()) return true;
  if (draft.avatarAssetId !== original.avatarAssetId) return true;
  if (draft.githubUsername.trim() !== original.githubUsername.trim()) return true;
  if (!quickLinksEqual(draft.quickLinks, original.quickLinks)) return true;
  if (!visibilityEqual(draft.moduleVisibility, original.moduleVisibility)) return true;
  for (const mode of PROFILE_MODES) {
    if (!appearanceEqual(draft.appearanceProfiles[mode], original.appearanceProfiles[mode])) {
      return true;
    }
  }
  return false;
}
