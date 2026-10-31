import { describe, expect, it } from "vitest";
import {
  DEFAULT_MODULE_VISIBILITY,
  MODULE_REGISTRY,
  isModuleEnabled,
  resolveModuleVisibility,
  type ModuleVisibility,
} from "./registry";
import { en, zhHans } from "../i18n/catalog";

/**
 * A mutation probe for the readonly visibility type: the product code can
 * never write through `ModuleVisibility`, so only a cast reaches the runtime
 * behavior these tests pin.
 */
function asMutable(visibility: ModuleVisibility): Record<string, boolean> {
  return visibility as Record<string, boolean>;
}

describe("module registry", () => {
  it("declares exactly the five known module ids", () => {
    expect([...MODULE_REGISTRY].map((module) => module.id).sort()).toEqual([
      "github",
      "review",
      "shortcuts",
      "todo",
      "weather",
    ]);
  });

  it("has no duplicate module ids", () => {
    const ids = MODULE_REGISTRY.map((module) => module.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("classifies the audited surfaces: four dashboard sections and one overlay", () => {
    const surfaceOf = Object.fromEntries(
      MODULE_REGISTRY.map((module) => [module.id, module.surface]),
    );
    expect(surfaceOf).toEqual({
      todo: "dashboard",
      weather: "dashboard",
      shortcuts: "dashboard",
      github: "dashboard",
      review: "overlay",
    });
  });

  it("ships the four surface modules enabled, GitHub disabled, and labels all in both catalogs", () => {
    // The Amendment ① contract: the pre-GitHub surface ships exactly as
    // v1.2.x left it, and the network module stays out until the user asks
    // for it. A default flipped in either direction is an upgrade-behavior
    // change, so it is pinned per module rather than as one aggregate.
    const defaults = Object.fromEntries(
      MODULE_REGISTRY.map((module) => [module.id, module.defaultEnabled]),
    );
    expect(defaults).toEqual({
      todo: true,
      weather: true,
      shortcuts: true,
      review: true,
      github: false,
    });
    for (const module of MODULE_REGISTRY) {
      expect(en[module.labelKey], `${module.labelKey} missing from en`).toBeTruthy();
      expect(zhHans[module.labelKey], `${module.labelKey} missing from zh-Hans`).toBeTruthy();
    }
  });
});

describe("resolveModuleVisibility", () => {
  it("resolves missing preferences to the registry defaults (surface enabled, GitHub hidden)", () => {
    expect(resolveModuleVisibility(undefined)).toEqual(DEFAULT_MODULE_VISIBILITY);
    expect(resolveModuleVisibility(null)).toEqual(DEFAULT_MODULE_VISIBILITY);
    expect(DEFAULT_MODULE_VISIBILITY.github).toBe(false);
  });

  it("resolves a partially stated preference against the per-module default", () => {
    expect(resolveModuleVisibility({ weather: false })).toEqual({
      todo: true,
      weather: false,
      shortcuts: true,
      review: true,
      github: false,
    });
  });

  it("lets an explicit false disable exactly the selected module", () => {
    const visibility = resolveModuleVisibility({ todo: false, review: false });
    expect(visibility.todo).toBe(false);
    expect(visibility.review).toBe(false);
    expect(visibility.weather).toBe(true);
    expect(visibility.shortcuts).toBe(true);
  });

  it("keeps an explicit true enabled — including GitHub, the user's own choice", () => {
    expect(resolveModuleVisibility({ github: true }).github).toBe(true);
    expect(resolveModuleVisibility({ todo: true, github: true })).toEqual({
      ...DEFAULT_MODULE_VISIBILITY,
      github: true,
    });
  });

  it("ignores unknown preference keys and returns exactly the five known ids", () => {
    const stray = { news: false, todo: false } as Partial<ModuleVisibility>;
    const visibility = resolveModuleVisibility(stray);
    expect(Object.keys(visibility).sort()).toEqual([
      "github",
      "review",
      "shortcuts",
      "todo",
      "weather",
    ]);
    expect(visibility.todo).toBe(false);
  });

  it("never mutates its input", () => {
    const preferences = { weather: false };
    const snapshot = JSON.stringify(preferences);
    resolveModuleVisibility(preferences);
    expect(JSON.stringify(preferences)).toBe(snapshot);
  });

  it("returns a fresh object per call, so no result shares state with the default or another result", () => {
    const first = resolveModuleVisibility();
    const second = resolveModuleVisibility();
    expect(first).not.toBe(DEFAULT_MODULE_VISIBILITY);
    expect(first).not.toBe(second);

    asMutable(first).todo = false;
    expect(DEFAULT_MODULE_VISIBILITY.todo).toBe(true);
    expect(second.todo).toBe(true);
    expect(resolveModuleVisibility().todo).toBe(true);
  });
});

describe("isModuleEnabled", () => {
  it("reads the resolved visibility per module id", () => {
    const visibility = resolveModuleVisibility({ shortcuts: false });
    expect(isModuleEnabled(visibility, "shortcuts")).toBe(false);
    expect(isModuleEnabled(visibility, "todo")).toBe(true);
    expect(isModuleEnabled(visibility, "github")).toBe(false);
  });
});
