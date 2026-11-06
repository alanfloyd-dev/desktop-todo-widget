import { describe, expect, it } from "vitest";
import { settingsDraftIsDirty, settingsSnapshot, type SettingsDraftSnapshot } from "./dirty";
import type { AppearanceSettings, AppearanceProfiles, ProductSettings } from "../../types";

/**
 * The dirty check's regression boundary (S1-B): a draft equal to the opened
 * settings is never dirty, every persisted field's change is detected, the
 * backend's trim normalization never reads as a change, and UI-only state is
 * structurally incapable of making the panel dirty.
 */

function appearanceProfile(overrides: Partial<AppearanceSettings> = {}): AppearanceSettings {
  return {
    backgroundType: "glass",
    solidColor: "#11191e",
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
    customTextColor: "#d3dade",
    sampledLuminance: null,
    ...overrides,
  };
}

function appearanceProfiles(
  overrides: Partial<Record<"sidebar" | "floating" | "desktop", AppearanceSettings>> = {},
): AppearanceProfiles {
  return {
    sidebar: overrides.sidebar ?? appearanceProfile(),
    floating: overrides.floating ?? appearanceProfile(),
    desktop: overrides.desktop ?? appearanceProfile(),
  };
}

function snapshot(overrides: Partial<SettingsDraftSnapshot> = {}): SettingsDraftSnapshot {
  return {
    dayRollover: "04:00",
    weatherLocationLabel: "Shanghai",
    weatherLatitude: 31.2,
    weatherLongitude: 121.5,
    weatherTimezone: "Asia/Shanghai",
    weatherCountry: "CN",
    weatherAdmin1: "Shanghai",
    temperatureUnit: "celsius",
    language: "system",
    appearance: "geological_observatory",
    appearanceProfiles: appearanceProfiles(),
    displayName: "User",
    avatarAssetId: null,
    quickLinks: [
      { id: "a", name: "Docs", url: "https://example.com/docs" },
      { id: "b", name: "Mail", url: "https://example.com/mail" },
    ],
    moduleVisibility: { todo: true, weather: true, shortcuts: true, review: true, github: false },
    githubUsername: "octocat",
    ...overrides,
  };
}

function storedSettings(overrides: Partial<ProductSettings> = {}): ProductSettings {
  return {
    geometryUnitsVersion: 1,
    mode: "floating",
    x: null,
    y: null,
    width: 620,
    height: 720,
    monitorIdentity: null,
    floatingPresentation: "expanded",
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
    weatherLocationLabel: "Shanghai",
    weatherLatitude: 31.2,
    weatherLongitude: 121.5,
    weatherTimezone: "Asia/Shanghai",
    weatherCountry: "CN",
    weatherAdmin1: "Shanghai",
    temperatureUnit: "celsius",
    language: "system",
    appearance: "geological_observatory",
    appearanceProfiles: appearanceProfiles(),
    displayName: "User",
    avatarAssetId: null,
    quickLinks: [
      { id: "a", name: "Docs", url: "https://example.com/docs" },
      { id: "b", name: "Mail", url: "https://example.com/mail" },
    ],
    moduleVisibility: { todo: true, weather: true, shortcuts: true, review: true, github: false },
    moduleLayout: { version: 1, entries: [] },
    githubUsername: "octocat",
    ...overrides,
  };
}

