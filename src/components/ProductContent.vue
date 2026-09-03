<script setup lang="ts">
import { computed } from "vue";
import { profileInitials } from "../appearance";
import type { ProductWindowMode, WeatherViewState } from "../types";
import TodoPanel from "./TodoPanel.vue";
import WeatherDisplay from "./WeatherDisplay.vue";

const props = defineProps<{
  mode: ProductWindowMode;
  locked: boolean;
  displayName: string;
  avatarUrl: string;
  avatarAvailable: boolean;
  homepageLabel: string;
  homepageUrl: string;
  nativeBridgeAvailable: boolean;
  weather: WeatherViewState;
}>();
defineEmits<{
  openHomepage: [];
  collapse: [];
  configureWeather: [];
  openReview: [];
  error: [message: string];
}>();

const dragRegionEnabled = computed(() => props.mode === "floating" && !props.locked);
const initials = computed(() => profileInitials(props.displayName));

const dateLabel = new Intl.DateTimeFormat("en-GB", {
  weekday: "long",
  day: "2-digit",
  month: "long",
})
  .format(new Date())
  .replace(/^(\w+) /, "$1, ")
  .toUpperCase();
</script>

<template>
  <div class="product-content" :data-mode="mode">
    <header
      class="observation-header"
      :class="{ 'drag-enabled': dragRegionEnabled }"
      :data-tauri-drag-region="dragRegionEnabled ? '' : undefined"
    >
      <!-- Tauri drag-region matching is target-based rather than inherited,
           so the visible text must carry the marker as well as the background. -->
      <p class="date-line" :data-tauri-drag-region="dragRegionEnabled ? '' : undefined">
        {{ dateLabel }}
      </p>
      <WeatherDisplay :weather="weather" @configure="$emit('configureWeather')" />
    </header>

    <TodoPanel
      :native-bridge-available="nativeBridgeAvailable"
      @open-review="$emit('openReview')"
      @error="$emit('error', $event)"
    />

    <footer class="signature">
      <button
        v-if="mode === 'floating'"
        type="button"
        class="profile-identity collapse-affordance"
        aria-label="Collapse to Avatar Orb"
        title="Collapse to Avatar Orb"
        @click="$emit('collapse')"
      >
        <span class="profile-avatar">
          <img v-if="avatarAvailable" :src="avatarUrl" alt="" />
          <span v-else>{{ initials }}</span>
        </span>
        <span>{{ displayName || "Your Name" }}</span>
      </button>
      <span v-else class="profile-identity">
        <span class="profile-avatar">
          <img v-if="avatarAvailable" :src="avatarUrl" alt="" />
          <span v-else>{{ initials }}</span>
        </span>
        <span>{{ displayName || "Your Name" }}</span>
      </span>
      <button v-if="homepageUrl" type="button" @click="$emit('openHomepage')">
        {{ homepageLabel || "Homepage" }} ↗
      </button>
      <span v-else class="homepage-placeholder">{{ homepageLabel || "Homepage" }}</span>
    </footer>
  </div>
</template>
