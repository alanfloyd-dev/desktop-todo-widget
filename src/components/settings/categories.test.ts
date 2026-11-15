import { describe, expect, it } from "vitest";
import {
  SETTINGS_CATEGORIES,
  initialCategory,
  isSettingsCategoryId,
  nextTabCategory,
  nextVerticalTabCategory,
} from "./categories";
import { en, zhHans } from "../../i18n/catalog";

/**
 * The Settings navigation's structural contract (S1-B, amended by S2-P2 and
 * the S2-P2 order correction): exactly the four approved categories in their
 * fixed order — General first — every label resolved in both locales, one tab
 * keyboard model shared by the whole panel, and the legacy Weather deep link
 * redirected onto Modules (whose section it became).
 */
describe("settings categories", () => {
  it("defines exactly the four approved categories in order", () => {
    expect(SETTINGS_CATEGORIES.map((category) => category.id)).toEqual([
      "general",
      "appearance",
      "modules",
      "about",
    ]);
  });

  it("no longer carries Weather as a navigation category", () => {
    // S2-P2 merged the Weather category into Modules: its id left the closed
    // set, so every navigation surface (tabs, rail, select) drops it at once.
    // The string-level check keeps the retired id comparable after the union
    // narrowed.
    expect(SETTINGS_CATEGORIES.map((category) => category.id as string)).not.toContain("weather");
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
    // The retired Weather id keeps its safe fallback: refused like any other
    // unknown value, never silently resurrected as a category.
    expect(isSettingsCategoryId("weather")).toBe(false);
    expect(isSettingsCategoryId("github")).toBe(false);
    expect(isSettingsCategoryId("")).toBe(false);
    expect(isSettingsCategoryId(null)).toBe(false);
    expect(isSettingsCategoryId(42)).toBe(false);
  });
});

describe("initialCategory", () => {
  it("opens Modules for the legacy Weather deep link", () => {
    // The Weather category became a Modules section (S2-P2); the deep link
    // still works — it opens the category that now hosts the section, and the
    // focusWeather flag rides along to focus the search inside it.
    expect(initialCategory(true, false)).toBe("modules");
  });

  it("opens Modules for the GitHub deep link", () => {
    expect(initialCategory(false, true)).toBe("modules");
  });

  it("opens Appearance without a deep link", () => {
    expect(initialCategory(false, false)).toBe("appearance");
  });

  it("the weather deep link still resolves when both flags arrive", () => {
    // Both flags now map to the same category; the weather-wins priority
    // moved from "which tabpanel is visible" to "which focus runs last"
    // (pinned in modulesPanel.contract.test.ts).
    expect(initialCategory(true, true)).toBe("modules");
  });
});

describe("nextTabCategory", () => {
  it("moves right, wrapping from the last category to the first", () => {
    expect(nextTabCategory("general", "ArrowRight")).toBe("appearance");
    expect(nextTabCategory("about", "ArrowRight")).toBe("general");
  });

  it("moves left, wrapping from the first category to the last", () => {
    expect(nextTabCategory("general", "ArrowLeft")).toBe("about");
    expect(nextTabCategory("appearance", "ArrowLeft")).toBe("general");
  });

  it("Home and End jump to the ends", () => {
    expect(nextTabCategory("about", "Home")).toBe("general");
    expect(nextTabCategory("appearance", "End")).toBe("about");
  });

  it("ignores every other key", () => {
    expect(nextTabCategory("appearance", " ")).toBeNull();
    expect(nextTabCategory("appearance", "Enter")).toBeNull();
    expect(nextTabCategory("appearance", "ArrowDown")).toBeNull();
  });

  it("ignores keys when the current id is not a category", () => {
    expect(nextTabCategory("github" as never, "ArrowRight")).toBeNull();
    // The retired Weather id is not a category either — every key refused.
    expect(nextTabCategory("weather" as never, "ArrowRight")).toBeNull();
  });
});

describe("nextVerticalTabCategory (S2-P1 rail)", () => {
  it("moves down in the same fixed category order, wrapping from the last to the first", () => {
    expect(nextVerticalTabCategory("general", "ArrowDown")).toBe("appearance");
    expect(nextVerticalTabCategory("appearance", "ArrowDown")).toBe("modules");
    expect(nextVerticalTabCategory("modules", "ArrowDown")).toBe("about");
    expect(nextVerticalTabCategory("about", "ArrowDown")).toBe("general");
  });

  it("moves up with the same wraparound", () => {
    expect(nextVerticalTabCategory("about", "ArrowUp")).toBe("modules");
    expect(nextVerticalTabCategory("general", "ArrowUp")).toBe("about");
  });

  it("Home and End jump to the same ends as the horizontal model", () => {
    expect(nextVerticalTabCategory("about", "Home")).toBe("general");
    expect(nextVerticalTabCategory("appearance", "End")).toBe("about");
  });

  it("orthogonal horizontal arrows do nothing", () => {
    expect(nextVerticalTabCategory("appearance", "ArrowLeft")).toBeNull();
    expect(nextVerticalTabCategory("appearance", "ArrowRight")).toBeNull();
  });

  it("ignores every other key", () => {
    expect(nextVerticalTabCategory("appearance", " ")).toBeNull();
    expect(nextVerticalTabCategory("appearance", "Enter")).toBeNull();
    expect(nextVerticalTabCategory("appearance", "Tab")).toBeNull();
  });

  it("ignores keys when the current id is not a category", () => {
    expect(nextVerticalTabCategory("github" as never, "ArrowDown")).toBeNull();
    expect(nextVerticalTabCategory("github" as never, "Home")).toBeNull();
  });
});
