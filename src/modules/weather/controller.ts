import { ref, type Ref } from "vue";
import type { WeatherViewState } from "../../types";

/**
 * The bridge the weather controller runs through.
 *
 * The browser preview has no Tauri runtime; `available` is false there and
 * every command call is skipped, exactly as `App.vue` guarded before this
 * controller was extracted. Tests inject a fake bridge to drive the state
 * transitions deterministically.
 */
export interface WeatherBridge {
  available: boolean;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

/**
 * Weather status as a translation key, so it re-renders on language change.
 * The controller stores keys only; the owner resolves them with the app's
 * single i18n instance.
 */
export interface WeatherMessage {
  key: string;
  params?: Record<string, string | number>;
}

/**
 * The preview fallback state. The browser preview renders a static sample so
 * the surface stays inspectable without a backend; the native path starts
 * unconfigured and is populated by `weather_state` on startup.
 */
export function browserWeatherState(nativeBridgeAvailable: boolean): WeatherViewState {
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

/**
 * Maps a backend weather failure to localized copy. The backend speaks a
 * small, pinned vocabulary (see `weather.rs`); anything unrecognised becomes
 * the generic message rather than leaking a raw string into a localized UI.
 */
export function weatherErrorLabel(reason: unknown): WeatherMessage {
  const kind = typeof reason === "object" && reason && "kind" in reason
    ? String((reason as { kind: unknown }).kind)
    : String(reason);
  if (kind.includes("Cooldown")) return { key: "error.weatherRefreshCooldown" };
  if (kind.includes("InProgress")) return { key: "error.weatherRefreshInProgress" };
  if (kind.includes("Timeout")) return { key: "error.weatherRefreshTimeout" };
  return { key: "error.weatherRefreshGeneric" };
}

/**
 * The single weather state owner for the whole product surface.
 *
 * Extracted verbatim from `App.vue` so the state transitions are unit
 * testable without a component harness; `App.vue` binds the returned refs
 * and calls exactly the same functions as before. Ownership rules:
 *
 * - `weather` is the only copy of the `WeatherViewState`; both the main
 *   weather card and the Settings weather section render from it.
 * - `refreshWeather` is single-flight: while a refresh is in flight a second
 *   call is a no-op (the backend additionally single-flights and cools down
 *   manual refreshes; the frontend guard keeps the UI free of dead ends).
 * - A failed manual refresh keeps the previous snapshot (re-synced from the
 *   backend cache via `loadWeather`) and surfaces the failure as a message
 *   key; it never blanks the card.
 */
export function createWeatherController(bridge: WeatherBridge) {
  const weather = ref<WeatherViewState>(browserWeatherState(bridge.available));
  const message = ref<WeatherMessage | null>(null);

  async function loadWeather() {
    if (!bridge.available) return;
    try {
      weather.value = await bridge.invoke<WeatherViewState>("weather_state");
    } catch {
      // Weather is isolated from product startup; missing/corrupt cache must
      // not prevent Todo or window state from rendering.
      weather.value = browserWeatherState(false);
    }
  }

  async function refreshWeather(manual = false) {
    if (!bridge.available || weather.value.refreshing) return;
    message.value = null;
    weather.value.refreshing = true;
    try {
      weather.value = await bridge.invoke<WeatherViewState>("refresh_weather", { manual });
      if (manual) message.value = { key: "status.weatherUpdated" };
    } catch (reason) {
      if (manual) message.value = weatherErrorLabel(reason);
      await loadWeather();
    }
  }

  return {
    weather: weather as Ref<WeatherViewState>,
    message,
    loadWeather,
    refreshWeather,
  };
}
