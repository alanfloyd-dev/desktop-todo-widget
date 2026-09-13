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
  openSettings: [];
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

      <!--
        The visible Settings entry point.

        Settings used to be reachable only from the right-click menu (and the
        tray), which nothing on the surface advertises. This is that same action
        as a footer control, not a second settings surface: it emits
        `openSettings`, which `App.vue` routes through the existing
        `product_action("settings")` command.

        It is unconditional within this component, which is what makes the
        visibility rule fall out structurally rather than from a mode list: the
        component renders exactly in the three expanded presentations (Floating
        expanded, Sidebar, Desktop) and is replaced by `FloatingOrb` in the
        collapsed Orb, so the gear exists in every expanded mode and nowhere
        else. It is also independent of the profile identity, so a user who
        cleared their display name and avatar still has a way in.

        Keyboard: it is a real `<button>`, so Tab reaches it and Enter/Space
        activate it without extra handlers.
      -->
      <button
        type="button"
        class="footer-settings-button"
        :aria-label="t('footer.settings')"
        :title="t('footer.settings')"
        @click="$emit('openSettings')"
      >
        <svg
          class="footer-settings-icon"
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <g
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap="round"
          >
            <circle cx="12" cy="12" r="5.6" />
            <circle cx="12" cy="12" r="2.3" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(45 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(90 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(135 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(180 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(225 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(270 12 12)" />
            <line x1="12" y1="3.4" x2="12" y2="6.5" transform="rotate(315 12 12)" />
          </g>
        </svg>
      </button>
    </footer>
  </div>
</template>
