import { describe, expect, it } from "vitest";
// Source-pinning contracts for the S1-E2 WeatherPanel extraction — the same
// `?raw` pattern as `visual.contract.test.ts` and the AboutPanel pins in
// `updateCheck.test.ts` (the Vitest node environment has no component
// mounting, so the boundary is pinned at the source level).
import settingsPanelVue from "../SettingsPanel.vue?raw";
import weatherPanelVue from "./WeatherPanel.vue?raw";
import modulesPanelVue from "./ModulesPanel.vue?raw";
import appVue from "../../App.vue?raw";

/**
 * S1-E2 moved the Weather category's controls into `WeatherPanel` while the
 * persisted fields stayed SettingsPanel drafts. S2-P2 then merged the Weather
 * category into Modules: `WeatherPanel` now renders inside `ModulesPanel`,
 * which passes every binding through untouched, and the deep link redirects
 * onto Modules. These pins hold the boundary in place: the child owns the
 * transient search UI and the two native weather-lookup commands, the parent
 * keeps the dirty snapshot, Save payload and draft mutations, the section
 * renders inside the modules tabpanel (no tabpanel of its own), and the
 * refresh chain stays child → ModulesPanel → parent → App controller.
 */

const WEATHER_DRAFT_FIELD_LINES = [
  "weatherLocationLabel: weatherLocationLabel.value,",
  "weatherLatitude: weatherLatitude.value,",
  "weatherLongitude: weatherLongitude.value,",
  "weatherTimezone: weatherTimezone.value,",
  "weatherCountry: weatherCountry.value,",
  "weatherAdmin1: weatherAdmin1.value,",
  "temperatureUnit: temperatureUnit.value,",
];

/** Every catalog key the Weather category renders (in the child, after E2). */
const WEATHER_I18N_KEYS = [
  "settings.weatherLocation",
  "settings.weatherLocationHint",
  "settings.weatherSearchPlaceholder",
  "settings.weatherSearching",
  "settings.weatherSearch",
  "settings.weatherResultsAriaLabel",
  "settings.weatherSelected",
  "settings.weatherSelectedHint",
  "settings.weatherNotConfigured",
  "settings.temperature",
  "settings.temperatureHint",
  "settings.celsius",
  "settings.fahrenheit",
  "settings.weatherData",
  "settings.weatherAttributionHint",
  "settings.refreshing",
  "settings.refreshNow",
  "settings.error.locationTimeout",
  "settings.error.locationInvalidResponse",
  "settings.error.locationUnavailable",
  "settings.error.locationQueryTooShort",
  "settings.error.locationNoMatch",
  "settings.error.openAttribution",
];

describe("WeatherPanel extraction: parent draft ownership (S1-E2)", () => {
  it("keeps every persisted weather field in the dirty snapshot and the Save payload", () => {
    // The exact draft-to-payload lines appear exactly twice: once in the
    // dirtyDraft projection and once in the save emit — the "dirty" and
    // "would write" sets stay identical by construction.
    for (const line of WEATHER_DRAFT_FIELD_LINES) {
      expect(settingsPanelVue.match(new RegExp(line.replace(/\./g, "\\."), "g"))?.length, line).toBe(2);
    }
  });

  it("writes all six location fields when the child reports a selection", () => {
    expect(settingsPanelVue).toMatch(
      /function selectLocation\(candidate: LocationCandidate\) \{\s*weatherLocationLabel\.value = candidate\.label;\s*weatherLatitude\.value = candidate\.latitude;\s*weatherLongitude\.value = candidate\.longitude;\s*weatherTimezone\.value = candidate\.timezone;\s*weatherCountry\.value = candidate\.country;\s*weatherAdmin1\.value = candidate\.admin1;\s*\}/,
    );
  });

  it("writes the temperature unit draft when the child reports a change", () => {
    expect(settingsPanelVue).toMatch(
      /function setTemperatureUnit\(unit: TemperatureUnit\) \{\s*temperatureUnit\.value = unit;\s*\}/,
    );
    expect(weatherPanelVue).toContain(
      '<select :value="temperatureUnit" @change="onTemperatureChange">',
    );
  });

  it("the child cannot reach the settings save path", () => {
    for (const forbidden of ["settingsSnapshot", "settingsDraftIsDirty", "dirtyDraft", "update_product_settings"]) {
      expect(weatherPanelVue, forbidden).not.toContain(forbidden);
    }
  });
});

