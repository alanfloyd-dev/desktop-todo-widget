<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_APPEARANCE, profileInitials, sampleImageLuminance } from "../appearance";
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
    appearance: string;
    appearanceSettings: AppearanceSettings;
    displayName: string;
    avatarAssetId: string | null;
    homepageLabel: string;
    homepageUrl: string;
    renderingBackend: RenderingBackend;
  }];
}>();

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

function save() {
  emit("save", {
    dayRollover: dayRollover.value,
    weatherLocationLabel: weatherLocationLabel.value,
    weatherLatitude: weatherLatitude.value,
    weatherLongitude: weatherLongitude.value,
    weatherTimezone: weatherTimezone.value,
    weatherCountry: weatherCountry.value,
    weatherAdmin1: weatherAdmin1.value,
    temperatureUnit: temperatureUnit.value,
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
    assetMessage.value = "Local file selection is available in the Windows app.";
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
  if (kind.includes("Timeout")) return "Location search timed out.";
  if (kind.includes("InvalidResponse")) return "The provider returned an invalid response.";
  return "Location search is unavailable.";
}

async function searchLocation() {
  const query = locationQuery.value.trim();
  locationError.value = "";
  candidates.value = [];
  if (query.length < 2) {
    locationError.value = "Enter at least two characters.";
    return;
  }
  locationSearching.value = true;
  try {
    candidates.value = await invoke<LocationCandidate[]>("search_weather_locations", { query });
    if (!candidates.value.length) locationError.value = "No matching locations.";
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
    locationError.value = "Could not open Open-Meteo.";
  }
}

onMounted(async () => {
  if (!props.focusWeather) return;
  await nextTick();
  weatherSection.value?.scrollIntoView({ block: "start" });
  locationInput.value?.focus();
});

onBeforeUnmount(() => {
  for (const assetId of provisionalAssets) {
    const saved =
      props.settings.avatarAssetId === assetId ||
      props.settings.appearanceSettings.imageAssetId === assetId;
    if (!saved) void discard(assetId);
  }
});
</script>

<template>
  <aside class="settings-panel" aria-label="Settings">
    <header>
      <div>
        <p>SETTINGS</p>
        <h2>Alan Desktop</h2>
      </div>
      <button type="button" aria-label="Close settings" @click="$emit('close')">Close</button>
    </header>

    <div class="settings-group">
      <div class="settings-section-heading settings-major-heading"><strong>PROFILE</strong></div>
      <label class="settings-row">
        <span><strong>Display name</strong><small>Footer profile</small></span>
        <input v-model="displayName" type="text" placeholder="Your Name" />
      </label>
      <div class="settings-row avatar-settings-row">
        <span><strong>Avatar</strong><small>Local managed copy</small></span>
        <div class="avatar-settings-control">
          <span class="settings-avatar-preview">
            <img v-if="avatarPreviewAvailable" :src="avatarPreviewUrl" alt="" />
            <span v-else>{{ profileInitials(displayName) }}</span>
          </span>
          <button type="button" @click="chooseAsset('avatar')">Choose image</button>
          <button type="button" :disabled="!avatarAssetId" @click="removeAvatar">Remove avatar</button>
        </div>
      </div>
      <label class="settings-row">
        <span><strong>Homepage label</strong><small>Footer shortcut text</small></span>
        <input v-model="homepageLabel" type="text" placeholder="Homepage" />
      </label>
      <label class="settings-row">
        <span><strong>Homepage URL</strong><small>Optional http(s) shortcut</small></span>
        <input v-model="homepageUrl" type="url" placeholder="https://example.com/" />
      </label>
      <label class="settings-row">
        <span><strong>Day rollover</strong><small>New day begins at</small></span>
        <input v-model="dayRollover" type="time" />
      </label>
      <div class="settings-section-heading settings-major-heading"><strong>WEATHER</strong></div>
      <section ref="weatherSection" class="weather-settings" aria-labelledby="weather-settings-heading">
        <div class="settings-section-heading">
          <span>
            <strong id="weather-settings-heading">Weather location</strong>
            <small>Search, then explicitly select a result</small>
          </span>
        </div>
        <div class="weather-search-row">
          <input
            ref="locationInput"
            v-model="locationQuery"
            type="search"
            placeholder="Search location"
            autocomplete="off"
            @keydown.enter.prevent="searchLocation"
          />
          <button type="button" :disabled="locationSearching" @click="searchLocation">
            {{ locationSearching ? "Searching…" : "Search" }}
          </button>
        </div>
        <p v-if="locationError" class="settings-error" role="status">{{ locationError }}</p>
        <div v-if="candidates.length" class="weather-candidates" aria-label="Location results">
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
          <span><strong>Selected</strong><small>Forecasts use saved coordinates and timezone</small></span>
          <span class="selected-location">
            <strong>{{ weatherLocationLabel || "Not configured" }}</strong>
            <small v-if="weatherLocationLabel">
              {{ [weatherAdmin1, weatherCountry].filter(Boolean).join(", ") || weatherTimezone }}
            </small>
          </span>
        </div>
        <label class="settings-row">
          <span><strong>Temperature</strong><small>Forecast and cache unit</small></span>
          <select v-model="temperatureUnit">
            <option value="celsius">Celsius · °C</option>
            <option value="fahrenheit">Fahrenheit · °F</option>
          </select>
        </label>
        <div class="settings-row weather-provider-row">
          <span><strong>Weather data</strong><small>CC BY 4.0 attribution</small></span>
          <button type="button" @click="openAttribution">Open-Meteo ↗</button>
        </div>
        <div class="weather-refresh-row">
          <button
            type="button"
            :disabled="!weather.configured || weather.refreshing"
            @click="$emit('refreshWeather')"
          >
            {{ weather.refreshing ? "Refreshing…" : "Refresh now" }}
          </button>
          <small v-if="weatherMessage" role="status">{{ weatherMessage }}</small>
        </div>
      </section>
      <section class="appearance-settings" aria-labelledby="appearance-settings-heading">
        <div class="settings-section-heading settings-major-heading">
          <strong id="appearance-settings-heading">APPEARANCE</strong>
          <small>Glass · Graphite Frost</small>
        </div>
        <div class="settings-row appearance-background-row">
          <span>
            <strong>Rendering</strong>
            <small>
              {{
                renderingBackend === "enhanced"
                  ? "Acrylic and transparent window effects. Screen-reader accessibility is currently limited."
                  : "Best compatibility and accessibility."
              }}
              <template v-if="renderingBackendChanged"> Restart the app to apply this change.</template>
            </small>
          </span>
          <div class="appearance-options" role="group" aria-label="Rendering backend">
            <button
              type="button"
              :class="{ active: renderingBackend === 'standard' }"
              @click="renderingBackend = 'standard'"
            >
              Standard
            </button>
            <button
              type="button"
              :class="{ active: renderingBackend === 'enhanced' }"
              @click="renderingBackend = 'enhanced'"
            >
              Enhanced transparency
            </button>
          </div>
        </div>

        <div class="settings-row appearance-background-row">
          <span><strong>Background</strong><small>Material style</small></span>
          <div class="appearance-options" role="group" aria-label="Background style">
            <button
              v-for="option in ['glass', 'solid', 'gradient', 'image', 'wallpaper'] as const"
              :key="option"
              type="button"
              :class="{ active: appearanceSettings.backgroundType === option }"
              @click="appearanceSettings.backgroundType = option"
            >
              {{ option[0]?.toUpperCase() + option.slice(1) }}
            </button>
          </div>
        </div>

        <template v-if="appearanceSettings.backgroundType === 'glass'">
          <label class="settings-row">
            <span><strong>Tint</strong><small>Graphite material color</small></span>
            <div class="color-control"><input v-model="appearanceSettings.glassTintColor" type="color" /><code>{{ appearanceSettings.glassTintColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>Tint opacity</strong><small>Background layer only</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.glassTintOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.glassTintOpacity * 100) }}%</output></div>
          </label>
          <label class="settings-row">
            <span><strong>Blur</strong><small>Bounded for stable rendering</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.blurPx" type="range" min="0" max="24" step="1" /><output>{{ appearanceSettings.blurPx }}px</output></div>
          </label>
        </template>

        <label v-if="appearanceSettings.backgroundType === 'solid'" class="settings-row">
          <span><strong>Color</strong><small>Solid · Graphite remains available</small></span>
          <div class="color-control"><input v-model="appearanceSettings.solidColor" type="color" /><code>{{ appearanceSettings.solidColor }}</code></div>
        </label>

        <template v-if="appearanceSettings.backgroundType === 'gradient'">
          <label class="settings-row">
            <span><strong>Start</strong><small>First gradient color</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientStartColor" type="color" /><code>{{ appearanceSettings.gradientStartColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>End</strong><small>Second gradient color</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientEndColor" type="color" /><code>{{ appearanceSettings.gradientEndColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>Angle</strong><small>0–360 degrees</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.gradientAngle" type="range" min="0" max="360" step="1" /><output>{{ appearanceSettings.gradientAngle }}°</output></div>
          </label>
        </template>

        <template v-if="appearanceSettings.backgroundType === 'image'">
          <div class="settings-row">
            <span><strong>Custom image</strong><small>PNG, JPEG, or WebP · local only</small></span>
            <div class="asset-actions">
              <button type="button" @click="chooseAsset('background')">Choose image</button>
              <small v-if="appearanceSettings.imageAssetId && !imagePreviewAvailable" class="asset-unavailable">Image unavailable · Glass fallback active</small>
            </div>
          </div>
          <label class="settings-row">
            <span><strong>Fit</strong><small>Image sizing</small></span>
            <select v-model="appearanceSettings.imageFit"><option value="cover">Cover</option><option value="contain">Contain</option><option value="stretch">Stretch</option></select>
          </label>
          <label class="settings-row">
            <span><strong>Position</strong><small>Image anchor</small></span>
            <select v-model="appearanceSettings.imagePosition"><option value="center">Center</option><option value="top">Top</option><option value="bottom">Bottom</option></select>
          </label>
        </template>

        <div v-if="appearanceSettings.backgroundType === 'wallpaper'" class="settings-row">
          <span><strong>Desktop wallpaper</strong><small>Read current Windows wallpaper once</small></span>
          <span class="asset-status">{{ wallpaperAvailable ? "Available" : "Unavailable · Glass fallback active" }}</span>
        </div>

        <label class="settings-row">
          <span><strong>Background opacity</strong><small>Content remains 100% opaque</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.backgroundOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.backgroundOpacity * 100) }}%</output></div>
        </label>
        <label class="settings-row">
          <span><strong>Overlay</strong><small>Readability protection</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.overlayStrength" type="range" min="0" max="0.72" step="0.01" /><output>{{ Math.round(appearanceSettings.overlayStrength * 100) }}%</output></div>
        </label>
        <label class="settings-row">
          <span><strong>Text contrast</strong><small>Auto can be overridden</small></span>
          <select v-model="appearanceSettings.textContrast"><option value="auto">Auto</option><option value="light">Light</option><option value="dark">Dark</option></select>
        </label>
        <button type="button" class="reset-appearance" @click="resetAppearance">↻ Reset appearance</button>
      </section>
      <p v-if="assetMessage" class="settings-error" role="status">{{ assetMessage }}</p>
    </div>

    <button type="button" class="save-settings" @click="save">Save settings</button>

    <section class="developer-section">
      <button type="button" class="developer-toggle" @click="developerOpen = !developerOpen">
        <span>Developer</span><span>{{ developerOpen ? "−" : "+" }}</span>
      </button>
      <div v-if="developerOpen" class="developer-content">
        <p class="database-path">SQLite · {{ databasePath }}</p>
        <DeveloperDiagnostics />
      </div>
    </section>
  </aside>
</template>
