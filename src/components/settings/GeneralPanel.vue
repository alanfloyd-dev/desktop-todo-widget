<script setup lang="ts">
import { computed } from "vue";
import { profileInitials } from "../../appearance";
import { languageOptionLabel, useI18n, type Language } from "../../i18n";

/**
 * The General category: the language picker, the display name, the avatar
 * and the day rollover time.
 *
 * It renders inside the `v-show` tabpanel `SettingsPanel` owns, so it mounts
 * and unmounts with the Settings panel rather than with the active category.
 * The split with the parent follows the panel's ownership rule (the same one
 * the WeatherPanel and ModulesPanel extractions apply): everything persisted —
 * the language, the display name, the avatar asset and the rollover time —
 * stays a SettingsPanel draft (the dirty snapshot and Save read it there),
 * and so does every side effect around them. The language preview goes
 * through the app's one i18n instance and is rolled back by the parent when
 * the panel closes; the avatar picker's native commands and the
 * provisional-asset cleanup stay with the parent's asset lifecycle. This
 * component owns only the presentation and reports every change back through
 * events instead of touching the draft itself.
 *
 * The template is a multi-root fragment on purpose: the moved rows stay the
 * tabpanel's direct children, so the rendered DOM — and with it every
 * descendant selector in styles.css — is identical to the inline layout this
 * extraction replaced.
 */
const props = defineProps<{
  /** The parent's language draft. The preview itself is the parent's side effect. */
  language: Language;
  /** The parent's display-name draft; also feeds the avatar's initials. */
  displayName: string;
  /** The parent's day-rollover draft. */
  dayRollover: string;
  /** The parent's avatar asset draft; the Remove button's disabled state reads it. */
  avatarAssetId: string | null;
  /** The avatar preview's source: the normalized bytes picked this session, or the stored copy. */
  avatarPreviewUrl: string;
  /** Whether the preview has an image to show; otherwise the initials render. */
  avatarPreviewAvailable: boolean;
}>();

const emit = defineEmits<{
  selectLanguage: [value: Language];
  updateDisplayName: [value: string];
  updateDayRollover: [value: string];
  chooseAvatar: [];
  removeAvatar: [];
}>();

/**
 * The avatar row's inline error is rendered by the parent and handed in
 * through this slot: the error state is the parent's — one ref shared with
 * the background picker, which clears it on its own picks — and the Rust
 * source pin in `appearance.rs` keeps the rendering expression in
 * SettingsPanel.vue. The slot keeps the element exactly where it rendered
 * inline, under the buttons that produce the failure.
 */
defineSlots<{ avatarError?: () => unknown }>();

const { t, locale } = useI18n();

/**
 * The "Currently using X" hint, shown while the preference is `system`: the
 * language actually rendering, read from the app's single i18n instance —
 * which already reflects the parent's unsaved preview while one is up.
 */
const effectiveLanguage = computed(() => languageOptionLabel(locale.value, t));

/**
 * v-model bridges over the parent-owned drafts. The computeds hold no state
 * of their own — reads flow down from the props, writes emit up — so the
 * inputs keep v-model's exact semantics (IME composition included) while the
 * only copy of each value stays the parent's draft.
 */
const displayNameDraft = computed({
  get: () => props.displayName,
  set: (value: string) => emit("updateDisplayName", value),
});
const dayRolloverDraft = computed({
  get: () => props.dayRollover,
  set: (value: string) => emit("updateDayRollover", value),
});
</script>

<template>
  <div class="settings-row">
    <span>
      <strong>{{ t("settings.language.label") }}</strong>
      <small>
        {{ t("settings.language.hint") }}<template v-if="language === 'system'"> · {{ t("settings.language.effective", { language: effectiveLanguage }) }}</template>
        · {{ t("settings.language.restartHint") }}
      </small>
    </span>
    <div class="appearance-options" role="group" :aria-label="t('settings.language.label')">
      <button
        v-for="option in (['system', 'en', 'zh-Hans'] as const)"
        :key="option"
        type="button"
        :class="{ active: language === option }"
        :aria-pressed="language === option"
        @click="$emit('selectLanguage', option)"
      >
        {{ languageOptionLabel(option, t) }}
      </button>
    </div>
  </div>
  <label class="settings-row">
    <span><strong>{{ t("settings.displayName") }}</strong><small>{{ t("settings.displayNameHint") }}</small></span>
    <input v-model="displayNameDraft" type="text" :placeholder="t('settings.displayNamePlaceholder')" />
  </label>
  <div class="settings-row avatar-settings-row">
    <span><strong>{{ t("settings.avatar") }}</strong><small>{{ t("settings.avatarHint") }}</small></span>
    <div class="avatar-settings-control">
      <span class="settings-avatar-preview" :aria-label="t('settings.avatarAriaLabel')">
        <img v-if="avatarPreviewAvailable" :src="avatarPreviewUrl" alt="" />
        <span v-else>{{ profileInitials(displayName) }}</span>
      </span>
      <button type="button" @click="$emit('chooseAvatar')">{{ t("settings.chooseImage") }}</button>
      <button type="button" :disabled="!avatarAssetId" @click="$emit('removeAvatar')">{{ t("settings.removeAvatar") }}</button>
      <!--
        The failure is shown here, under the button that caused it, instead of
        at the bottom of the panel: the picker is a native dialog whose
        rejections are otherwise invisible, and "the click did nothing" is the
        worst possible answer. The parent renders the element itself, through
        the avatarError slot above.
      -->
      <slot name="avatarError"></slot>
    </div>
  </div>
  <label class="settings-row">
    <span><strong>{{ t("settings.dayRollover") }}</strong><small>{{ t("settings.dayRolloverHint") }}</small></span>
    <input v-model="dayRolloverDraft" type="time" />
  </label>
</template>
