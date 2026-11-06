import { describe, expect, it } from "vitest";
import {
  SETTINGS_CATEGORIES,
  initialCategory,
  isSettingsCategoryId,
  nextTabCategory,
} from "./categories";
import { en, zhHans } from "../../i18n/catalog";

/**
 * The Settings navigation's structural contract (S1-B): exactly the five
 * approved categories in their fixed order, every label resolved in both
 * locales, and one tab keyboard model shared by the whole panel.
 */
describe("settings categories", () => {
  it("defines exactly the five approved categories in order", () => {
    expect(SETTINGS_CATEGORIES.map((category) => category.id)).toEqual([
      "appearance",
      "general",
      "modules",
      "weather",
      "about",
    ]);
  });

  it("labels every category in both locales, never as a raw key", () => {
    for (const category of SETTINGS_CATEGORIES) {
      expect(en[category.labelKey], `en missing ${category.labelKey}`).toBeTruthy();
      expect(zhHans[category.labelKey], `zh-Hans missing ${category.labelKey}`).toBeTruthy();
      expect(en[category.labelKey]).not.toBe(category.labelKey);
      expect(zhHans[category.labelKey]).not.toBe(category.labelKey);
    }
  });

  it("narrows valid ids and rejects everything else", () => {
    for (const category of SETTINGS_CATEGORIES) {
      expect(isSettingsCategoryId(category.id)).toBe(true);
    }
    expect(isSettingsCategoryId("github")).toBe(false);
    expect(isSettingsCategoryId("")).toBe(false);
    expect(isSettingsCategoryId(null)).toBe(false);
    expect(isSettingsCategoryId(42)).toBe(false);
  });
});

describe("initialCategory", () => {
  it("opens Weather for the weather deep link", () => {
    expect(initialCategory(true, false)).toBe("weather");
  });

  it("opens Modules for the GitHub deep link", () => {
    expect(initialCategory(false, true)).toBe("modules");
  });

  it("opens Appearance without a deep link", () => {
    expect(initialCategory(false, false)).toBe("appearance");
  });

  it("the weather deep link wins when both flags arrive", () => {
    expect(initialCategory(true, true)).toBe("weather");
  });
});

describe("nextTabCategory", () => {
  it("moves right, wrapping from the last category to the first", () => {
    expect(nextTabCategory("appearance", "ArrowRight")).toBe("general");
    expect(nextTabCategory("about", "ArrowRight")).toBe("appearance");
  });

  it("moves left, wrapping from the first category to the last", () => {
    expect(nextTabCategory("about", "ArrowLeft")).toBe("weather");
    expect(nextTabCategory("appearance", "ArrowLeft")).toBe("about");
  });

  it("Home and End jump to the ends", () => {
    expect(nextTabCategory("weather", "Home")).toBe("appearance");
    expect(nextTabCategory("general", "End")).toBe("about");
  });

  it("ignores every other key", () => {
    expect(nextTabCategory("appearance", " ")).toBeNull();
    expect(nextTabCategory("appearance", "Enter")).toBeNull();
    expect(nextTabCategory("appearance", "ArrowDown")).toBeNull();
  });

  it("ignores keys when the current id is not a category", () => {
    expect(nextTabCategory("github" as never, "ArrowRight")).toBeNull();
  });
});
