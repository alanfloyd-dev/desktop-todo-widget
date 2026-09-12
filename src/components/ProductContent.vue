<script setup lang="ts">
import { computed } from "vue";
import { profileInitials } from "../appearance";
import { useI18n } from "../i18n";
import type { ProductWindowMode, QuickLink, WeatherViewState } from "../types";
import TodoPanel from "./TodoPanel.vue";
import WeatherDisplay from "./WeatherDisplay.vue";

const props = defineProps<{
  mode: ProductWindowMode;
  locked: boolean;
  displayName: string;
  avatarUrl: string;
  avatarAvailable: boolean;
  quickLinks: QuickLink[];
  nativeBridgeAvailable: boolean;
  weather: WeatherViewState;
}>();
defineEmits<{
  openQuickLink: [id: string];
  collapse: [];
  configureWeather: [];
  openReview: [];
  error: [message: string];
}>();

const { t, intlLocale } = useI18n();

const dragRegionEnabled = computed(
  () => (props.mode === "floating" || props.mode === "desktop") && !props.locked,
);
const initials = computed(() => profileInitials(props.displayName));

/**
 * Whether there is any user profile content to render.
 *
 * Either half is enough: an avatar with no name still shows the picture, and a
 * name with no avatar shows derived initials. Neither present means the identity
 * block is absent — no placeholder label, no invented initials. `displayName` is
 * compared trimmed so a whitespace-only value counts as absent.
 */
const hasIdentity = computed(
  () => props.avatarAvailable || props.displayName.trim().length > 0,
);

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

    <!--
      Quick Links is a product section, not a footer affordance: it sits in the
      same section rhythm as the todo panel above it. With no links configured the
      whole section is absent rather than rendering an empty block, which is why
      the heading lives inside the `v-if`.
    -->
    <section
      v-if="quickLinks.length"
      class="quick-links"
      :aria-label="t('quickLinks.heading')"
    >
      <p class="section-heading">{{ t("quickLinks.heading") }}</p>
      <ul>
        <li v-for="link in quickLinks" :key="link.id">
          <button type="button" :title="link.url" @click="$emit('openQuickLink', link.id)">
            <span class="quick-link-name">{{ link.name }}</span>
            <span class="quick-link-arrow" aria-hidden="true">↗</span>
          </button>
        </li>
      </ul>
    </section>

    <!--
      The profile identity is user content, not a product fixture. With neither a
      display name nor an avatar there is nothing to show, so no identity block and
      no fallback content is rendered — a cleared display name stays semantically
      empty.

      Collapsing is a window action, not profile content, so it is decoupled from
      the identity: in Floating the control is always present. With identity the
      existing affordance (whole block clickable) is preserved; without it a
      standalone control takes its place rather than a fake placeholder. Both emit
      the same `collapse` event, so there is one collapse path.
    -->
    <footer class="signature">
      <template v-if="mode === 'floating'">
        <button
          v-if="hasIdentity"
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
          <span>{{ displayName }}</span>
        </button>
        <button
          v-else
          type="button"
          class="collapse-affordance collapse-affordance-standalone"
          :aria-label="t('footer.collapseToOrb')"
          :title="t('footer.collapseToOrb')"
          @click="$emit('collapse')"
        >{{ t("footer.collapseToOrb") }}</button>
      </template>
      <span v-else-if="hasIdentity" class="profile-identity">
        <span class="profile-avatar">
          <img v-if="avatarAvailable" :src="avatarUrl" alt="" />
          <span v-else>{{ initials }}</span>
        </span>
        <span>{{ displayName }}</span>
      </span>
    </footer>
  </div>
</template>
