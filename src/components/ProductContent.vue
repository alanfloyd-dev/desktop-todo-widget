<script setup lang="ts">
import { computed } from "vue";
import { profileInitials } from "../appearance";
import { useI18n } from "../i18n";
import { hasColumnSpan, type DashboardModuleId } from "../modules/layout";
import type {
  GithubViewState,
  ModuleLayout,
  ModuleVisibility,
  ProductWindowMode,
  QuickLink,
  WeatherViewState,
} from "../types";
import ModuleSection from "../modules/shared/ModuleSection.vue";
import TodoPanel from "../modules/todo/TodoPanel.vue";
import WeatherDisplay from "../modules/weather/WeatherDisplay.vue";
import GithubPanel from "../modules/github/GithubPanel.vue";

const props = defineProps<{
  mode: ProductWindowMode;
  locked: boolean;
  displayName: string;
  avatarUrl: string;
  avatarAvailable: boolean;
  quickLinks: QuickLink[];
  /** Effective per-module visibility, resolved by the app from the settings document. */
  moduleVisibility: ModuleVisibility;
  /**
   * Effective dashboard layout, resolved by the app from the persisted
   * document; the slot order below is its entry order.
   */
  moduleLayout: ModuleLayout;
  /**
   * Whether a collapse change can actually persist (D2), derived by the app
   * from the RAW stored layout document. False disables the module toggles
   * with a localized explanation instead of letting a click do nothing.
   */
  collapseAvailable: boolean;
  nativeBridgeAvailable: boolean;
  weather: WeatherViewState;
  weatherMessage: string;
  /** The one GitHub state owned by the app-level controller (v1.3.0-G1). */
  github: GithubViewState;
  githubMessage: string;
  /** Account the backend reported as nonexistent, for the current identity (M2). */
  githubNotFound: string | null;
}>();
defineEmits<{
  openQuickLink: [id: string];
  collapse: [];
  configureWeather: [];
  refreshWeather: [];
  configureGithub: [];
  refreshGithub: [];
  openGithubProfile: [];
  openReview: [];
  openSettings: [];
  toggleModule: [id: DashboardModuleId];
  error: [message: string];
}>();

const { t, intlLocale } = useI18n();

/**
 * The two-column activation policy: the grid gains a second column only when
 * the effective layout actually requests one. The all-ones default layout —
 * and any order-only customization — renders single-column at every width,
 * which is what keeps today's full-width presentation the default.
 */
const spannedLayout = computed(() => hasColumnSpan(props.moduleLayout));

/**
 * The stored collapse state of one dashboard module (D1), read from the
 * effective layout the app resolved. Absent means expanded; the flag is
 * presentation data only and never influences the slot order or the span
 * classes below.
 */
