<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import ContextMenu from "./components/ContextMenu.vue";
import AppearanceBackground from "./components/AppearanceBackground.vue";
import FloatingOrb from "./components/FloatingOrb.vue";
import ProductContent from "./components/ProductContent.vue";
import ReviewPanel from "./components/ReviewPanel.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import {
  activeAppearance,
  browserRepresentativeLuminance,
  browserResolvedContrast,
  customTextVariables,
  defaultAppearanceProfiles,
  sampleImageLuminance,
} from "./appearance";
import { createI18n, normalizeLanguage, provideI18n } from "./i18n";
import type {
  AppearanceProfiles,
  AppearanceSettings,
  AssetPayload,
  ContrastResult,
  Language,
  ProductSettings,
  ProductViewState,
  QuickLink,
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
/** Weather status as a translation key, so it re-renders on language change. */
const weatherMessageKey = ref<{ key: string; params?: Record<string, string | number> } | null>(
  null,
);
const weatherMessage = computed(() =>
  weatherMessageKey.value ? t(weatherMessageKey.value.key, weatherMessageKey.value.params) : "",
);
const imageAsset = ref<AssetPayload>(emptyAsset());
const wallpaperAsset = ref<AssetPayload>(emptyAsset());
const avatarAsset = ref<AssetPayload>(emptyAsset());
const resolvedContrast = ref<ResolvedContrast>("light");
const menu = ref<{ open: boolean; x: number; y: number }>({ open: false, x: 0, y: 0 });
let unlistenSettings: UnlistenFn | undefined;
let unlistenProductState: UnlistenFn | undefined;
let weatherTimer: number | undefined;

/**
 * The app owns the single i18n instance and provides it to every child, so
 * there is exactly one reactive locale for the whole product surface.
 */
const i18n = provideI18n(
  createI18n(normalizeLanguage(state.value.settings.language), state.value.systemLocale),
);
const t = i18n.t;

watch(
  () => state.value.settings.language,
  (language) => i18n.setLanguage(normalizeLanguage(language)),
);
watch(
  () => state.value.systemLocale,
  (systemLocale) => i18n.setSystemLocale(systemLocale ?? ""),
);
watch(
  i18n.locale,
  (locale) => {
    document.documentElement.lang = locale;
  },
  { immediate: true },
);

const modeClass = computed(() => `mode-${state.value.settings.mode}`);
/**
 * The appearance profile of the window mode the product is in.
 *
 * Sidebar, Floating, and Desktop each own one, so this is the single place that
 * decides which profile the background, the contrast resolution, and the custom
 * text colour read from.
 */
const appearance = computed<AppearanceSettings>(() => activeAppearance(state.value.settings));
/** Custom text variables, or `undefined` when the profile is not on Custom. */
const customTextStyle = computed(() => customTextVariables(appearance.value));
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
  // The browser preview has no persisted profiles, so every mode starts from the
  // same draft; `resolve_appearance_contrast` and the profile selector are still
  // exercised because the draft is stored per mode.
  const appearanceProfiles = defaultAppearanceProfiles();
  for (const profile of Object.values(appearanceProfiles)) {
    profile.backgroundType = backgroundType;
    profile.solidColor = lightSolid ? "#f3f5f6" : "#11191e";
  }
  return {
    settings: {
      geometryUnitsVersion: 1,
      mode,
      // Browser preview is not a hosted window; it always uses the compatibility path.
      renderingBackend: "standard",
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
      language: "system",
      appearance: "geological_observatory",
      appearanceProfiles,
      // Browser preview only. The native default comes from `ProductSettings`;
      // both are the same neutral placeholder, never a personal identity.
      displayName: "User",
      avatarAssetId: null,
      quickLinks: [],
    },
    desktopExperimental: true,
    databasePath: "browser preview · app-data/alan-desktop.sqlite3",
    // In the browser preview the navigator locale stands in for the OS locale
    // that the native build reads from the registry.
    systemLocale: navigator.language ?? "",
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
  const profile = activeAppearance(settings);
  imageAsset.value = emptyAsset(profile.imageAssetId);
  wallpaperAsset.value = emptyAsset();
  avatarAsset.value = emptyAsset(settings.avatarAssetId);
  if (nativeBridgeAvailable) {
    try {
      if (profile.imageAssetId) {
        imageAsset.value = await invoke<AssetPayload>("load_managed_asset", {
          kind: "background",
          assetId: profile.imageAssetId,
        });
      }
      if (["glass", "wallpaper"].includes(profile.backgroundType)) {
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
  const sampleUrl = profile.backgroundType === "image"
    ? imageAsset.value.dataUrl
    : ["glass", "wallpaper"].includes(profile.backgroundType)
      ? wallpaperAsset.value.dataUrl
      : null;
  const sampled = sampleUrl ? await sampleImageLuminance(sampleUrl) : profile.sampledLuminance;
  if (nativeBridgeAvailable) {
    try {
      const result = await invoke<ContrastResult>("resolve_appearance_contrast", {
        appearance: profile,
        sampledLuminance: sampled,
        previous: resolvedContrast.value,
      });
      resolvedContrast.value = result.resolved;
      return;
    } catch {
      // The browser fallback below is the same bounded luminance policy.
    }
  }
  const representative = browserRepresentativeLuminance(profile, sampled);
  resolvedContrast.value = browserResolvedContrast(
    profile,
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
  if (kind.includes("Cooldown")) return { key: "error.weatherRefreshCooldown" };
  if (kind.includes("InProgress")) return { key: "error.weatherRefreshInProgress" };
  if (kind.includes("Timeout")) return { key: "error.weatherRefreshTimeout" };
  return { key: "error.weatherRefreshGeneric" };
}

async function refreshWeather(manual = false) {
  if (!nativeBridgeAvailable || weather.value.refreshing) return;
  weatherMessageKey.value = null;
  weather.value.refreshing = true;
  try {
    weather.value = await invoke<WeatherViewState>("refresh_weather", { manual });
    if (manual) weatherMessageKey.value = { key: "status.weatherUpdated" };
  } catch (reason) {
    if (manual) weatherMessageKey.value = weatherErrorLabel(reason);
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
  } else if (action === "github") {
    // The project page is opened by the native shell (`product_action`), which the
    // browser preview has no equivalent of. Nothing to apply to the preview state.
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
    await loadAppearanceRuntime();
    return;
  }
  try {
    state.value = await invoke<ProductViewState>("product_action", { action });
    // A mode action changes which profile is current, and a Floating action
    // changes the presentation the wallpaper/image is sampled against. Either
    // way the runtime appearance has to be rebuilt from the new profile instead
    // of keeping the previous mode's material.
    if (action.startsWith("floating.") || action.startsWith("mode.")) {
      await loadAppearanceRuntime();
    }
  } catch (reason) {
    error.value = String(reason);
  }
}

async function openQuickLink(id: string) {
  const link = state.value.settings.quickLinks.find((candidate) => candidate.id === id);
  if (!link) return;
  if (!nativeBridgeAvailable) {
    // Browser preview only. The native path hands the URL to the Shell so the
    // system default browser opens it; this stand-in has no Shell to hand it to.
    window.open(link.url, "_blank", "noopener,noreferrer");
    return;
  }
  try {
    await invoke("open_quick_link", { id });
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
  language: Language;
  appearance: string;
  appearanceProfiles: AppearanceProfiles;
  displayName: string;
  avatarAssetId: string | null;
  quickLinks: QuickLink[];
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
    // The backend publishes the authoritative product state after every product
    // action. Native menus (the tray and the Orb's context menu) cannot return
    // that result to us, so this event is the only way a natively triggered mode
    // or presentation change reaches the DOM: without it the layout keeps
    // rendering the previous mode inside the new mode's window geometry. A mode
    // change also swaps the appearance profile, so the material has to be
    // reloaded here too — there is no other hook for a native mode switch.
    unlistenProductState = await listen<ProductViewState>("product-state", (event) => {
      const modeChanged = event.payload.settings.mode !== state.value.settings.mode;
      state.value = event.payload;
      menu.value.open = false;
      if (modeChanged) void loadAppearanceRuntime();
    });
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", closeMenu);
  window.removeEventListener("blur", closeMenu);
  window.removeEventListener("keydown", handleKey);
  unlistenSettings?.();
  unlistenProductState?.();
  if (weatherTimer !== undefined) window.clearInterval(weatherTimer);
});
</script>

<template>
  <main
    :class="['app-shell', modeClass, presentationClass, `contrast-${resolvedContrast}`, { 'text-custom': !!customTextStyle }]"
    :style="customTextStyle"
    :data-window-mode="state.settings.mode"
    :data-floating-presentation="state.settings.floatingPresentation"
    @contextmenu.prevent="openContextMenu"
  >
    <AppearanceBackground
      :appearance="appearance"
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
        :quick-links="state.settings.quickLinks"
        :native-bridge-available="nativeBridgeAvailable"
        :weather="weather"
        @open-quick-link="openQuickLink"
        @collapse="runAction('floating.collapse')"
        @open-settings="runAction('settings')"
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
