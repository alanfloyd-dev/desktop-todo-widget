<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_APPEARANCE, profileInitials, sampleImageLuminance } from "../appearance";
import { languageOptionLabel, normalizeLanguage, useI18n, type Language } from "../i18n";
import type {
  AppearanceSettings,
  AssetPayload,
  LocationCandidate,
  ProductSettings,
  RenderingBackend,
  TemperatureUnit,
  WeatherViewState,
} from "../types";
import DeveloperDiagnostics from "./DeveloperDiagnostics.vue";

const props = defineProps<{
  settings: ProductSettings;
  databasePath: string;
  weather: WeatherViewState;
  weatherMessage: string;
  focusWeather: boolean;
  imageAvailable: boolean;
  wallpaperAvailable: boolean;
  avatarUrl: string;
  avatarAvailable: boolean;
}>();
const emit = defineEmits<{
  close: [];
  refreshWeather: [];
  save: [patch: {
    dayRollover: string;
    weatherLocationLabel: string;
    weatherLatitude: number | null;
    weatherLongitude: number | null;
    weatherTimezone: string;
    weatherCountry: string;
    weatherAdmin1: string;
    temperatureUnit: TemperatureUnit;
    language: Language;
    appearance: string;
    appearanceSettings: AppearanceSettings;
    displayName: string;
    avatarAssetId: string | null;
    homepageLabel: string;
    homepageUrl: string;
    renderingBackend: RenderingBackend;
  }];
}>();

const i18n = useI18n();
const { t } = i18n;

/**
 * True once this panel has handed a language choice to the parent's Save path.
 *
 * Language previews immediately (below), so a preview that is not saved has to
 * be rolled back when the panel goes away — but only for a save that was never
 * attempted. A save that failed leaves this flag `true`, which keeps the preview
 * on screen: the parent shows the error, and silently snapping the language back
 * would hide the fact that the click did something.
 */
let languageSaved = false;

const dayRollover = ref(props.settings.dayRollover);
const weatherLocationLabel = ref(props.settings.weatherLocationLabel);
const weatherLatitude = ref(props.settings.weatherLatitude);
const weatherLongitude = ref(props.settings.weatherLongitude);
const weatherTimezone = ref(props.settings.weatherTimezone);
const weatherCountry = ref(props.settings.weatherCountry);
const weatherAdmin1 = ref(props.settings.weatherAdmin1);
const temperatureUnit = ref<TemperatureUnit>(props.settings.temperatureUnit);
const locationQuery = ref("");
const candidates = ref<LocationCandidate[]>([]);
const locationSearching = ref(false);
const locationError = ref("");
const weatherSection = ref<HTMLElement | null>(null);
const locationInput = ref<HTMLInputElement | null>(null);
const appearance = ref(props.settings.appearance);
const appearanceSettings = ref<AppearanceSettings>({ ...props.settings.appearanceSettings });
const displayName = ref(props.settings.displayName);
const avatarAssetId = ref(props.settings.avatarAssetId);
const avatarPreviewUrl = ref(props.avatarUrl);
const avatarPreviewAvailable = ref(props.avatarAvailable);
const imagePreviewAvailable = ref(props.imageAvailable);
const assetMessage = ref("");
const provisionalAssets = new Set<string>();
const homepageLabel = ref(props.settings.homepageLabel);
const homepageUrl = ref(props.settings.homepageUrl);
const developerOpen = ref(false);
/**
 * Rendering backend draft.
 *
 * The hosting backend is fixed when the window's WebView is created, so this
 * value only takes effect after a restart. That is stated inline on the row
 * rather than with a modal or a banner.
 */
const renderingBackend = ref<RenderingBackend>(props.settings.renderingBackend);
const renderingBackendChanged = computed(
  () => renderingBackend.value !== props.settings.renderingBackend,
);

/**
 * Language draft.
 *
 * Changing the selection previews immediately (the app-level i18n instance is
 * updated here, not on save) because a language picker that does not show its
 * own effect is unusable on a panel this long. The persisted value is still
 * written by Save, together with every other setting on this panel, and an
 * unsaved preview is reverted in `onBeforeUnmount`.
 */
const language = ref<Language>(normalizeLanguage(props.settings.language));
const effectiveLanguage = computed(() =>
  languageOptionLabel(i18n.locale.value, t),
);

