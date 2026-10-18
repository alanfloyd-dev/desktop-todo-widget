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
  it("declares exactly the four known module ids", () => {
    expect([...MODULE_REGISTRY].map((module) => module.id).sort()).toEqual([
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

  it("classifies the audited surfaces: three dashboard sections and one overlay", () => {
    const surfaceOf = Object.fromEntries(
      MODULE_REGISTRY.map((module) => [module.id, module.surface]),
    );
    expect(surfaceOf).toEqual({
      todo: "dashboard",
      weather: "dashboard",
      shortcuts: "dashboard",
      review: "overlay",
    });
  });

  it("ships every module enabled and labels it in both catalogs", () => {
    for (const module of MODULE_REGISTRY) {
      expect(module.defaultEnabled, `${module.id} must default to enabled`).toBe(true);
      expect(en[module.labelKey], `${module.labelKey} missing from en`).toBeTruthy();
      expect(zhHans[module.labelKey], `${module.labelKey} missing from zh-Hans`).toBeTruthy();
    }
  });
});

describe("resolveModuleVisibility", () => {
  it("resolves missing preferences to all enabled", () => {
    expect(resolveModuleVisibility(undefined)).toEqual(DEFAULT_MODULE_VISIBILITY);
    expect(resolveModuleVisibility(null)).toEqual(DEFAULT_MODULE_VISIBILITY);
  });

  it("resolves a partially stated preference against the enabled default", () => {
    expect(resolveModuleVisibility({ weather: false })).toEqual({
      todo: true,
      weather: false,
      shortcuts: true,
      review: true,
    });
  });

  it("lets an explicit false disable exactly the selected module", () => {
    const visibility = resolveModuleVisibility({ todo: false, review: false });
    expect(visibility.todo).toBe(false);
    expect(visibility.review).toBe(false);
    expect(visibility.weather).toBe(true);
    expect(visibility.shortcuts).toBe(true);
  });

  it("keeps an explicit true enabled", () => {
    expect(resolveModuleVisibility({ todo: true })).toEqual(DEFAULT_MODULE_VISIBILITY);
  });

  it("ignores unknown preference keys and returns exactly the four known ids", () => {
    const stray = { news: false, todo: false } as Partial<ModuleVisibility>;
    const visibility = resolveModuleVisibility(stray);
    expect(Object.keys(visibility).sort()).toEqual(["review", "shortcuts", "todo", "weather"]);
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
  });
});
