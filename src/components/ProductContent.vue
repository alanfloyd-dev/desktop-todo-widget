<script setup lang="ts">
import { computed } from "vue";
import { profileInitials } from "../appearance";
import { useI18n } from "../i18n";
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

const { t, intlLocale } = useI18n();

const dragRegionEnabled = computed(() => props.mode === "floating" && !props.locked);
const initials = computed(() => profileInitials(props.displayName));

/**
 * Computed rather than a module-level constant: the date line has to re-render
 * in the new language when the UI language changes.
 */
const dateLabel = computed(() =>
  new Intl.DateTimeFormat(intlLocale.value, {
    weekday: "long",
    day: "2-digit",
    month: "long",
  })
    .format(new Date())
    .toUpperCase(),
);
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
        :aria-label="t('footer.collapseToOrb')"
        :title="t('footer.collapseToOrb')"
        @click="$emit('collapse')"
      >
        <span class="profile-avatar">
          <img v-if="avatarAvailable" :src="avatarUrl" alt="" />
          <span v-else>{{ initials }}</span>
        </span>
        <span>{{ displayName || t("footer.defaultDisplayName") }}</span>
      </button>
      <span v-else class="profile-identity">
        <span class="profile-avatar">
          <img v-if="avatarAvailable" :src="avatarUrl" alt="" />
          <span v-else>{{ initials }}</span>
        </span>
        <span>{{ displayName || t("footer.defaultDisplayName") }}</span>
      </span>
      <button v-if="homepageUrl" type="button" @click="$emit('openHomepage')">
        {{ homepageLabel || t("footer.defaultHomepageLabel") }} ↗
      </button>
      <span v-else class="homepage-placeholder">
        {{ homepageLabel || t("footer.defaultHomepageLabel") }}
      </span>
    </footer>
  </div>
</template>
