<script setup lang="ts">
import { computed } from "vue";
import { APPEARANCE_MODES } from "../../appearance";
import { useI18n } from "../../i18n";
import type {
  AppearanceSettings,
  BackgroundType,
  ImageFit,
  ImagePosition,
  ProductWindowMode,
  TextContrast,
} from "../../types";

/**
 * The Appearance category: the per-window-mode profile selector, the background
 * material and its controls, the background image picker, the text contrast
 * modes and the reset action.
 *
 * It renders inside the `v-show` tabpanel `SettingsPanel` owns, so it mounts
 * and unmounts with the Settings panel rather than with the active category.
 * The split with the parent follows the panel's ownership rule (the same one
 * the GeneralPanel, ModulesPanel and WeatherPanel extractions apply): every
 * persisted value — the three appearance profiles and the legacy appearance
 * field — stays a SettingsPanel draft (the dirty snapshot and Save read it
 * there), and so does every side effect around them. The profile being edited
 * and the current profile's settings are the parent's state, the background
 * image picker's native commands and the provisional-asset lifecycle stay with
 * the parent's asset lifecycle, and reset clears only the profile the parent
 * is editing. This component owns only the presentation and reports every
 * change back through events instead of touching the draft itself.
 *
 * The template is a multi-root fragment on purpose: the moved rows stay the
 * tabpanel's direct children, so the rendered DOM — and with it every
 * descendant selector in styles.css — is identical to the inline layout this
 * extraction replaced.
 *
 * The background row's inline error is rendered by the parent and handed in
 * through the `backgroundError` slot, exactly like GeneralPanel's avatarError:
 * the error state is the parent's — one ref shared with the avatar picker,
 * which clears it on its own picks — and the Rust source pin in
 * `appearance.rs` requires the rendering expression to stay in
 * SettingsPanel.vue. The slot keeps the element exactly where it rendered
 * inline, under the button that produces the failure.
 */
const props = defineProps<{
  /**
   * The profile the controls edit — the parent's editor state, not the
   * product's window mode. Selecting here never changes the mode the app
   * runs in and is never persisted.
   */
  mode: ProductWindowMode;
  /** The parent's draft of the profile being edited; rendered read-only here. */
  settings: AppearanceSettings;
  /** Whether the OS reports a usable desktop wallpaper (the Wallpaper row's status). */
  wallpaperAvailable: boolean;
  /**
   * Whether the edited profile's background image cannot be shown right now —
   * the parent computes it from its picker state and the loaded profile.
   */
  editedImageUnavailable: boolean;
}>();

const emit = defineEmits<{
  selectProfile: [mode: ProductWindowMode];
  updateBackgroundType: [value: BackgroundType];
  updateGlassTintColor: [value: string];
  updateGlassTintOpacity: [value: number];
  updateBlurPx: [value: number];
  updateSolidColor: [value: string];
  updateGradientStartColor: [value: string];
  updateGradientEndColor: [value: string];
  updateGradientAngle: [value: number];
  updateImageFit: [value: ImageFit];
  updateImagePosition: [value: ImagePosition];
  updateBackgroundOpacity: [value: number];
  updateOverlayStrength: [value: number];
  updateTextContrast: [value: TextContrast];
  updateCustomTextColor: [value: string];
  chooseBackground: [];
  resetAppearance: [];
}>();

/**
 * The background row's inline error is rendered by the parent; see the
 * component comment for why the element cannot move here.
 */
defineSlots<{ backgroundError?: () => unknown }>();

const { t } = useI18n();

/**
 * v-model bridges over the parent-owned profile draft. The computeds hold no
 * state of their own — reads flow down from the `settings` prop, writes emit
 * up — so the inputs keep v-model's exact semantics (the range steps and the
 * color inputs included) while the only copy of each value stays the parent's
 * draft. The two select bridges narrow the option strings before emitting, so
 * an unexpected value cannot reach the parent's draft.
 */
