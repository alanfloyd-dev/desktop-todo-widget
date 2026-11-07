<script setup lang="ts">
import { nextTick, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../../i18n";
import type { LocationCandidate, TemperatureUnit, WeatherViewState } from "../../types";

/**
 * The Weather category: location search, the selected location's display, the
 * temperature unit, the data attribution and the manual refresh entry.
 *
 * It renders inside the `v-show` tabpanel `SettingsPanel` owns, so it mounts
 * and unmounts with the Settings panel rather than with the active category,
 * and the transient search state below lives for exactly one panel session.
 * The split with the parent follows the panel's ownership rule: everything
 * persisted — the six location fields and the temperature unit — stays a
 * SettingsPanel draft (the dirty snapshot and Save read them there); this
 * component owns only the search UI and reports every persisted change back
 * through events instead of touching the draft itself.
 */
const props = defineProps<{
  /** Persisted location label — display only here; the parent owns the draft. */
  weatherLocationLabel: string;
  weatherTimezone: string;
  weatherCountry: string;
  weatherAdmin1: string;
  temperatureUnit: TemperatureUnit;
  /** Live module view state, shared with the dashboard card. */
  weather: WeatherViewState;
  /** The parent's reply to the last manual refresh (shared message source). */
  weatherMessage: string;
  /**
   * Deep link (the weather card's "Set location"): the panel opened directly
   * onto this category, so the location search is the entry point and receives
   * the initial focus.
   */
  focusWeather: boolean;
}>();

const emit = defineEmits<{
  selectLocation: [candidate: LocationCandidate];
  updateTemperatureUnit: [unit: TemperatureUnit];
  refreshWeather: [];
}>();

const { t } = useI18n();

/**
 * Transient search state. Never persisted and never part of the dirty
 * snapshot — a half-typed query or a stale result list must not make the
 * panel claim it has unsaved changes.
 */
const locationQuery = ref("");
const candidates = ref<LocationCandidate[]>([]);
const locationSearching = ref(false);
const locationError = ref("");
/** The section, for the deep link's scrollIntoView. */
const weatherSection = ref<HTMLElement | null>(null);
const locationInput = ref<HTMLInputElement | null>(null);

function weatherErrorLabel(reason: unknown) {
  const kind = typeof reason === "object" && reason && "kind" in reason
    ? String((reason as { kind: unknown }).kind)
    : String(reason);
  if (kind.includes("Timeout")) return t("settings.error.locationTimeout");
  if (kind.includes("InvalidResponse")) return t("settings.error.locationInvalidResponse");
  return t("settings.error.locationUnavailable");
}

async function searchLocation() {
  const query = locationQuery.value.trim();
  locationError.value = "";
  candidates.value = [];
  if (query.length < 2) {
    locationError.value = t("settings.error.locationQueryTooShort");
    return;
  }
  locationSearching.value = true;
  try {
    candidates.value = await invoke<LocationCandidate[]>("search_weather_locations", { query });
    if (!candidates.value.length) locationError.value = t("settings.error.locationNoMatch");
  } catch (reason) {
    locationError.value = weatherErrorLabel(reason);
  } finally {
    locationSearching.value = false;
  }
}

/**
 * Chooses a candidate: the six persisted fields are the parent's draft (it
 * writes them all on this event), while resetting the search UI happens here
 * because the query and the result list are this component's own state.
 */
function selectLocation(candidate: LocationCandidate) {
  emit("selectLocation", candidate);
  candidates.value = [];
  locationQuery.value = "";
  locationError.value = "";
}

function candidateDetail(candidate: LocationCandidate) {
  return [candidate.admin1, candidate.country].filter(Boolean).join(", ");
}

function onTemperatureChange(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (value === "celsius" || value === "fahrenheit") emit("updateTemperatureUnit", value);
}

async function openAttribution() {
  try {
    await invoke("open_weather_attribution");
  } catch {
    locationError.value = t("settings.error.openAttribution");
  }
}

onMounted(async () => {
  // Deep links (the weather card's "Set location") opened this panel on the
  // weather category — `initialCategory` chose it before first render, so the
  // target is visible and `scrollIntoView` below never runs against a hidden
  // panel. The tabpanel is the parent's `v-show`, so this runs exactly once
  // per panel session, on first mount.
  if (props.focusWeather) {
    await nextTick();
    weatherSection.value?.scrollIntoView({ block: "start" });
    locationInput.value?.focus();
  }
});
</script>

<template>
  <section ref="weatherSection" class="weather-settings" aria-labelledby="weather-settings-heading">
    <div class="settings-section-heading">
    <span>
      <strong id="weather-settings-heading">{{ t("settings.weatherLocation") }}</strong>
      <small>{{ t("settings.weatherLocationHint") }}</small>
    </span>
    </div>
    <div class="weather-search-row">
      <input
        ref="locationInput"
        v-model="locationQuery"
        type="search"
        :placeholder="t('settings.weatherSearchPlaceholder')"
        autocomplete="off"
        @keydown.enter.prevent="searchLocation"
      />
      <button type="button" :disabled="locationSearching" @click="searchLocation">
        {{ locationSearching ? t("settings.weatherSearching") : t("settings.weatherSearch") }}
      </button>
    </div>
    <p v-if="locationError" class="settings-error" role="status">{{ locationError }}</p>
    <div v-if="candidates.length" class="weather-candidates" :aria-label="t('settings.weatherResultsAriaLabel')">
      <button
        v-for="candidate in candidates"
        :key="`${candidate.latitude}:${candidate.longitude}:${candidate.timezone}`"
        type="button"
        @click="selectLocation(candidate)"
      >
        <strong>{{ candidate.label }}</strong>
        <small>{{ candidateDetail(candidate) || candidate.timezone }}</small>
      </button>
    </div>
    <div class="settings-row weather-selected">
      <span><strong>{{ t("settings.weatherSelected") }}</strong><small>{{ t("settings.weatherSelectedHint") }}</small></span>
      <span class="selected-location">
        <strong>{{ weatherLocationLabel || t("settings.weatherNotConfigured") }}</strong>
        <small v-if="weatherLocationLabel">
          {{ [weatherAdmin1, weatherCountry].filter(Boolean).join(", ") || weatherTimezone }}
        </small>
      </span>
    </div>
    <label class="settings-row">
      <span><strong>{{ t("settings.temperature") }}</strong><small>{{ t("settings.temperatureHint") }}</small></span>
      <select :value="temperatureUnit" @change="onTemperatureChange">
        <option value="celsius">{{ t("settings.celsius") }}</option>
        <option value="fahrenheit">{{ t("settings.fahrenheit") }}</option>
      </select>
    </label>
    <div class="settings-row weather-provider-row">
      <span><strong>{{ t("settings.weatherData") }}</strong><small>{{ t("settings.weatherAttributionHint") }}</small></span>
      <button type="button" @click="openAttribution">Open-Meteo ↗</button>
    </div>
    <div class="weather-refresh-row">
      <button
        type="button"
        :disabled="!weather.configured || weather.refreshing"
        @click="$emit('refreshWeather')"
      >
        {{ weather.refreshing ? t("settings.refreshing") : t("settings.refreshNow") }}
      </button>
      <small v-if="weatherMessage" role="status">{{ weatherMessage }}</small>
    </div>
  </section>
</template>
