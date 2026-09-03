<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import ContextMenu from "./components/ContextMenu.vue";
import AppearanceBackground from "./components/AppearanceBackground.vue";
import FloatingOrb from "./components/FloatingOrb.vue";
import ProductContent from "./components/ProductContent.vue";
import ReviewPanel from "./components/ReviewPanel.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import {
  browserRepresentativeLuminance,
  browserResolvedContrast,
  sampleImageLuminance,
} from "./appearance";
import type {
  AppearanceSettings,
  AssetPayload,
  ContrastResult,
  ProductSettings,
  ProductViewState,
  ResolvedContrast,
  TemperatureUnit,
  WeatherViewState,
} from "./types";

const nativeBridgeAvailable = "__TAURI_INTERNALS__" in window;
const state = ref<ProductViewState>(browserState());
const error = ref("");
const settingsOpen = ref(false);
const reviewOpen = ref(false);
const weatherSettingsRequested = ref(false);
const weather = ref<WeatherViewState>(browserWeatherState());
const weatherMessage = ref("");
const imageAsset = ref<AssetPayload>(emptyAsset());
const wallpaperAsset = ref<AssetPayload>(emptyAsset());
const avatarAsset = ref<AssetPayload>(emptyAsset());
const resolvedContrast = ref<ResolvedContrast>("light");
const menu = ref<{ open: boolean; x: number; y: number }>({ open: false, x: 0, y: 0 });
let unlistenSettings: UnlistenFn | undefined;
let weatherTimer: number | undefined;

const modeClass = computed(() => `mode-${state.value.settings.mode}`);
const presentationClass = computed(
  () => `presentation-${state.value.settings.floatingPresentation}`,
);
const floatingCollapsed = computed(
  () =>
    state.value.settings.mode === "floating" &&
    state.value.settings.floatingPresentation === "collapsed",
);

function emptyAsset(assetId: string | null = null): AssetPayload {
  return { assetId, available: false, dataUrl: null };
}

function browserState(): ProductViewState {
  const preview = new URLSearchParams(window.location.search);
  const requestedMode = preview.get("mode");
  const mode = requestedMode === "sidebar" || requestedMode === "desktop"
    ? requestedMode
    : "floating";
  const presentation = preview.get("presentation") === "collapsed" ? "collapsed" : "expanded";
  const background = preview.get("background");
  const backgroundType = ["glass", "solid", "gradient", "image", "wallpaper"].includes(background ?? "")
    ? background as AppearanceSettings["backgroundType"]
    : "glass";
  const lightSolid = preview.get("palette") === "light";
  return {
    settings: {
      geometryUnitsVersion: 1,
      mode,
      x: null,
      y: null,
      width: window.innerWidth,
      height: window.innerHeight,
      monitorIdentity: null,
      floatingPresentation: presentation,
      floatingOrbX: null,
      floatingOrbY: null,
      floatingOrbMonitorIdentity: null,
      desktopX: null,
      desktopY: null,
      desktopWidth: 420,
      desktopHeight: 700,
      sidebarSide: "left",
      sidebarWidth: 380,
      alwaysOnTop: false,
      locked: false,
      dayRollover: "04:00",
      weatherLocationLabel: "",
      weatherLatitude: null,
      weatherLongitude: null,
      weatherTimezone: "",
      weatherCountry: "",
      weatherAdmin1: "",
      temperatureUnit: "celsius",
      appearance: "geological_observatory",
      appearanceSettings: {
        backgroundType,
        solidColor: lightSolid ? "#f3f5f6" : "#11191e",
        glassTintColor: "#11191e",
        glassTintOpacity: 0.78,
        blurPx: 14,
        overlayStrength: 0.18,
        gradientStartColor: "#11191e",
        gradientEndColor: "#213747",
        gradientAngle: 135,
        imageAssetId: null,
        imageFit: "cover",
        imagePosition: "center",
        backgroundOpacity: 0.94,
        textContrast: "auto",
        sampledLuminance: null,
      },
      displayName: "Your Name",
      avatarAssetId: null,
      homepageLabel: "Homepage",
      homepageUrl: "",
    },
    desktopExperimental: true,
    databasePath: "browser preview · app-data/alan-desktop.sqlite3",
  };
}

