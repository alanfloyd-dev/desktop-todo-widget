<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "../i18n";
import type { WeatherCondition, WeatherViewState } from "../types";

const props = defineProps<{ weather: WeatherViewState }>();
defineEmits<{ configure: [] }>();
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
  <div class="weather-display" aria-live="polite">
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
  </div>
</template>