function selectLanguage(value: Language) {
  language.value = value;
  i18n.setLanguage(value);
}

function save() {
  languageSaved = true;
  emit("save", {
    dayRollover: dayRollover.value,
    weatherLocationLabel: weatherLocationLabel.value,
    weatherLatitude: weatherLatitude.value,
    weatherLongitude: weatherLongitude.value,
    weatherTimezone: weatherTimezone.value,
    weatherCountry: weatherCountry.value,
    weatherAdmin1: weatherAdmin1.value,
    temperatureUnit: temperatureUnit.value,
    language: language.value,
    appearance: appearance.value,
    appearanceSettings: appearanceSettings.value,
    displayName: displayName.value,
    avatarAssetId: avatarAssetId.value,
    homepageLabel: homepageLabel.value,
    homepageUrl: homepageUrl.value,
    renderingBackend: renderingBackend.value,
  });
}

async function discard(assetId: string) {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    await invoke("discard_managed_asset", { assetId });
  } catch {
    // A saved asset is intentionally protected by the native command.
  }
}

async function chooseAsset(kind: "background" | "avatar") {
  assetMessage.value = "";
  if (!("__TAURI_INTERNALS__" in window)) {
    assetMessage.value = t("settings.error.localFilePicker");
    return;
  }
  try {
    const selected = await invoke<AssetPayload | null>("choose_local_asset", { kind });
    if (!selected?.assetId || !selected.dataUrl) return;
    const previous = kind === "avatar"
      ? avatarAssetId.value
      : appearanceSettings.value.imageAssetId;
    if (previous && provisionalAssets.has(previous)) {
      provisionalAssets.delete(previous);
      void discard(previous);
    }
    provisionalAssets.add(selected.assetId);
    if (kind === "avatar") {
      avatarAssetId.value = selected.assetId;
      avatarPreviewUrl.value = selected.dataUrl;
      avatarPreviewAvailable.value = selected.available;
    } else {
      appearanceSettings.value.imageAssetId = selected.assetId;
      appearanceSettings.value.backgroundType = "image";
      appearanceSettings.value.sampledLuminance = await sampleImageLuminance(selected.dataUrl);
      imagePreviewAvailable.value = selected.available;
    }
  } catch (reason) {
    assetMessage.value = String(reason);
  }
}

function removeAvatar() {
  avatarAssetId.value = null;
  avatarPreviewUrl.value = "";
  avatarPreviewAvailable.value = false;
}

function resetAppearance() {
  appearanceSettings.value = { ...DEFAULT_APPEARANCE };
  imagePreviewAvailable.value = false;
}

function weatherErrorLabel(reason: unknown) {
  const kind = typeof reason === "object" && reason && "kind" in reason
    ? String((reason as { kind: unknown }).kind)
    : String(reason);
  if (kind.includes("Timeout")) return t("settings.error.locationTimeout");
  if (kind.includes("InvalidResponse")) return t("settings.error.locationInvalidResponse");
  return t("settings.error.locationUnavailable");
}

async function searchLocation() {
  const query = locationQuery.value.trim();
  locationError.value = "";
  candidates.value = [];
  if (query.length < 2) {
    locationError.value = t("settings.error.locationQueryTooShort");
    return;
  }
  locationSearching.value = true;
  try {
    candidates.value = await invoke<LocationCandidate[]>("search_weather_locations", { query });
    if (!candidates.value.length) locationError.value = t("settings.error.locationNoMatch");
  } catch (reason) {
    locationError.value = weatherErrorLabel(reason);
  } finally {
    locationSearching.value = false;
  }
}

function selectLocation(candidate: LocationCandidate) {
  weatherLocationLabel.value = candidate.label;
  weatherLatitude.value = candidate.latitude;
  weatherLongitude.value = candidate.longitude;
  weatherTimezone.value = candidate.timezone;
  weatherCountry.value = candidate.country;
  weatherAdmin1.value = candidate.admin1;
  candidates.value = [];
  locationQuery.value = "";
  locationError.value = "";
}

function candidateDetail(candidate: LocationCandidate) {
  return [candidate.admin1, candidate.country].filter(Boolean).join(", ");
}

async function openAttribution() {
  try {
    await invoke("open_weather_attribution");
  } catch {
    locationError.value = t("settings.error.openAttribution");
  }
}