function browserWeatherState(): WeatherViewState {
  if (!nativeBridgeAvailable) {
    return {
      configured: true,
      snapshot: {
        condition: "clear",
        temperature: 28,
        dailyHigh: 33,
        dailyLow: 22,
        precipitationProbability: 2,
        fetchedAt: Math.floor(Date.now() / 1000),
        locationKey: "browser-preview",
        timezone: "Asia/Shanghai",
        temperatureUnit: "celsius",
      },
      cacheStatus: "fresh",
      cacheAgeSeconds: 0,
      refreshRecommended: false,
      refreshing: false,
      lastRefresh: "browser-preview",
    };
  }
  return {
    configured: false,
    snapshot: null,
    cacheStatus: "missing",
    cacheAgeSeconds: null,
    refreshRecommended: false,
    refreshing: false,
    lastRefresh: "none",
  };
}

async function loadState() {
  if (!nativeBridgeAvailable) return;
  try {
    state.value = await invoke<ProductViewState>("product_state");
  } catch (reason) {
    error.value = String(reason);
  }
}

async function loadAppearanceRuntime() {
  const settings = state.value.settings;
  imageAsset.value = emptyAsset(settings.appearanceSettings.imageAssetId);
  wallpaperAsset.value = emptyAsset();
  avatarAsset.value = emptyAsset(settings.avatarAssetId);
  if (nativeBridgeAvailable) {
    try {
      if (settings.appearanceSettings.imageAssetId) {
        imageAsset.value = await invoke<AssetPayload>("load_managed_asset", {
          kind: "background",
          assetId: settings.appearanceSettings.imageAssetId,
        });
      }
      if (["glass", "wallpaper"].includes(settings.appearanceSettings.backgroundType)) {
        wallpaperAsset.value = await invoke<AssetPayload>("load_windows_wallpaper");
      }
      if (settings.avatarAssetId) {
        avatarAsset.value = await invoke<AssetPayload>("load_managed_asset", {
          kind: "avatar",
          assetId: settings.avatarAssetId,
        });
      }
    } catch {
      // Assets are optional presentation data. A damaged file or unavailable
      // wallpaper must leave the local-first Todo/Weather surface usable.
    }
  }
  const appearance = settings.appearanceSettings;
  const sampleUrl = appearance.backgroundType === "image"
    ? imageAsset.value.dataUrl
    : ["glass", "wallpaper"].includes(appearance.backgroundType)
      ? wallpaperAsset.value.dataUrl
      : null;
  const sampled = sampleUrl ? await sampleImageLuminance(sampleUrl) : appearance.sampledLuminance;
  if (nativeBridgeAvailable) {
    try {
      const result = await invoke<ContrastResult>("resolve_appearance_contrast", {
        appearance,
        sampledLuminance: sampled,
        previous: resolvedContrast.value,
      });
      resolvedContrast.value = result.resolved;
      return;
    } catch {
      // The browser fallback below is the same bounded luminance policy.
    }
  }
  const representative = browserRepresentativeLuminance(appearance, sampled);
  resolvedContrast.value = browserResolvedContrast(
    appearance.textContrast,
    representative,
    resolvedContrast.value,
  );
}

async function loadWeather() {
  if (!nativeBridgeAvailable) return;
  try {
    weather.value = await invoke<WeatherViewState>("weather_state");
  } catch {
    // Weather is isolated from product startup; missing/corrupt cache must not
    // prevent Todo or window state from rendering.
    weather.value = browserWeatherState();
  }
}

function weatherErrorLabel(reason: unknown) {
  const kind = typeof reason === "object" && reason && "kind" in reason
    ? String((reason as { kind: unknown }).kind)
    : String(reason);
  if (kind.includes("Cooldown")) return "Please wait before refreshing again.";
  if (kind.includes("InProgress")) return "Weather refresh is already running.";
  if (kind.includes("Timeout")) return "Weather request timed out. Cached data is unchanged.";
  return "Weather could not refresh. Cached data is unchanged.";
}

