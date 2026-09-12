/**
 * Frontend i18n layer.
 *
 * One small, central place where product copy is resolved. Components never
 * branch on the locale themselves: they ask for a stable key through `t()`.
 *
 * The layer deliberately stops at:
 *   - no plural rules (v1 copy is written to avoid needing them, or uses
 *     `Intl` directly),
 *   - no locale-aware sorting or number formats beyond `Intl` defaults,
 *   - no lazy chunking; both catalogs are a few kilobytes.
 */
import { computed, inject, provide, ref, type ComputedRef, type InjectionKey } from "vue";
import { FALLBACK_LOCALE, en, zhHans, type Locale } from "./catalog";

/**
 * The persisted `settings.language` values.
 *
 * `system` follows the operating system locale, resolved by
 * `resolveActiveLocale`. The other two force a language.
 */
export type Language = "system" | "en" | "zh-Hans";

export const LANGUAGE_OPTIONS: Language[] = ["system", "en", "zh-Hans"];

/** Normalizes an unvalidated persisted value to a supported preference. */
export function normalizeLanguage(value: unknown): Language {
  return value === "en" || value === "zh-Hans" ? value : "system";
}

/**
 * True when a locale tag names a Simplified Chinese UI.
 *
 * Mirrors `src-tauri/src/locale.rs` so the frontend and the native menus make
 * the same decision. v1 ships no Traditional Chinese catalog, so `zh-TW`,
 * `zh-HK`, and `zh-Hant` fall back to English.
 */
export function isSimplifiedChineseLocale(systemLocale: string): boolean {
  const trimmed = systemLocale.trim().split(/[.@]/)[0];
  if (!trimmed) return false;
  const subtags = trimmed.split(/[-_]/);
  if (subtags[0]?.toLowerCase() !== "zh") return false;
  const rest = subtags.slice(1).map((subtag) => subtag.toLowerCase());
  if (rest.includes("hant")) return false;
  if (rest.includes("hans")) return true;
  return rest.includes("cn") || rest.includes("sg");
}

/** Resolves the preference plus the system locale to a concrete language. */
export function resolveActiveLocale(language: Language, systemLocale: string): Locale {
  if (language === "en") return "en";
  if (language === "zh-Hans") return "zh-Hans";
  return isSimplifiedChineseLocale(systemLocale) ? "zh-Hans" : FALLBACK_LOCALE;
}

export type TranslateParams = Record<string, string | number>;

const catalogs: Record<Locale, Record<string, string>> = {
  en,
  "zh-Hans": zhHans,
};

/**
 * Looks up a key, falling back to English and finally to the key itself.
 *
 * Returning the key is intentional: a missing string is visible in review
 * instead of rendering as blank product copy.
 */
export function translate(locale: Locale, key: string, params?: TranslateParams): string {
  const template = catalogs[locale][key] ?? catalogs[FALLBACK_LOCALE][key] ?? key;
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

export interface I18n {
  /** The persisted preference. */
  language: ComputedRef<Language>;
  /** The language actually rendering, after `system` is resolved. */
  locale: ComputedRef<Locale>;
  /** BCP-47 tag for `Intl` formatting. */
  intlLocale: ComputedRef<string>;
  t: (key: string, params?: TranslateParams) => string;
  setLanguage: (language: Language) => void;
  setSystemLocale: (systemLocale: string) => void;
}

export function createI18n(initialLanguage: Language, initialSystemLocale: string): I18n {
  const language = ref<Language>(initialLanguage);
  const systemLocale = ref(initialSystemLocale);
  const locale = computed(() =>
    resolveActiveLocale(language.value, systemLocale.value),
  );
  return {
    language: computed(() => language.value),
    locale,
    intlLocale: computed(() => locale.value),
    t: (key, params) => translate(locale.value, key, params),
    setLanguage: (value) => {
      language.value = value;
    },
    setSystemLocale: (value) => {
      systemLocale.value = value;
    },
  };
}

export const I18N_KEY: InjectionKey<I18n> = Symbol("alan-desktop-i18n");

/** Provides the i18n instance to every descendant component. */
export function provideI18n(i18n: I18n): I18n {
  provide(I18N_KEY, i18n);
  return i18n;
}

/**
 * Reads the provided i18n instance.
 *
 * Throws instead of silently falling back to English: a component mounted
 * outside `App.vue` would otherwise render untranslated copy with no signal.
 */
export function useI18n(): I18n {
  const i18n = inject(I18N_KEY);
  if (!i18n) throw new Error("i18n was not provided by an ancestor component");
  return i18n;
}

/** The display name of a language preference, in the currently active language. */
export function languageOptionLabel(language: Language, t: I18n["t"]): string {
  if (language === "en") return t("settings.language.option.en");
  if (language === "zh-Hans") return t("settings.language.option.zhHans");
  return t("settings.language.option.system");
}