function moduleCollapsed(id: DashboardModuleId): boolean {
  return props.moduleLayout.entries.find((entry) => entry.id === id)?.collapsed === true;
}

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
      :class="{ 'drag-enabled': dragRegionEnabled, 'header-closed': !moduleVisibility.weather }"
      :data-tauri-drag-region="dragRegionEnabled ? '' : undefined"
    >
      <!-- Tauri drag-region matching is target-based rather than inherited,
           so the visible text must carry the marker as well as the background. -->
      <p class="date-line" :data-tauri-drag-region="dragRegionEnabled ? '' : undefined">
        {{ dateLabel }}
      </p>
    </header>

    <!--
      The structural dashboard container (C3b-2). The slots render in the
      order of the effective layout — resolved once in App from the persisted
      document through the C1 policy — keyed by module id so a reorder MOVES
      mounted components instead of remounting them (TodoPanel keeps its
      ephemeral draft and drag state; no listener is duplicated and no
      backend write is triggered). Visibility stays independent of layout:
      each slot additionally requires its module to be enabled, and hidden
      modules render nothing. The grid is single-column unless the effective
      layout actually requests a second column (`hasColumnSpan`), so the
      all-ones default keeps today's full-width presentation at any width.
      The header and footer are shell and stay outside.
    -->
    <div class="dashboard-grid" :class="{ 'dashboard-grid--spanned': spannedLayout }">
      <template v-for="entry in moduleLayout.entries" :key="entry.id">
        <!--
          The weather card, extracted from the header in C3b-1. The slot owns
          the card's module spacing and the header separator (the border the
          observation header used to carry below the weather); hiding the
          card never stops the app-owned weather controller — the Settings
          weather section reads the same state.
        -->
        <div
          v-if="entry.id === 'weather' && moduleVisibility.weather"
          class="dashboard-slot weather-slot"
          :class="{ 'dashboard-slot--span-2': spannedLayout && entry.colSpan === 2 }"
        >
          <WeatherDisplay
            :weather="weather"
            :weather-message="weatherMessage"
            @configure="$emit('configureWeather')"
            @refresh="$emit('refreshWeather')"
          />
        </div>

        <div
          v-if="entry.id === 'todo' && moduleVisibility.todo"
          class="dashboard-slot"
          :class="{ 'dashboard-slot--span-2': spannedLayout && entry.colSpan === 2 }"
        >
          <!--
            A disabled todo module unmounts its panel: the component owns only
            ephemeral UI state (it removes its own drag listeners on unmount),
            while every task lives in SQLite behind commands that stay
            invokable. A later re-enable mounts the panel fresh, which
            re-queries the current task day. The wrapper keeps the grid item
            and its span class in one place. Collapsing (D1) is different by
            design: the panel itself stays mounted behind its container's
            `v-show` body, so the draft and drag state survive.
          -->
          <TodoPanel
            :native-bridge-available="nativeBridgeAvailable"
            :show-review-entry="moduleVisibility.review"
            :collapsed="moduleCollapsed('todo')"
            :collapse-available="collapseAvailable"
            @open-review="$emit('openReview')"
            @toggle="$emit('toggleModule', 'todo')"
            @error="$emit('error', $event)"
          />
        </div>

        <!--
          Quick Links is a product section, not a footer affordance: it sits
          in the same section rhythm as the todo panel. The whole slot is
          absent when the module is disabled, or when no links are configured
          — which is why the heading lives inside the `v-if` conditions. Since
          D1 the section renders through the shared collapsible container:
          the container's root receives the `quick-links` class (and the
          landmark label) by fallthrough, so the border-top rhythm and the
          `.quick-links` selectors keep applying unchanged, the heading moves
          into the container's header, and the list renders behind the
          visibility-only body.
        -->
        <div
          v-if="entry.id === 'shortcuts' && moduleVisibility.shortcuts && quickLinks.length"
          class="dashboard-slot"
          :class="{ 'dashboard-slot--span-2': spannedLayout && entry.colSpan === 2 }"
        >
          <ModuleSection
            class="quick-links"
            :aria-label="t('quickLinks.heading')"
            :title="t('quickLinks.heading')"
            :module-name="t('modules.shortcuts')"
            :collapsed="moduleCollapsed('shortcuts')"
            :toggle-disabled="!collapseAvailable"
            @toggle="$emit('toggleModule', 'shortcuts')"
          >
            <ul>
              <li v-for="link in quickLinks" :key="link.id">
                <button
                  type="button"
                  :title="link.url"
                  @click="$emit('openQuickLink', link.id)"
                >
                  <span class="quick-link-name">{{ link.name }}</span>
                  <span class="quick-link-arrow" aria-hidden="true">↗</span>
                </button>
              </li>
            </ul>
          </ModuleSection>
        </div>

        <!--
          The GitHub statistics module (v1.3.0-G1). Hidden by default — the
          registry ships it disabled and the backend refuses any request for
          an unconfigured username — so this slot changes nothing for existing
          surfaces until the user both enables the module and configures an
          account. Collapse rides the same shared container as Quick Links;
          the card itself only ever renders from the app-owned state.
        -->
        <div
          v-if="entry.id === 'github' && moduleVisibility.github"
          class="dashboard-slot"
          :class="{ 'dashboard-slot--span-2': spannedLayout && entry.colSpan === 2 }"
        >
          <ModuleSection
            class="github-module"
            :aria-label="t('github.heading')"
            :title="t('github.heading')"
            :module-name="t('modules.github')"
            :collapsed="moduleCollapsed('github')"
            :toggle-disabled="!collapseAvailable"
            @toggle="$emit('toggleModule', 'github')"
          >
            <GithubPanel
              :github="github"
              :github-message="githubMessage"
              :not-found="githubNotFound"
              @configure="$emit('configureGithub')"
              @refresh="$emit('refreshGithub')"
              @open-profile="$emit('openGithubProfile')"
            />
          </ModuleSection>
        </div>
      </template>
    </div>

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