describe("WeatherPanel extraction: child interface (S1-E2)", () => {
  it("receives the required display state as explicit typed props", () => {
    for (const prop of [
      "weatherLocationLabel: string;",
      "weatherTimezone: string;",
      "weatherCountry: string;",
      "weatherAdmin1: string;",
      "temperatureUnit: TemperatureUnit;",
      "weather: WeatherViewState;",
      "weatherMessage: string;",
      "focusWeather: boolean;",
    ]) {
      expect(weatherPanelVue, prop).toContain(prop);
    }
    expect(weatherPanelVue).toContain("defineProps<{");
  });

  it("reports persisted changes back through explicit typed events", () => {
    for (const event of [
      "selectLocation: [candidate: LocationCandidate];",
      "updateTemperatureUnit: [unit: TemperatureUnit];",
      "refreshWeather: [];",
    ]) {
      expect(weatherPanelVue, event).toContain(event);
    }
  });

  it("ModulesPanel passes every binding exactly once on the child element (S2-P2 composition)", () => {
    // WeatherPanel renders inside ModulesPanel since S2-P2; the usage and its
    // bindings moved there verbatim, and SettingsPanel no longer mounts it.
    const usages = modulesPanelVue.match(/<WeatherPanel\b[^>]*>/g);
    expect(usages?.length).toBe(1);
    const tag = usages![0];
    // No conditional mounting may ride on the element itself — the section
    // must render even when the Weather module is switched off, and the
    // search state must survive category switches like every other panel's.
    expect(tag).not.toContain("v-if");
    expect(tag).not.toContain("v-show");
    expect(tag).not.toContain("moduleVisibility.weather");
    for (const binding of [
      ":weather-location-label",
      ":weather-timezone",
      ":weather-country",
      ":weather-admin1",
      ":temperature-unit",
      ":weather",
      ":weather-message",
      ":focus-weather",
      "@select-location",
      "@update-temperature-unit",
      "@refresh-weather",
    ]) {
      expect(tag, binding).toContain(binding);
    }
    // SettingsPanel no longer references the child at all: the grandchild
    // chain is parent → ModulesPanel → WeatherPanel.
    expect(settingsPanelVue).not.toContain("<WeatherPanel");
  });

  it("renders from a single weather-settings root (class and refs preserved)", () => {
    expect(weatherPanelVue).toMatch(
      /<template>\s*<section ref="weatherSection" class="weather-settings" aria-labelledby="weather-settings-heading">/,
    );
    // Single root: the template ends where the root section closes.
    expect(weatherPanelVue).toMatch(
      /<\/section>\s*<\/template>\s*$/,
    );
  });

  it("keeps every Weather translation key in the child template", () => {
    // Template attribute values quote with single quotes; script strings with
    // double quotes — pin the key itself, not one quote style.
    for (const key of WEATHER_I18N_KEYS) {
      expect(weatherPanelVue, key).toMatch(new RegExp(`['"]${key}['"]`));
    }
  });
});

describe("WeatherPanel extraction: moved-as-a-unit responsibilities (S1-E2)", () => {
  it("the search and attribution responsibilities live in WeatherPanel, not SettingsPanel", () => {
    // A second owner would mean two search state machines or two attribution
    // entry points — the AboutPanel extraction's rule, applied here.
    for (const owned of [
      "search_weather_locations",
      "open_weather_attribution",
      "locationQuery",
      "locationSearching",
      "locationError",
      "weatherErrorLabel",
      "candidateDetail",
      "weatherSection",
      "locationInput",
    ]) {
      expect(weatherPanelVue, owned).toContain(owned);
      expect(settingsPanelVue, owned).not.toContain(owned);
    }
  });

  it("preserves the native command names and arguments verbatim", () => {
    expect(weatherPanelVue).toContain(
      'invoke<LocationCandidate[]>("search_weather_locations", { query })',
    );
    expect(weatherPanelVue).toContain('invoke("open_weather_attribution")');
  });
});