const glassTintColor = computed({
  get: () => props.settings.glassTintColor,
  set: (value: string) => emit("updateGlassTintColor", value),
});
const glassTintOpacity = computed({
  get: () => props.settings.glassTintOpacity,
  set: (value: number) => emit("updateGlassTintOpacity", value),
});
const blurPx = computed({
  get: () => props.settings.blurPx,
  set: (value: number) => emit("updateBlurPx", value),
});
const solidColor = computed({
  get: () => props.settings.solidColor,
  set: (value: string) => emit("updateSolidColor", value),
});
const gradientStartColor = computed({
  get: () => props.settings.gradientStartColor,
  set: (value: string) => emit("updateGradientStartColor", value),
});
const gradientEndColor = computed({
  get: () => props.settings.gradientEndColor,
  set: (value: string) => emit("updateGradientEndColor", value),
});
const gradientAngle = computed({
  get: () => props.settings.gradientAngle,
  set: (value: number) => emit("updateGradientAngle", value),
});
const imageFit = computed({
  get: () => props.settings.imageFit,
  set: (value: string) => {
    if (value === "cover" || value === "contain" || value === "stretch") {
      emit("updateImageFit", value);
    }
  },
});
const imagePosition = computed({
  get: () => props.settings.imagePosition,
  set: (value: string) => {
    if (value === "center" || value === "top" || value === "bottom") {
      emit("updateImagePosition", value);
    }
  },
});
const backgroundOpacity = computed({
  get: () => props.settings.backgroundOpacity,
  set: (value: number) => emit("updateBackgroundOpacity", value),
});
const overlayStrength = computed({
  get: () => props.settings.overlayStrength,
  set: (value: number) => emit("updateOverlayStrength", value),
});
const customTextColor = computed({
  get: () => props.settings.customTextColor,
  set: (value: string) => emit("updateCustomTextColor", value),
});
</script>