describe("settingsDraftIsDirty", () => {
  it("an untouched draft is not dirty", () => {
    const original = settingsSnapshot(storedSettings());
    expect(settingsDraftIsDirty(snapshot(), original)).toBe(false);
  });

  it("detects each scalar preference change", () => {
    const original = settingsSnapshot(storedSettings());
    const cases: Partial<SettingsDraftSnapshot>[] = [
      { dayRollover: "06:30" },
      { weatherLatitude: 31.3 },
      { weatherLongitude: 121.6 },
      { temperatureUnit: "fahrenheit" },
      { language: "en" },
      { appearance: "something_else" },
      { avatarAssetId: "asset-1" },
      { githubUsername: "octo-cat" },
      { weatherLocationLabel: "Beijing" },
      { weatherTimezone: "Asia/Beijing" },
      { weatherCountry: "" },
      { weatherAdmin1: "" },
    ];
    for (const overrides of cases) {
      expect(
        settingsDraftIsDirty(snapshot(overrides), original),
        `expected dirty for ${JSON.stringify(overrides)}`,
      ).toBe(true);
    }
  });

  it("whitespace the backend trims on save is not a change", () => {
    const original = settingsSnapshot(storedSettings());
    expect(settingsDraftIsDirty(snapshot({ displayName: " User  " }), original)).toBe(false);
    expect(settingsDraftIsDirty(snapshot({ githubUsername: "  octocat " }), original)).toBe(false);
    expect(settingsDraftIsDirty(snapshot({ weatherLocationLabel: " Shanghai " }), original)).toBe(
      false,
    );
    // A real edit under the same trim rule is still dirty.
    expect(settingsDraftIsDirty(snapshot({ displayName: "User 2" }), original)).toBe(true);
  });

  it("quick links compare by identity, content, and order", () => {
    const original = settingsSnapshot(storedSettings());
    expect(settingsDraftIsDirty(snapshot(), original)).toBe(false);
    // Reorder: same rows, different order.
    const reordered = [
      { id: "b", name: "Mail", url: "https://example.com/mail" },
      { id: "a", name: "Docs", url: "https://example.com/docs" },
    ];
    expect(settingsDraftIsDirty(snapshot({ quickLinks: reordered }), original)).toBe(true);
    // Edit in place.
    const renamed = [
      { id: "a", name: "Docs", url: "https://example.com/docs" },
      { id: "b", name: "Mail", url: "https://example.com/mail/" },
    ];
    expect(settingsDraftIsDirty(snapshot({ quickLinks: renamed }), original)).toBe(true);
    // Delete one.
    expect(settingsDraftIsDirty(snapshot({ quickLinks: [renamed[0]] }), original)).toBe(true);
  });

  it("module visibility toggles are dirty and restoring clears it", () => {
    const original = settingsSnapshot(storedSettings());
    const toggled = snapshot({
      moduleVisibility: { todo: true, weather: true, shortcuts: true, review: true, github: true },
    });
    expect(settingsDraftIsDirty(toggled, original)).toBe(true);
    expect(settingsDraftIsDirty(snapshot(), original)).toBe(false);
  });

  it("an appearance edit in one profile is dirty; the other profiles stay independent", () => {
    const original = settingsSnapshot(storedSettings());
    const changed = appearanceProfiles({
      floating: appearanceProfile({ gradientAngle: 90 }),
    });
    expect(settingsDraftIsDirty(snapshot({ appearanceProfiles: changed }), original)).toBe(true);
    // A different mode's edit does not match either.
    const sidebarOnly = appearanceProfiles({
      sidebar: appearanceProfile({ backgroundType: "solid", solidColor: "#000000" }),
    });
    expect(settingsDraftIsDirty(snapshot({ appearanceProfiles: sidebarOnly }), original)).toBe(
      true,
    );
  });

  it("every persisted profile member participates, including sampled luminance", () => {
    const original = settingsSnapshot(storedSettings());
    const withLuminance = appearanceProfiles({
      desktop: appearanceProfile({ sampledLuminance: 0.5 }),
    });
    expect(settingsDraftIsDirty(snapshot({ appearanceProfiles: withLuminance }), original)).toBe(
      true,
    );
  });

  it("resetting an edited value back to the original clears the dirty state", () => {
    const original = settingsSnapshot(storedSettings());
    const edited = snapshot({ displayName: "Example", temperatureUnit: "fahrenheit" });
    expect(settingsDraftIsDirty(edited, original)).toBe(true);
    const restored = snapshot({ displayName: "User", temperatureUnit: "celsius" });
    expect(settingsDraftIsDirty(restored, original)).toBe(false);
  });
});

describe("settingsSnapshot", () => {
  it("normalizes the language preference the same way the draft does", () => {
    // A legacy or hand-shaped stored value resolves to `system`, matching the
    // draft's initialization, so an old document does not open dirty.
    const original = settingsSnapshot(storedSettings({ language: "fr" as never }));
    expect(settingsDraftIsDirty(snapshot(), original)).toBe(false);
  });

  it("resolves module visibility through the registry policy", () => {
    // A document missing members resolves to all-enabled (GitHub's registry
    // default included), so the resolved comparison matches the panel draft.
    const original = settingsSnapshot(
      storedSettings({ moduleVisibility: {} as ProductSettings["moduleVisibility"] }),
    );
    const resolvedDraft = snapshot({
      moduleVisibility: { todo: true, weather: true, shortcuts: true, review: true, github: false },
    });
    expect(settingsDraftIsDirty(resolvedDraft, original)).toBe(false);
  });
});