async function refreshWeather(manual = false) {
  if (!nativeBridgeAvailable || weather.value.refreshing) return;
  weatherMessage.value = "";
  weather.value.refreshing = true;
  try {
    weather.value = await invoke<WeatherViewState>("refresh_weather", { manual });
    if (manual) weatherMessage.value = "Weather updated.";
  } catch (reason) {
    if (manual) weatherMessage.value = weatherErrorLabel(reason);
    await loadWeather();
  }
}

function openWeatherSettings() {
  reviewOpen.value = false;
  weatherSettingsRequested.value = true;
  settingsOpen.value = true;
  menu.value.open = false;
}

function openContextMenu(event: MouseEvent) {
  if (floatingCollapsed.value && nativeBridgeAvailable) {
    void invoke("show_product_context_menu").catch((reason) => {
      error.value = String(reason);
    });
    return;
  }
  const menuWidth = 236;
  const menuHeight = state.value.settings.mode === "sidebar" ? 390 : 354;
  menu.value = {
    open: true,
    x: Math.max(10, Math.min(event.clientX, window.innerWidth - menuWidth - 10)),
    y: Math.max(10, Math.min(event.clientY, window.innerHeight - menuHeight - 10)),
  };
}

function applyBrowserAction(action: string) {
  const settings = state.value.settings;
  if (action.startsWith("mode.")) {
    settings.mode = action.slice(5) as ProductSettings["mode"];
  } else if (action === "side.left" || action === "side.right") {
    settings.sidebarSide = action.endsWith("left") ? "left" : "right";
  } else if (action === "lock.toggle") {
    settings.locked = !settings.locked;
  } else if (action === "always_on_top.toggle" && settings.mode !== "desktop") {
    settings.alwaysOnTop = !settings.alwaysOnTop;
  } else if (action === "settings") {
    reviewOpen.value = false;
    settingsOpen.value = true;
  } else if (action === "floating.expand") {
    settings.floatingPresentation = "expanded";
  } else if (action === "floating.collapse") {
    settings.floatingPresentation = "collapsed";
  }
}

async function runAction(action: string) {
  error.value = "";
  menu.value.open = false;
  if (action === "settings") {
    reviewOpen.value = false;
    settingsOpen.value = true;
  }
  if (!nativeBridgeAvailable) {
    applyBrowserAction(action);
    return;
  }
  try {
    state.value = await invoke<ProductViewState>("product_action", { action });
    if (action.startsWith("floating.")) await loadAppearanceRuntime();
  } catch (reason) {
    error.value = String(reason);
  }
}

async function openHomepage() {
  if (!state.value.settings.homepageUrl) return;
  if (!nativeBridgeAvailable) {
    window.open(state.value.settings.homepageUrl, "_blank", "noopener,noreferrer");
    return;
  }
  try {
    await invoke("open_shortcut", { id: "homepage" });
  } catch (reason) {
    error.value = String(reason);
  }
}

async function saveSettings(patch: {
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
}) {
  const active = state.value.settings;
  const weatherIdentityChanged =
    active.weatherLatitude !== patch.weatherLatitude ||
    active.weatherLongitude !== patch.weatherLongitude ||
    active.weatherTimezone !== patch.weatherTimezone ||
    active.temperatureUnit !== patch.temperatureUnit;
  if (weatherIdentityChanged) {
    // Never let a previous location/unit snapshot survive the instant at
    // which the active settings identity changes. Matching SQLite cache will
    // be loaded immediately after persistence, otherwise the UI stays empty
    // while the new identity refreshes.
    weather.value = {
      ...browserWeatherState(),
      configured: patch.weatherLatitude !== null && patch.weatherLongitude !== null,
      refreshRecommended: patch.weatherLatitude !== null && patch.weatherLongitude !== null,
    };
  }
  if (!nativeBridgeAvailable) {
    Object.assign(state.value.settings, patch);
    settingsOpen.value = false;
    reviewOpen.value = false;
    await loadAppearanceRuntime();
    return;
  }
  try {
    state.value = await invoke<ProductViewState>("update_product_settings", { patch });
    settingsOpen.value = false;
    weatherSettingsRequested.value = false;
    await loadAppearanceRuntime();
    await loadWeather();
    if (weather.value.configured && weather.value.refreshRecommended) {
      void refreshWeather(false);
    }
  } catch (reason) {
    error.value = String(reason);
  }
}

