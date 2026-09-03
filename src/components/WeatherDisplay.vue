<script setup lang="ts">
import { computed } from "vue";
import type { WeatherCondition, WeatherViewState } from "../types";

const props = defineProps<{ weather: WeatherViewState }>();
defineEmits<{ configure: [] }>();

const conditionLabels: Record<WeatherCondition, string> = {
  clear: "Clear",
  "mainly-clear": "Mainly clear",
  "partly-cloudy": "Partly cloudy",
  cloudy: "Cloudy",
  fog: "Fog",
  drizzle: "Drizzle",
  rain: "Rain",
  snow: "Snow",
  showers: "Showers",
  thunderstorm: "Thunderstorm",
  unknown: "Conditions unavailable",
};

const unitSymbol = computed(() =>
  props.weather.snapshot?.temperatureUnit === "fahrenheit" ? "°F" : "°C",
);

function temperature(value: number) {
  return `${Math.round(value)}°`;
}

function ageLabel(seconds: number | null) {
  if (seconds === null) return "Update unavailable";
  if (seconds < 60 * 60) return `Updated ${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 24 * 60 * 60) return `Updated ${Math.round(seconds / 3600)}h ago`;
  return "Last updated yesterday";
}
</script>

<template>
  <div class="weather-display" aria-live="polite">
    <template v-if="!weather.configured">
      <p class="weather-empty">WEATHER —</p>
      <button type="button" class="weather-configure" @click="$emit('configure')">
        Set location
      </button>
    </template>

    <template v-else-if="weather.snapshot && weather.cacheStatus !== 'very-stale'">
      <p class="weather-primary">
        <span>{{ temperature(weather.snapshot.temperature) }}{{ unitSymbol.slice(1) }}</span>
        <span>{{ conditionLabels[weather.snapshot.condition] }}</span>
      </p>
      <p class="weather-secondary">
        {{ temperature(weather.snapshot.dailyLow) }} / {{ temperature(weather.snapshot.dailyHigh) }}
        <span>·</span>
        Rain {{ weather.snapshot.precipitationProbability }}%
      </p>
      <p v-if="weather.cacheStatus === 'stale'" class="weather-age">
        {{ ageLabel(weather.cacheAgeSeconds) }}
      </p>
    </template>

    <template v-else>
      <p class="weather-empty">WEATHER UNAVAILABLE</p>
      <p v-if="weather.snapshot" class="weather-age">{{ ageLabel(weather.cacheAgeSeconds) }}</p>
    </template>
  </div>
</template>
