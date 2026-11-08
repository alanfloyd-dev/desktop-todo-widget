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
 * persisted fields stayed SettingsPanel drafts. These pins hold the boundary
 * in place: the child owns the transient search UI and the two native
 * weather-lookup commands, the parent keeps the dirty snapshot, Save payload,
 * draft mutations and the `v-show` tabpanel, and the refresh chain stays
 * child → parent → App controller.
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

  it("the parent passes every binding exactly once on the child element", () => {
    const usages = settingsPanelVue.match(/<WeatherPanel\b[^>]*>/g);
    expect(usages?.length).toBe(1);
    const tag = usages![0];
    // No conditional mounting may ride on the element itself.
    expect(tag).not.toContain("v-if");
    expect(tag).not.toContain("v-show");
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

describe("WeatherPanel extraction: lifecycle and deep link (S1-E2)", () => {
  it("the parent mounts WeatherPanel unconditionally inside its v-show tabpanel", () => {
    // The wrapper stays the parent's and the child carries no `v-if`: search
    // state must survive switching categories, exactly like AboutPanel's
    // checker state does.
    expect(settingsPanelVue).toMatch(
      /<section\s+v-show="activeCategory === 'weather'"\s+id="settings-panel-weather"\s+class="settings-category-panel"\s+role="tabpanel"\s+aria-labelledby="settings-tab-weather"\s*>(?:\s*<!--[\s\S]*?-->\s*)*<WeatherPanel\b/,
    );
    expect(settingsPanelVue.match(/v-show="activeCategory === '/g)?.length).toBe(5);
    expect(settingsPanelVue.match(/role="tabpanel"/g)?.length).toBe(5);
  });

  it("the weather deep link still opens the category and focuses the search input", () => {
    // initialCategory keeps choosing Weather before first render (so the
    // tabpanel is visible when the child mounts), and the child owns the
    // scrollIntoView + focus pair on its own refs.
    expect(settingsPanelVue).toContain("initialCategory(props.focusWeather, props.focusGithub)");
    expect(settingsPanelVue.match(/props\.focusWeather/g)?.length).toBe(1);
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
      /if \(props\.focusGithub\) \{\s*await nextTick\(\);\s*githubRow\.value\?\.scrollIntoView\(\{ block: "start" \}\);\s*githubInput\.value\?\.focus\(\);\s*\}/,
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
