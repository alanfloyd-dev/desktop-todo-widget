import { describe, expect, it, vi, type Mock } from "vitest";
import { browserWeatherState, createWeatherController, type WeatherBridge } from "./controller";
import type { WeatherViewState } from "../../types";

/**
 * The bridge's `invoke` is runtime-generic (any result type); a test double
 * with concrete return types needs the cast to fit that signature.
 */
function asInvoke(mock: Mock<(command: string) => Promise<unknown>>): WeatherBridge["invoke"] {
  return mock as unknown as WeatherBridge["invoke"];
}

function snapshotState(refreshing = false): WeatherViewState {
  return {
    configured: true,
    snapshot: {
      condition: "clear",
      temperature: 21,
      dailyHigh: 26,
      dailyLow: 14,
      precipitationProbability: 10,
      fetchedAt: 1_800_000_000,
      locationKey: "test",
      timezone: "UTC",
      temperatureUnit: "celsius",
    },
    cacheStatus: "fresh",
    cacheAgeSeconds: 30,
    refreshRecommended: false,
    refreshing,
    lastRefresh: "success",
  };
}

/** A bridge whose `refresh_weather` answer is under the test's control. */
function deferredBridge() {
  let resolveRefresh: (state: WeatherViewState) => void = () => {};
  const mock = vi.fn((command: string): Promise<unknown> => {
    if (command === "refresh_weather") {
      return new Promise<WeatherViewState>((resolve) => {
        resolveRefresh = resolve;
      });
    }
    if (command === "weather_state") {
      return Promise.resolve(snapshotState());
    }
    return Promise.resolve({});
  });
  const bridge: WeatherBridge = { available: true, invoke: asInvoke(mock) };
  // Late-bound on purpose: the capture reads the closure variable, which the
  // pending promise's executor replaces once the refresh starts.
  return { bridge, invoke: mock, resolveRefresh: (state: WeatherViewState) => resolveRefresh(state) };
}

describe("weather controller", () => {
  it("a manual refresh rides the existing refresh_weather command once", async () => {
    const harness = deferredBridge();
    const controller = createWeatherController(harness.bridge);

    const pending = controller.refreshWeather(true);
    expect(harness.invoke).toHaveBeenCalledWith("refresh_weather", { manual: true });
    expect(controller.weather.value.refreshing).toBe(true);

    harness.resolveRefresh(snapshotState());
    await pending;
    expect(harness.invoke).toHaveBeenCalledTimes(1);
    expect(controller.weather.value.refreshing).toBe(false);
    expect(controller.message.value).toEqual({ key: "status.weatherUpdated" });
  });

  it("a repeated refresh while one is in flight does not start a second request", async () => {
    const harness = deferredBridge();
    const controller = createWeatherController(harness.bridge);

    void controller.refreshWeather(true);
    void controller.refreshWeather(true);
    void controller.refreshWeather(true);
    expect(harness.invoke).toHaveBeenCalledTimes(1);

    harness.resolveRefresh(snapshotState());
    await Promise.resolve();
    await Promise.resolve();
    expect(harness.invoke).toHaveBeenCalledTimes(1);
  });

  it("a failed manual refresh keeps the previous snapshot and reports the failure", async () => {
    const keep = snapshotState();
    const mock = vi.fn((command: string): Promise<unknown> => {
      if (command === "refresh_weather") {
        return Promise.reject({ kind: "Timeout" });
      }
      if (command === "weather_state") {
        // The backend cache still serves the old snapshot.
        return Promise.resolve(keep);
      }
      return Promise.resolve({});
    });
    const controller = createWeatherController({ available: true, invoke: asInvoke(mock) });

    await controller.refreshWeather(true);
    expect(controller.weather.value.snapshot).toEqual(keep.snapshot);
    expect(controller.weather.value.configured).toBe(true);
    expect(controller.message.value).toEqual({ key: "error.weatherRefreshTimeout" });
    // The card re-syncs from the backend cache after a failure.
    expect(mock).toHaveBeenCalledWith("weather_state");
  });

  it("the backend refusal vocabulary maps to localized-copy keys", () => {
    const controller = createWeatherController({
      available: true,
      invoke: asInvoke(
        vi.fn((command: string): Promise<unknown> =>
          command === "refresh_weather"
            ? Promise.reject({ kind: "Cooldown" })
            : Promise.resolve({}),
        ),
      ),
    });
    return controller.refreshWeather(true).then(() => {
      expect(controller.message.value).toEqual({ key: "error.weatherRefreshCooldown" });
    });
  });

  it("a browser preview bridge never issues commands", async () => {
    const mock = vi.fn((command: string): Promise<unknown> => Promise.resolve({}));
    const controller = createWeatherController({ available: false, invoke: asInvoke(mock) });
    await controller.loadWeather();
    await controller.refreshWeather(true);
    expect(mock).not.toHaveBeenCalled();
    // The preview keeps its sample state.
    expect(controller.weather.value.configured).toBe(true);
    expect(controller.weather.value.snapshot?.locationKey).toBe("browser-preview");
  });

  it("the native fallback state starts unconfigured and idle", () => {
    const state = browserWeatherState(true);
    expect(state.configured).toBe(false);
    expect(state.refreshing).toBe(false);
    expect(state.snapshot).toBeNull();
  });
});