onMounted(async () => {
  if (!props.focusWeather) return;
  await nextTick();
  weatherSection.value?.scrollIntoView({ block: "start" });
  locationInput.value?.focus();
});

onBeforeUnmount(() => {
  // Revert an unsaved language preview.
  //
  // `props.settings` still holds the persisted value here: the parent closes this
  // panel before it replaces the state with the command's response, so a saved
  // change is never rolled back to the value it just replaced.
  if (!languageSaved) i18n.setLanguage(normalizeLanguage(props.settings.language));

  for (const assetId of provisionalAssets) {
    const saved =
      props.settings.avatarAssetId === assetId ||
      props.settings.appearanceSettings.imageAssetId === assetId;
    if (!saved) void discard(assetId);
  }
});
</script>

<template>
  <aside class="settings-panel" :aria-label="t('settings.panelAriaLabel')">
    <header>
      <div>
        <p>{{ t("settings.title") }}</p>
        <h2>{{ t("app.name") }}</h2>
      </div>
      <button
        type="button"
        :aria-label="t('settings.closeAriaLabel')"
        @click="$emit('close')"
      >{{ t("settings.close") }}</button>
    </header>

    <div class="settings-group">
      <div class="settings-section-heading settings-major-heading">
        <strong>{{ t("settings.languageHeading") }}</strong>
      </div>
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
            @click="selectLanguage(option)"
          >
            {{ languageOptionLabel(option, t) }}
          </button>
        </div>
      </div>

      <div class="settings-section-heading settings-major-heading"><strong>{{ t("settings.profileHeading") }}</strong></div>
      <label class="settings-row">
        <span><strong>{{ t("settings.displayName") }}</strong><small>{{ t("settings.displayNameHint") }}</small></span>
        <input v-model="displayName" type="text" :placeholder="t('settings.displayNamePlaceholder')" />
      </label>
      <div class="settings-row avatar-settings-row">
        <span><strong>{{ t("settings.avatar") }}</strong><small>{{ t("settings.avatarHint") }}</small></span>
        <div class="avatar-settings-control">
          <span class="settings-avatar-preview" :aria-label="t('settings.avatarAriaLabel')">
            <img v-if="avatarPreviewAvailable" :src="avatarPreviewUrl" alt="" />
            <span v-else>{{ profileInitials(displayName) }}</span>
          </span>
          <button type="button" @click="chooseAsset('avatar')">{{ t("settings.chooseImage") }}</button>
          <button type="button" :disabled="!avatarAssetId" @click="removeAvatar">{{ t("settings.removeAvatar") }}</button>
        </div>
      </div>
      <label class="settings-row">
        <span><strong>{{ t("settings.homepageLabel") }}</strong><small>{{ t("settings.homepageLabelHint") }}</small></span>
        <input v-model="homepageLabel" type="text" :placeholder="t('settings.homepageLabelPlaceholder')" />
      </label>
      <label class="settings-row">
        <span><strong>{{ t("settings.homepageUrl") }}</strong><small>{{ t("settings.homepageUrlHint") }}</small></span>
        <input v-model="homepageUrl" type="url" placeholder="https://example.com/" />
      </label>
      <label class="settings-row">
        <span><strong>{{ t("settings.dayRollover") }}</strong><small>{{ t("settings.dayRolloverHint") }}</small></span>
        <input v-model="dayRollover" type="time" />
      </label>
      <div class="settings-section-heading settings-major-heading"><strong>{{ t("settings.weatherHeading") }}</strong></div>
      <section ref="weatherSection" class="weather-settings" aria-labelledby="weather-settings-heading">
        <div class="settings-section-heading">
          <span>
            <strong id="weather-settings-heading">{{ t("settings.weatherLocation") }}</strong>
            <small>{{ t("settings.weatherLocationHint") }}</small>
          </span>
        </div>
        <div class="weather-search-row">
          <input
            ref="locationInput"
            v-model="locationQuery"
            type="search"
            :placeholder="t('settings.weatherSearchPlaceholder')"
            autocomplete="off"
            @keydown.enter.prevent="searchLocation"
          />
          <button type="button" :disabled="locationSearching" @click="searchLocation">
            {{ locationSearching ? t("settings.weatherSearching") : t("settings.weatherSearch") }}
          </button>
        </div>
        <p v-if="locationError" class="settings-error" role="status">{{ locationError }}</p>
        <div v-if="candidates.length" class="weather-candidates" :aria-label="t('settings.weatherResultsAriaLabel')">
          <button
            v-for="candidate in candidates"
            :key="`${candidate.latitude}:${candidate.longitude}:${candidate.timezone}`"
            type="button"
            @click="selectLocation(candidate)"
          >
            <strong>{{ candidate.label }}</strong>
            <small>{{ candidateDetail(candidate) || candidate.timezone }}</small>
          </button>
        </div>
        <div class="settings-row weather-selected">
          <span><strong>{{ t("settings.weatherSelected") }}</strong><small>{{ t("settings.weatherSelectedHint") }}</small></span>
          <span class="selected-location">
            <strong>{{ weatherLocationLabel || t("settings.weatherNotConfigured") }}</strong>
            <small v-if="weatherLocationLabel">
              {{ [weatherAdmin1, weatherCountry].filter(Boolean).join(", ") || weatherTimezone }}
            </small>
          </span>
        </div>
        <label class="settings-row">
          <span><strong>{{ t("settings.temperature") }}</strong><small>{{ t("settings.temperatureHint") }}</small></span>
          <select v-model="temperatureUnit">
            <option value="celsius">{{ t("settings.celsius") }}</option>
            <option value="fahrenheit">{{ t("settings.fahrenheit") }}</option>
          </select>
        </label>
        <div class="settings-row weather-provider-row">
          <span><strong>{{ t("settings.weatherData") }}</strong><small>{{ t("settings.weatherAttributionHint") }}</small></span>
          <button type="button" @click="openAttribution">Open-Meteo ↗</button>
        </div>
        <div class="weather-refresh-row">
          <button
            type="button"
            :disabled="!weather.configured || weather.refreshing"
            @click="$emit('refreshWeather')"
          >
            {{ weather.refreshing ? t("settings.refreshing") : t("settings.refreshNow") }}
          </button>
          <small v-if="weatherMessage" role="status">{{ weatherMessage }}</small>
        </div>
      </section>
      <section class="appearance-settings" aria-labelledby="appearance-settings-heading">
        <div class="settings-section-heading settings-major-heading">
          <strong id="appearance-settings-heading">{{ t("settings.appearanceHeading") }}</strong>
          <small>{{ t("settings.appearanceMaterial") }}</small>
        </div>
        <div class="settings-row appearance-background-row">
          <span>
            <strong>{{ t("settings.rendering") }}</strong>
            <small>
              {{
                renderingBackend === "enhanced"
                  ? t("settings.renderingEnhancedHint")
                  : t("settings.renderingStandardHint")
              }}
              <template v-if="renderingBackendChanged"> {{ t("settings.renderingRestart") }}</template>
            </small>
          </span>
          <div class="appearance-options" role="group" :aria-label="t('settings.renderingAriaLabel')">
            <button
              type="button"
              :class="{ active: renderingBackend === 'standard' }"
              @click="renderingBackend = 'standard'"
            >
              {{ t("settings.renderingStandard") }}
            </button>
            <button
              type="button"
              :class="{ active: renderingBackend === 'enhanced' }"
              @click="renderingBackend = 'enhanced'"
            >
              {{ t("settings.renderingEnhanced") }}
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
              :class="{ active: appearanceSettings.backgroundType === option }"
              @click="appearanceSettings.backgroundType = option"
            >
              {{ t(`settings.backgroundType.${option}`) }}
            </button>
          </div>
        </div>

        <template v-if="appearanceSettings.backgroundType === 'glass'">
          <label class="settings-row">
            <span><strong>{{ t("settings.tint") }}</strong><small>{{ t("settings.tintHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.glassTintColor" type="color" /><code>{{ appearanceSettings.glassTintColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.tintOpacity") }}</strong><small>{{ t("settings.tintOpacityHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.glassTintOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.glassTintOpacity * 100) }}%</output></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.blur") }}</strong><small>{{ t("settings.blurHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.blurPx" type="range" min="0" max="24" step="1" /><output>{{ appearanceSettings.blurPx }}px</output></div>
          </label>
        </template>

        <label v-if="appearanceSettings.backgroundType === 'solid'" class="settings-row">
          <span><strong>{{ t("settings.color") }}</strong><small>{{ t("settings.colorHint") }}</small></span>
          <div class="color-control"><input v-model="appearanceSettings.solidColor" type="color" /><code>{{ appearanceSettings.solidColor }}</code></div>
        </label>

        <template v-if="appearanceSettings.backgroundType === 'gradient'">
          <label class="settings-row">
            <span><strong>{{ t("settings.gradientStart") }}</strong><small>{{ t("settings.gradientStartHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientStartColor" type="color" /><code>{{ appearanceSettings.gradientStartColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.gradientEnd") }}</strong><small>{{ t("settings.gradientEndHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientEndColor" type="color" /><code>{{ appearanceSettings.gradientEndColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.angle") }}</strong><small>{{ t("settings.angleHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.gradientAngle" type="range" min="0" max="360" step="1" /><output>{{ appearanceSettings.gradientAngle }}°</output></div>
          </label>
        </template>

        <template v-if="appearanceSettings.backgroundType === 'image'">
          <div class="settings-row">
            <span><strong>{{ t("settings.customImage") }}</strong><small>{{ t("settings.customImageHint") }}</small></span>
            <div class="asset-actions">
              <button type="button" @click="chooseAsset('background')">{{ t("settings.chooseImage") }}</button>
              <small v-if="appearanceSettings.imageAssetId && !imagePreviewAvailable" class="asset-unavailable">{{ t("settings.imageUnavailable") }}</small>
            </div>
          </div>
          <label class="settings-row">
            <span><strong>{{ t("settings.fit") }}</strong><small>{{ t("settings.fitHint") }}</small></span>
            <select v-model="appearanceSettings.imageFit"><option value="cover">{{ t("settings.fit.cover") }}</option><option value="contain">{{ t("settings.fit.contain") }}</option><option value="stretch">{{ t("settings.fit.stretch") }}</option></select>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.position") }}</strong><small>{{ t("settings.positionHint") }}</small></span>
            <select v-model="appearanceSettings.imagePosition"><option value="center">{{ t("settings.position.center") }}</option><option value="top">{{ t("settings.position.top") }}</option><option value="bottom">{{ t("settings.position.bottom") }}</option></select>
          </label>
        </template>

        <div v-if="appearanceSettings.backgroundType === 'wallpaper'" class="settings-row">
          <span><strong>{{ t("settings.desktopWallpaper") }}</strong><small>{{ t("settings.desktopWallpaperHint") }}</small></span>
          <span class="asset-status">{{ wallpaperAvailable ? t("settings.wallpaperAvailable") : t("settings.wallpaperUnavailable") }}</span>
        </div>

        <label class="settings-row">
          <span><strong>{{ t("settings.backgroundOpacity") }}</strong><small>{{ t("settings.backgroundOpacityHint") }}</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.backgroundOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.backgroundOpacity * 100) }}%</output></div>
        </label>
        <label class="settings-row">
          <span><strong>{{ t("settings.overlay") }}</strong><small>{{ t("settings.overlayHint") }}</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.overlayStrength" type="range" min="0" max="0.72" step="0.01" /><output>{{ Math.round(appearanceSettings.overlayStrength * 100) }}%</output></div>
        </label>
        <label class="settings-row">
          <span><strong>{{ t("settings.textContrast") }}</strong><small>{{ t("settings.textContrastHint") }}</small></span>
          <select v-model="appearanceSettings.textContrast"><option value="auto">{{ t("settings.textContrast.auto") }}</option><option value="light">{{ t("settings.textContrast.light") }}</option><option value="dark">{{ t("settings.textContrast.dark") }}</option></select>
        </label>
        <button type="button" class="reset-appearance" @click="resetAppearance">{{ t("settings.resetAppearance") }}</button>
      </section>
      <p v-if="assetMessage" class="settings-error" role="status">{{ assetMessage }}</p>
    </div>

    <button type="button" class="save-settings" @click="save">{{ t("settings.save") }}</button>

    <section class="developer-section">
      <button type="button" class="developer-toggle" @click="developerOpen = !developerOpen">
        <span>{{ t("settings.developer") }}</span><span>{{ developerOpen ? "−" : "+" }}</span>
      </button>
      <div v-if="developerOpen" class="developer-content">
        <p class="database-path">{{ t("settings.developer.databasePath", { path: databasePath }) }}</p>
        <DeveloperDiagnostics />
      </div>
    </section>
  </aside>
</template>