function closeMenu(event?: Event) {
  if (event?.target instanceof Element && event.target.closest(".context-menu")) return;
  menu.value.open = false;
}

function handleKey(event: KeyboardEvent) {
  if (event.key === "Escape") {
    menu.value.open = false;
    settingsOpen.value = false;
    reviewOpen.value = false;
  }
}

onMounted(async () => {
  await loadState();
  await loadAppearanceRuntime();
  await loadWeather();
  if (weather.value.configured && weather.value.refreshRecommended) {
    void refreshWeather(false);
  }
  weatherTimer = window.setInterval(() => {
    void loadWeather().then(() => {
      if (weather.value.configured && weather.value.refreshRecommended) {
        void refreshWeather(false);
      }
    });
  }, 60 * 60 * 1000);
  window.addEventListener("pointerdown", closeMenu);
  window.addEventListener("blur", closeMenu);
  window.addEventListener("keydown", handleKey);
  if (nativeBridgeAvailable) {
    unlistenSettings = await listen("open-settings", () => {
      settingsOpen.value = true;
      reviewOpen.value = false;
      menu.value.open = false;
    });
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", closeMenu);
  window.removeEventListener("blur", closeMenu);
  window.removeEventListener("keydown", handleKey);
  unlistenSettings?.();
  if (weatherTimer !== undefined) window.clearInterval(weatherTimer);
});
</script>

<template>
  <main
    :class="['app-shell', modeClass, presentationClass, `contrast-${resolvedContrast}`]"
    :data-window-mode="state.settings.mode"
    :data-floating-presentation="state.settings.floatingPresentation"
    @contextmenu.prevent="openContextMenu"
  >
    <AppearanceBackground
      :appearance="state.settings.appearanceSettings"
      :image-url="imageAsset.dataUrl || ''"
      :image-available="imageAsset.available"
      :wallpaper-url="wallpaperAsset.dataUrl || ''"
      :wallpaper-available="wallpaperAsset.available"
      :resolved-contrast="resolvedContrast"
    />
    <p v-if="error" class="app-error" role="alert">{{ error }}</p>

    <div class="appearance-content-layer">
      <FloatingOrb
        v-if="floatingCollapsed"
        :avatar-url="avatarAsset.dataUrl || ''"
        :avatar-available="avatarAsset.available"
        :display-name="state.settings.displayName"
        :locked="state.settings.locked"
        :native-bridge-available="nativeBridgeAvailable"
        @expand="runAction('floating.expand')"
        @error="error = $event"
      />

      <ProductContent
        v-else
        :key="state.settings.dayRollover"
        :mode="state.settings.mode"
        :locked="state.settings.locked"
        :display-name="state.settings.displayName"
        :avatar-url="avatarAsset.dataUrl || ''"
        :avatar-available="avatarAsset.available"
        :homepage-label="state.settings.homepageLabel"
        :homepage-url="state.settings.homepageUrl"
        :native-bridge-available="nativeBridgeAvailable"
        :weather="weather"
        @open-homepage="openHomepage"
        @collapse="runAction('floating.collapse')"
        @configure-weather="openWeatherSettings"
        @open-review="settingsOpen = false; reviewOpen = true"
        @error="error = $event"
      />

      <ContextMenu
      v-if="menu.open"
      :settings="state.settings"
      :x="menu.x"
      :y="menu.y"
      @select="runAction"
      />

      <SettingsPanel
      v-if="settingsOpen"
      :settings="state.settings"
      :database-path="state.databasePath"
      :weather="weather"
      :weather-message="weatherMessage"
      :focus-weather="weatherSettingsRequested"
      @close="settingsOpen = false"
      @save="saveSettings"
      @refresh-weather="refreshWeather(true)"
        :image-available="imageAsset.available"
        :wallpaper-available="wallpaperAsset.available"
        :avatar-url="avatarAsset.dataUrl || ''"
        :avatar-available="avatarAsset.available"
      />

      <ReviewPanel
        v-if="reviewOpen"
        :native-bridge-available="nativeBridgeAvailable"
        @close="reviewOpen = false"
        @error="error = $event"
      />
    </div>
  </main>
</template>