<template>
  <!--
    Which profile the controls below edit. This is Settings UI state only: it
    never changes the window mode the product runs in, and it is not persisted,
    so it reopens on the mode actually in use. The mode names are the same ones
    the window menu uses.
  -->
  <div class="settings-row appearance-background-row">
    <span>
      <strong>{{ t("settings.appearanceFor") }}</strong>
      <small>{{ t("settings.appearanceForHint") }}</small>
    </span>
    <div class="appearance-options" role="group" :aria-label="t('settings.appearanceForAriaLabel')">
      <button
        v-for="option in APPEARANCE_MODES"
        :key="option"
        type="button"
        :class="{ active: mode === option }"
        :aria-pressed="mode === option"
        @click="$emit('selectProfile', option)"
      >
        {{ t(`menu.mode.${option}`) }}
      </button>
    </div>
  </div>
  <div class="settings-row appearance-background-row">
    <span><strong>{{ t("settings.background") }}</strong><small>{{ t("settings.backgroundHint") }}</small></span>
    <div class="appearance-options" role="group" :aria-label="t('settings.backgroundAriaLabel')">
      <button
        v-for="option in ['glass', 'solid', 'gradient', 'image', 'wallpaper'] as const"
        :key="option"
        type="button"
        :class="{ active: settings.backgroundType === option }"
        @click="$emit('updateBackgroundType', option)"
      >
        {{ t(`settings.backgroundType.${option}`) }}
      </button>
    </div>
  </div>

  <template v-if="settings.backgroundType === 'glass'">
    <label class="settings-row">
      <span><strong>{{ t("settings.tint") }}</strong><small>{{ t("settings.tintHint") }}</small></span>
      <div class="color-control"><input v-model="glassTintColor" type="color" /><code>{{ settings.glassTintColor }}</code></div>
    </label>
    <label class="settings-row">
      <span><strong>{{ t("settings.tintOpacity") }}</strong><small>{{ t("settings.tintOpacityHint") }}</small></span>
      <div class="range-control"><input v-model.number="glassTintOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(settings.glassTintOpacity * 100) }}%</output></div>
    </label>
    <label class="settings-row">
      <span><strong>{{ t("settings.blur") }}</strong><small>{{ t("settings.blurHint") }}</small></span>
      <div class="range-control"><input v-model.number="blurPx" type="range" min="0" max="24" step="1" /><output>{{ settings.blurPx }}px</output></div>
    </label>
  </template>

  <label v-if="settings.backgroundType === 'solid'" class="settings-row">
    <span><strong>{{ t("settings.color") }}</strong><small>{{ t("settings.colorHint") }}</small></span>
    <div class="color-control"><input v-model="solidColor" type="color" /><code>{{ settings.solidColor }}</code></div>
  </label>

  <template v-if="settings.backgroundType === 'gradient'">
    <label class="settings-row">
      <span><strong>{{ t("settings.gradientStart") }}</strong><small>{{ t("settings.gradientStartHint") }}</small></span>
      <div class="color-control"><input v-model="gradientStartColor" type="color" /><code>{{ settings.gradientStartColor }}</code></div>
    </label>
    <label class="settings-row">
      <span><strong>{{ t("settings.gradientEnd") }}</strong><small>{{ t("settings.gradientEndHint") }}</small></span>
      <div class="color-control"><input v-model="gradientEndColor" type="color" /><code>{{ settings.gradientEndColor }}</code></div>
    </label>
    <label class="settings-row">
      <span><strong>{{ t("settings.angle") }}</strong><small>{{ t("settings.angleHint") }}</small></span>
      <div class="range-control"><input v-model.number="gradientAngle" type="range" min="0" max="360" step="1" /><output>{{ settings.gradientAngle }}°</output></div>
    </label>
  </template>

  <template v-if="settings.backgroundType === 'image'">
    <div class="settings-row">
      <span><strong>{{ t("settings.customImage") }}</strong><small>{{ t("settings.customImageHint") }}</small></span>
      <div class="asset-actions">
        <button type="button" @click="$emit('chooseBackground')">{{ t("settings.chooseImage") }}</button>
        <small v-if="editedImageUnavailable" class="asset-unavailable">{{ t("settings.imageUnavailable") }}</small>
        <!--
          The failure is shown here, under the button that caused it, instead of
          at the bottom of the panel: the picker is a native dialog whose
          rejections are otherwise invisible, and "the click did nothing" is the
          worst possible answer. The parent renders the element itself, through
          the backgroundError slot above.
        -->
        <slot name="backgroundError"></slot>
      </div>
    </div>
    <label class="settings-row">
      <span><strong>{{ t("settings.fit") }}</strong><small>{{ t("settings.fitHint") }}</small></span>
      <select v-model="imageFit"><option value="cover">{{ t("settings.fit.cover") }}</option><option value="contain">{{ t("settings.fit.contain") }}</option><option value="stretch">{{ t("settings.fit.stretch") }}</option></select>
    </label>
    <label class="settings-row">
      <span><strong>{{ t("settings.position") }}</strong><small>{{ t("settings.positionHint") }}</small></span>
      <select v-model="imagePosition"><option value="center">{{ t("settings.position.center") }}</option><option value="top">{{ t("settings.position.top") }}</option><option value="bottom">{{ t("settings.position.bottom") }}</option></select>
    </label>
  </template>

  <div v-if="settings.backgroundType === 'wallpaper'" class="settings-row">
    <span><strong>{{ t("settings.desktopWallpaper") }}</strong><small>{{ t("settings.desktopWallpaperHint") }}</small></span>
    <span class="asset-status">{{ wallpaperAvailable ? t("settings.wallpaperAvailable") : t("settings.wallpaperUnavailable") }}</span>
  </div>

  <label class="settings-row">
    <span><strong>{{ t("settings.backgroundOpacity") }}</strong><small>{{ t("settings.backgroundOpacityHint") }}</small></span>
    <div class="range-control"><input v-model.number="backgroundOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(settings.backgroundOpacity * 100) }}%</output></div>
  </label>
  <label class="settings-row">
    <span><strong>{{ t("settings.overlay") }}</strong><small>{{ t("settings.overlayHint") }}</small></span>
    <div class="range-control"><input v-model.number="overlayStrength" type="range" min="0" max="0.72" step="0.01" /><output>{{ Math.round(settings.overlayStrength * 100) }}%</output></div>
  </label>
  <div class="settings-row appearance-background-row">
    <span><strong>{{ t("settings.textContrast") }}</strong><small>{{ t("settings.textContrastHint") }}</small></span>
    <div class="appearance-options" role="group" :aria-label="t('settings.textContrastAriaLabel')">
      <button
        v-for="option in ['auto', 'light', 'dark', 'custom'] as const"
        :key="option"
        type="button"
        :class="{ active: settings.textContrast === option }"
        @click="$emit('updateTextContrast', option)"
      >
        {{ t(`settings.textContrast.${option}`) }}
      </button>
    </div>
  </div>
  <label v-if="settings.textContrast === 'custom'" class="settings-row">
    <span><strong>{{ t("settings.customTextColor") }}</strong><small>{{ t("settings.customTextColorHint") }}</small></span>
    <div class="color-control"><input v-model="customTextColor" type="color" /><code>{{ settings.customTextColor }}</code></div>
  </label>
  <button type="button" class="reset-appearance" @click="$emit('resetAppearance')">{{ t("settings.resetAppearance") }}</button>
</template>
