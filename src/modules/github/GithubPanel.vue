<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "../../i18n";
import type { GithubViewState } from "../../types";

/**
 * The GitHub dashboard card (v1.3.0-G1): the two metrics — followers and the
 * total stars across the account's owned public non-fork repositories — plus
 * the account identity, the last-update line, and the refresh affordance.
 *
 * Presentation only: every state below renders from the one `GithubViewState`
 * the app-level controller owns, and every action is emitted upward — the
 * same boundary `WeatherDisplay` keeps. There is deliberately no content
 * fetching, polling, or collapse coupling here: collapse is the
 * shared `ModuleSection` container's job and only ever hides this body.
 */
const props = defineProps<{
  github: GithubViewState;
  githubMessage: string;
  /**
   * The account the backend explicitly reported as nonexistent (current
   * identity only). The card renders the localized not-found explanation in
   * place of the generic empty placeholder (M2).
   */
  notFound: string | null;
}>();
defineEmits<{ configure: []; refresh: []; openProfile: [] }>();
const { t, intlLocale } = useI18n();

/**
 * Metric numbers are locale-formatted (1,234 / 1.234), matching the date
 * line's `Intl` usage. The backend's `u64` arrives as a JSON number, exact
 * for every count GitHub can serve.
 */
function count(value: number) {
  return new Intl.NumberFormat(intlLocale.value).format(value);
}

function ageLabel(seconds: number | null) {
  if (seconds === null) return t("github.ageUnavailable");
  if (seconds < 60 * 60) {
    return t("github.ageMinutes", { minutes: Math.max(1, Math.round(seconds / 60)) });
  }
  if (seconds < 24 * 60 * 60) {
    return t("github.ageHours", { hours: Math.round(seconds / 3600) });
  }
  return t("github.ageYesterday");
}

/**
 * The account name shown as the identity line and the profile link target:
 * the canonical login from the snapshot when one exists, otherwise the
 * configured spelling. Empty only when unconfigured, where no identity line
 * renders at all.
 */
const accountName = computed(
  () => props.github.snapshot?.username ?? props.github.username,
);

/**
 * The snapshot-bearing states: fresh data renders plainly, stale data keeps
 * rendering with the explicit age line (the offline contract — the last
 * complete fetch stays visible and honestly dated).
 */
const hasSnapshot = computed(
  () => props.github.snapshot !== null && props.github.cacheStatus !== "missing",
);
</script>

<template>
  <div class="github-display" aria-live="polite">
    <!--
      The manual refresh affordance: a real button with a localized accessible
      name, disabled for the duration of the in-flight refresh so the spinning
      icon and the disabled semantics always agree. It exists only when a
      username is configured — an unconfigured module has nothing to refresh
      and, by contract, must not ask the backend for anything.
    -->
    <button
      v-if="github.configured"
      type="button"
      class="github-refresh"
      :class="{ refreshing: github.refreshing }"
      :disabled="github.refreshing"
      :aria-label="t('github.refreshAriaLabel')"
      :title="t('github.refreshAriaLabel')"
      @click="$emit('refresh')"
    >
      <svg class="github-refresh-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="23 4 23 10 17 10" />
          <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
        </g>
      </svg>
    </button>

    <!-- Unconfigured: the setup state. No numbers are invented here. -->
    <template v-if="!github.configured">
      <p class="github-empty">{{ t("github.setupHint") }}</p>
      <button type="button" class="github-configure" @click="$emit('configure')">
        {{ t("github.setUsername") }}
      </button>
    </template>

    <!-- Configured with a complete snapshot: the two metrics. -->
    <template v-else-if="hasSnapshot">
      <p class="github-primary">
        <span class="github-metric">
          <span class="github-metric-value">{{ count(github.snapshot!.followers) }}</span>
          <span class="github-metric-label">{{ t("github.followers") }}</span>
        </span>
        <span class="github-metric">
          <span class="github-metric-value">{{ count(github.snapshot!.stars) }}</span>
          <span class="github-metric-label">{{ t("github.stars") }}</span>
        </span>
      </p>
      <p class="github-secondary">
        <!--
          The safe profile link: a labeled control that hands the username to
          the shell (`open_github_profile`), never a raw anchor with an
          unvalidated href.
        -->
        <button
          type="button"
          class="github-account"
          :aria-label="t('github.profileAriaLabel', { name: accountName })"
          :title="t('github.profileAriaLabel', { name: accountName })"
          @click="$emit('openProfile')"
        >@{{ accountName }} <span aria-hidden="true">↗</span></button>
      </p>
      <p v-if="github.cacheStatus === 'stale'" class="github-age">
        {{ ageLabel(github.cacheAgeSeconds) }}
      </p>
    </template>

    <!--
      Configured without a usable snapshot: the honest empty/unavailable
      states. An explicit not-found report from the backend renders its own
      localized explanation — never the generic empty placeholder (M2). A
      rate-limited window names its remaining minutes; anything else stays
      generic rather than leaking backend vocabulary.
    -->
    <template v-else>
      <p v-if="notFound" class="github-empty">
        {{ t("github.error.notFound", { name: notFound }) }}
      </p>
      <p v-else-if="github.rateLimited && github.retryAfterSeconds !== null" class="github-empty">
        {{
          t("github.rateLimited", {
            minutes: Math.max(1, Math.ceil(github.retryAfterSeconds / 60)),
          })
        }}
      </p>
      <p v-else class="github-empty">{{ t("github.empty") }}</p>
      <p v-if="github.snapshot" class="github-age">{{ ageLabel(github.cacheAgeSeconds) }}</p>
    </template>

    <!--
      The status of the last manual refresh. Success is quiet, failure keeps
      the previous snapshot on the card above this line.
    -->
    <p v-if="githubMessage" class="github-status" role="status">{{ githubMessage }}</p>
  </div>
</template>