describe("WeatherPanel extraction: lifecycle and deep link (S1-E2, S2-P2)", () => {
  it("ModulesPanel mounts WeatherPanel unconditionally inside the modules tabpanel", () => {
    // Since S2-P2 the wrapper is ModulesPanel's fragment and the tabpanel is
    // still the parent's `v-show` — the weather tabpanel is gone, and the
    // child carries no `v-if`: search state must survive switching
    // categories, exactly like AboutPanel's checker state does.
    expect(settingsPanelVue.match(/v-show="activeCategory === '/g)?.length).toBe(4);
    expect(settingsPanelVue.match(/role="tabpanel"/g)?.length).toBe(4);
    // The section sits between the visibility group and the GitHub group —
    // the approved Modules-page order (visibility → weather → GitHub →
    // quick links).
    const resetEnd = modulesPanelVue.indexOf("settings-module-reset");
    const weatherUsage = modulesPanelVue.indexOf("<WeatherPanel");
    const githubHeading = modulesPanelVue.indexOf("settings.githubConfig.heading");
    expect(resetEnd).toBeGreaterThan(-1);
    expect(weatherUsage).toBeGreaterThan(resetEnd);
    expect(githubHeading).toBeGreaterThan(weatherUsage);
  });

  it("the weather deep link still opens Modules and focuses the search input", () => {
    // initialCategory redirects the legacy Weather flag onto Modules before
    // first render (so the section is visible when the children mount), and
    // the child owns the scrollIntoView + focus pair on its own refs. The
    // parent references the flag exactly once outside that call: the
    // `:focus-weather` pass-through binding.
    expect(settingsPanelVue).toContain("initialCategory(props.focusWeather, props.focusGithub)");
    expect(settingsPanelVue.match(/props\.focusWeather/g)?.length).toBe(1);
    expect(settingsPanelVue).toContain(':focus-weather="focusWeather"');
    expect(weatherPanelVue).toMatch(
      /if \(props\.focusWeather\) \{\s*await nextTick\(\);\s*weatherSection\.value\?\.scrollIntoView\(\{ block: "start" \}\);\s*locationInput\.value\?\.focus\(\);\s*\}/,
    );
  });

  it("the GitHub deep link's focus lives in the child that owns the row (since S1-E3)", () => {
    // S1-E3 moved the GitHub username row into ModulesPanel, and with it the
    // scrollIntoView + focus pair — the same split this file pins for the
    // weather link. The parent keeps only the initialCategory choice.
    expect(settingsPanelVue).not.toMatch(/githubRow|githubInput/);
    expect(modulesPanelVue).toMatch(
      /if \(props\.focusGithub && !props\.focusWeather\) \{\s*await nextTick\(\);\s*githubRow\.value\?\.scrollIntoView\(\{ block: "start" \}\);\s*githubInput\.value\?\.focus\(\);\s*\}/,
    );
  });

  it("refresh continues through the parent controller", () => {
    // Child button → parent re-emit → the App-level weather controller; the
    // child never talks to refresh_weather itself.
    expect(weatherPanelVue).toContain('@click="$emit(\'refreshWeather\')"');
    expect(weatherPanelVue).toContain(':disabled="!weather.configured || weather.refreshing"');
    expect(weatherPanelVue).not.toContain("refresh_weather");
    expect(settingsPanelVue).toContain('@refresh-weather="$emit(\'refreshWeather\')"');
    expect(appVue).toContain('@refresh-weather="refreshWeather(true)"');
  });
});
