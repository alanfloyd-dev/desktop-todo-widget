<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "../i18n";
import type { WeatherCondition, WeatherViewState } from "../types";

const props = defineProps<{ weather: WeatherViewState; weatherMessage: string }>();
defineEmits<{ configure: []; refresh: [] }>();
const { t } = useI18n();

const conditionLabels: Record<WeatherCondition, string> = {
  clear: "weather.condition.clear",
  "mainly-clear": "weather.condition.mainly-clear",
  "partly-cloudy": "weather.condition.partly-cloudy",
  cloudy: "weather.condition.cloudy",
  fog: "weather.condition.fog",
  drizzle: "weather.condition.drizzle",
  rain: "weather.condition.rain",
  snow: "weather.condition.snow",
  showers: "weather.condition.showers",
  thunderstorm: "weather.condition.thunderstorm",
  unknown: "weather.condition.unknown",
};

const unitSymbol = computed(() =>
  props.weather.snapshot?.temperatureUnit === "fahrenheit" ? "°F" : "°C",
);

function temperature(value: number) {
  return `${Math.round(value)}°`;
}

function ageLabel(seconds: number | null) {
  if (seconds === null) return t("weather.ageUnavailable");
  if (seconds < 60 * 60) {
    return t("weather.ageMinutes", { minutes: Math.max(1, Math.round(seconds / 60)) });
  }
  if (seconds < 24 * 60 * 60) {
    return t("weather.ageHours", { hours: Math.round(seconds / 3600) });
  }
  return t("weather.ageYesterday");
}
</script>

<template>
  <div class="weather-display" :class="{ 'has-refresh': weather.configured }" aria-live="polite">
    <!--
      The manual refresh affordance: an icon-only control that rides the exact
      same single-flight refresh path as the Settings entry (`refresh_weather`
      with `manual: true`). It exists only when a location is configured —
      without a location there is nothing to refresh — and it is disabled for
      the duration of the in-flight refresh, so the spinning icon and the
      disabled semantics always agree.
    -->
    <button
      v-if="weather.configured"
      type="button"
      class="weather-refresh"
      :class="{ refreshing: weather.refreshing }"
      :disabled="weather.refreshing"
      :aria-label="t('weather.refreshAriaLabel')"
      :title="t('weather.refreshAriaLabel')"
      @click="$emit('refresh')"
    >
      <svg class="weather-refresh-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="23 4 23 10 17 10" />
          <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
        </g>
      </svg>
    </button>

    <template v-if="!weather.configured">
      <p class="weather-empty">{{ t("weather.empty") }}</p>
      <button type="button" class="weather-configure" @click="$emit('configure')">
        {{ t("weather.setLocation") }}
      </button>
    </template>

    <template v-else-if="weather.snapshot && weather.cacheStatus !== 'very-stale'">
      <p class="weather-primary">
        <span>{{ temperature(weather.snapshot.temperature) }}{{ unitSymbol.slice(1) }}</span>
        <span>{{ t(conditionLabels[weather.snapshot.condition]) }}</span>
      </p>
      <p class="weather-secondary">
        {{ temperature(weather.snapshot.dailyLow) }} / {{ temperature(weather.snapshot.dailyHigh) }}
        <span>·</span>
        {{ t("weather.rain", { percent: weather.snapshot.precipitationProbability }) }}
      </p>
      <p v-if="weather.cacheStatus === 'stale'" class="weather-age">
        {{ ageLabel(weather.cacheAgeSeconds) }}
      </p>
    </template>

    <template v-else>
      <p class="weather-empty">{{ t("weather.unavailable") }}</p>
      <p v-if="weather.snapshot" class="weather-age">{{ ageLabel(weather.cacheAgeSeconds) }}</p>
    </template>

    <!--
      The status of the last manual refresh, shared with the Settings weather
      section (the same message source). Success is quiet, failure keeps the
      previous snapshot on the card above this line.
    -->
    <p v-if="weatherMessage" class="weather-status" role="status">{{ weatherMessage }}</p>
  </div>
</template>
