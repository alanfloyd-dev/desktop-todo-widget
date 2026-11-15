import { describe, expect, it } from "vitest";
import { en, zhHans } from "../../i18n/catalog";
// Vite's `?raw` import for the Vue sources — the same pinning pattern as
// `updateCheck.test.ts` and `moduleSection.contract.test.ts`.
import settingsPanelVue from "../SettingsPanel.vue?raw";
import modulesPanelVue from "./ModulesPanel.vue?raw";
import appVue from "../../App.vue?raw";
// The stylesheet has to be read as a file: Vitest's CSS stubbing intercepts
// `.css?raw` and yields an empty string. `node:fs` resolves fine at runtime;
// the type suppressions are needed only because `tsconfig` restricts ambient
// types to `vite/client`, which does not declare Node builtins.
// @ts-expect-error see above
import { readFileSync } from "node:fs";
// @ts-expect-error see above
import { fileURLToPath } from "node:url";

const stylesCss = readFileSync(
  fileURLToPath(new URL("../../styles.css", import.meta.url)),
  "utf8",
);

/**
 * S1-C visual-consistency contracts, pinned at the source level in the same
 * spirit as the project's existing contract tests (module ids, section
 * structure): the Settings heading system must carry exactly the dashboard's
 * `.module-section-heading` tokens, the responsive boundary must keep its
 * DPR-proof form, secondary text must keep the unified 11px muted tokens with
 * nothing below the 10px floor, and the removed S1-B/S1-C catalog keys must
 * stay removed in both locales.
 */

/**
 * Body of the LAST rule whose selector list contains exactly `selector`.
 * Shared selectors are re-stated in comma groups (e.g. the `display: block`
 * helper) before their token rule, so the last match is the cascade winner
 * at equal specificity — exactly what the browser applies. The file has no
 * nested rules except media preludes, which never equal a queried selector.
 */
function ruleBody(css: string, selector: string): string {
  const ruleRe = /([^{}]+)\{([^}]*)\}/g;
  let found: string | null = null;
  let match: RegExpExecArray | null;
  while ((match = ruleRe.exec(css)) !== null) {
    const selectors = match[1]
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split(",")
      .map((part) => part.trim().replace(/\s+/g, " "));
    if (selectors.includes(selector)) found = match[2];
  }
  if (found === null) throw new Error(`rule not found: ${selector}`);
  return found;
}

/** Value of one declaration inside a rule body. */
function decl(block: string, property: string): string {
  const match = block.match(new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+)`, "m"));
  expect(match, `declaration not found: ${property}`).toBeTruthy();
  return match![1].trim();
}

/**
 * Bodies of EVERY rule whose selector list contains exactly `selector` — the
 * plain-text sheet carries more than one `.settings-panel` block (the palette
 * boundary near the top, the layout rule further down), and custom properties
 * cascade per declaration, so token assertions must pick the block that
 * actually declares them.
 */
function allRuleBodies(css: string, selector: string): string[] {
  const ruleRe = /([^{}]+)\{([^}]*)\}/g;
  const bodies: string[] = [];
  let match: RegExpExecArray | null;
  while ((match = ruleRe.exec(css)) !== null) {
    const selectors = match[1]
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split(",")
      .map((part) => part.trim().replace(/\s+/g, " "));
    if (selectors.includes(selector)) bodies.push(match[2]);
  }
  if (!bodies.length) throw new Error(`rule not found: ${selector}`);
  return bodies;
}

/** The `.settings-panel` block that declares the token boundary. */
function paletteBlock(css: string): string {
  const withTokens = allRuleBodies(css, ".settings-panel").filter((b) => b.includes("--ink"));
  expect(withTokens.length, "expected exactly one palette-bearing .settings-panel block").toBe(1);
  return withTokens[0];
}

const HEADING_TOKENS: Record<string, string> = {
  "font-size": "13px",
  color: "var(--mineral)",
  "font-weight": "500",
  "letter-spacing": "normal",
};

describe("settings heading system (S1-C 2.1)", () => {
  it("subsection headings share the dashboard `.module-section-title` tokens", () => {
    const settings = ruleBody(stylesCss, ".settings-section-heading strong");
    const dashboard = ruleBody(stylesCss, ".module-section-heading > .module-section-title");
    for (const [property, expected] of Object.entries(HEADING_TOKENS)) {
      expect(decl(settings, property), `settings heading ${property}`).toBe(expected);
      expect(decl(dashboard, property), `dashboard heading ${property}`).toBe(expected);
    }
  });
});

describe("settings contrast isolation (S1-D / review R-2)", () => {
  // The fixed graphite palette, byte-identical to the `:root` defaults: the
  // panel re-declares them on itself so inherited shell values (contrast-dark,
  // text-custom) can never reach the panel subtree.
  const FIXED_PANEL_PALETTE: Record<string, string> = {
    "--ink": "#d3dade",
    "--muted": "#849198",
    "--faint": "#59676f",
    "--mineral": "#81b9df",
    "--danger": "#d9a4a4",
    "--line": "rgba(145, 164, 174, 0.22)",
    "--line-strong": "rgba(145, 164, 174, 0.38)",
  };

  it("declares the full fixed foreground palette on the panel itself", () => {
    const panel = paletteBlock(stylesCss);
    for (const [token, expected] of Object.entries(FIXED_PANEL_PALETTE)) {
      expect(decl(panel, token), `panel token ${token}`).toBe(expected);
    }
  });

  it("keeps the palette free of contrast-shell scoping", () => {
    // The historical partial pin was scoped under `.text-custom`; the shell's
    // contrast modes must not gate the panel's palette in either direction.
    expect(stylesCss.includes(".text-custom .settings-panel")).toBe(false);
    expect(stylesCss.includes(".contrast-dark .settings-panel")).toBe(false);
  });

  it("matches the :root defaults exactly (no second palette)", () => {
    const root = ruleBody(stylesCss, ":root");
    const panel = paletteBlock(stylesCss);
    for (const token of Object.keys(FIXED_PANEL_PALETTE)) {
      expect(decl(panel, token)).toBe(decl(root, token));
    }
  });
});

describe("responsive boundary (S1-C 2.8 / review F-1)", () => {
  it("expresses the 420px boundary as a negated min-width, DPR-proof", () => {
    expect(stylesCss).toContain("@media not all and (min-width: 420px)");
    // The obsolete one-pixel-early form must not survive anywhere in the sheet.
    expect(stylesCss.includes("max-width: 419px")).toBe(false);
  });

  it("tab labels can never be compressed by the flex row", () => {
    const tabs = ruleBody(stylesCss, ".settings-panel .settings-nav--tabs button");
    expect(decl(tabs, "flex")).toBe("none");
    expect(decl(tabs, "white-space")).toBe("nowrap");
  });
});

describe("wide navigation boundary (S2-P1)", () => {
  // The rail's visible state is the base rule; the negated min-width form —
  // the same DPR-proof strategy as the 420px boundary — hides it below 560px,
  // and the exact complement hides the tabs at >=560px. The two predicates are
  // strict complements, so exactly one presentation is active at any width.
  it("expresses the 560px boundary as a negated min-width, DPR-proof", () => {
    expect(stylesCss).toContain("@media not all and (min-width: 560px)");
    // The obsolete one-pixel-early form must not survive anywhere in the sheet.
    expect(stylesCss.includes("max-width: 559px")).toBe(false);
    expect(stylesCss.includes("max-width: 560px")).toBe(false);
  });

  it("hides the rail below 560px and shows it from the base rule", () => {
    // Media-inner rules are invisible to the single-level-brace rule helpers,
    // so the hide is asserted textually inside the negated block; the base
    // (visible) rule is the only exact `.settings-nav--rail` rule body.
    const negatedStart = stylesCss.indexOf("@media not all and (min-width: 560px)");
    expect(negatedStart).toBeGreaterThan(-1);
    const negatedEnd = stylesCss.indexOf("@media (min-width: 560px)");
    const negated = stylesCss.slice(negatedStart, negatedEnd);
    expect(negated).toContain(".settings-nav--rail");
    expect(negated).toContain("display: none");
    const base = allRuleBodies(stylesCss, ".settings-nav--rail");
    expect(base.length, "only the base rail rule is an exact selector body").toBe(1);
    expect(decl(base[0], "display")).toBe("flex");
    expect(base[0]).toContain("flex-direction: column");
  });

  it("hides the horizontal tabs at >=560px via the exact complement", () => {
    const wideStart = stylesCss.indexOf("@media (min-width: 560px)");
    expect(wideStart).toBeGreaterThan(-1);
    const wide = stylesCss.slice(wideStart, stylesCss.indexOf("}", wideStart));
    expect(wide).toContain(".settings-nav--tabs");
    expect(wide).toContain("display: none");
  });

  it("keeps the 420px boundary block byte-identical", () => {
    const block420 = `@media not all and (min-width: 420px) {
  .settings-nav--tabs {
    display: none;
  }

  .settings-nav--select {
    display: flex;
    flex: none;
    padding: 8px 20px 10px;
    border-bottom: 1px solid var(--line);
  }
}`;
    const start = stylesCss.indexOf("@media not all and (min-width: 420px)");
    expect(start).toBeGreaterThan(-1);
    // Brace-counted extraction: the block ends at its own balancing close,
    // wherever later blocks were inserted after it.
    let depth = 0;
    let end = start;
    for (; end < stylesCss.length; end++) {
      if (stylesCss[end] === "{") depth++;
      else if (stylesCss[end] === "}") {
        depth--;
        if (depth === 0) {
          end++;
          break;
        }
      }
    }
    const actual = stylesCss.slice(start, end).replace(/\r\n/g, "\n").trim();
    expect(actual).toBe(block420);
  });
});

describe("navigation rail (S2-P1)", () => {
  it("renders a vertical WAI-ARIA tablist bound to the one shared category state", () => {
    expect(settingsPanelVue).toContain('class="settings-nav settings-nav--rail"');
    expect(settingsPanelVue).toContain('role="tablist"');
    expect(settingsPanelVue).toContain('aria-orientation="vertical"');
    // Exactly one activeCategory ref: all three presentations read and write it.
    expect(settingsPanelVue.match(/const activeCategory = ref<SettingsCategoryId>/g)?.length).toBe(1);
    // Tabs and rail switch through the same handler; the narrow select keeps
    // its own change-bridge to the same state.
    expect(settingsPanelVue.match(/@click="selectCategory\(category\.id\)"/g)?.length).toBe(2);
  });

  it("keeps roving tabindex, tabpanel associations, and rail controls across presentations", () => {
    // Tabs and rail carry the identical roving-tabindex binding.
    expect(settingsPanelVue.match(/:tabindex="activeCategory === category\.id \? 0 : -1"/g)?.length).toBe(2);
    // The tabpanels still label off the horizontal tabs' ids — the original
    // association is unchanged by the rail. Weather is no longer one of them
    // (S2-P2 merged it into Modules), so exactly four remain.
    expect(settingsPanelVue.match(/aria-labelledby="settings-tab-/g)?.length).toBe(4);
    // Rail items point at the same four tabpanels; the rail and tabpanel ids
    // are the same runtime bindings the tabs use.
    expect(settingsPanelVue).toContain(':aria-controls="`settings-panel-${category.id}`"');
    expect(settingsPanelVue).toContain(':id="`settings-rail-${category.id}`"');
    for (const id of ["appearance", "general", "modules", "about"]) {
      expect(settingsPanelVue).toContain(`id="settings-panel-${id}"`);
    }
    // The retired Weather tabpanel must not survive anywhere in the panel.
    expect(settingsPanelVue).not.toContain("settings-panel-weather");
    expect(settingsPanelVue).not.toContain("activeCategory === 'weather'");
  });

  it("drives the rail with the shared vertical reducer, focus following the active category", () => {
    expect(settingsPanelVue).toContain("onRailKeydown");
    expect(settingsPanelVue).toContain("nextVerticalTabCategory(activeCategory.value, event.key)");
    expect(settingsPanelVue).toContain("#settings-rail-${next}");
    // The horizontal model is untouched: same reducer name, same focus target.
    expect(settingsPanelVue).toContain("nextTabCategory(activeCategory.value, event.key)");
    expect(settingsPanelVue).toContain("#settings-tab-${next}");
  });

  it("adds only the navigation-shell wrapper; panel fragments keep their parents", () => {
    expect(settingsPanelVue.match(/class="settings-body"/g)?.length).toBe(1);
    expect(settingsPanelVue.match(/ref="settingsRail"/g)?.length).toBe(1);
    expect(settingsPanelVue.match(/ref="settingsContent"/g)?.length).toBe(1);
    // No new wrapper between the tabpanel sections and their panel components:
    // every child panel still mounts directly inside its own v-show section.
    // WeatherPanel is the S2-P2 exception by composition — it mounts inside
    // ModulesPanel (pinned in weatherPanel.contract.test.ts), not here.
    for (const panel of ["AppearancePanel", "GeneralPanel", "ModulesPanel", "AboutPanel"]) {
      expect(settingsPanelVue).toContain(`<${panel}`);
    }
    expect(settingsPanelVue).not.toContain("<WeatherPanel");
  });

  it("keeps the rail on the existing palette tokens", () => {
    // Two `.settings-nav--rail` rules exist (base + media hide); the base is
    // the one carrying the layout declarations.
    const rail = allRuleBodies(stylesCss, ".settings-nav--rail").find(
      (b) => decl(b, "display") === "flex",
    );
    expect(rail, "base rail rule").toBeTruthy();
    expect(decl(rail!, "width")).toBe("112px");
    expect(decl(rail!, "border-right")).toBe("1px solid var(--line)");
    const active = ruleBody(stylesCss, ".settings-panel .settings-nav--rail button.active");
    expect(decl(active, "border-left-color")).toBe("var(--mineral)");
    expect(decl(active, "color")).toBe("var(--ink)");
  });
});

describe("secondary text tokens (S1-C 2.2)", () => {
  it("row hints, module descriptions and the intro share 11px muted", () => {
    for (const selector of [".settings-row small", ".module-switch-text small", ".settings-intro"]) {
      const block = ruleBody(stylesCss, selector);
      expect(decl(block, "font-size"), selector).toBe("11px");
      expect(decl(block, "color"), selector).toBe("var(--muted)");
    }
  });

  // Review finding R-1: the narrow layout's category selector label is
  // auxiliary text like every other hint, so it carries the same tokens.
  it("the category selector label shares 11px muted (R-1)", () => {
    const block = ruleBody(stylesCss, ".settings-category-field > span");
    expect(decl(block, "font-size")).toBe("11px");
    expect(decl(block, "color")).toBe("var(--muted)");
  });

  it("keeps every Settings-scope text at or above the 10px floor", () => {
    for (const selector of [
      ".selected-location small",
      // `.asset-status` stands in for the mono value group (`.color-control
      // code` / `.range-control output` / `.asset-status`); the output
      // selector itself has a later layout-only rule without a font-size.
      ".asset-status",
      ".database-path",
      ".developer-diagnostics dl div",
      ".update-status-row small",
    ]) {
      const size = Number.parseInt(decl(ruleBody(stylesCss, selector), "font-size"), 10);
      expect(size, selector).toBeGreaterThanOrEqual(10);
    }
  });
});

describe("module rows (S1-C 2.3)", () => {
  it("standardize on approximately 56px with the switch semantics intact", () => {
    const row = ruleBody(stylesCss, ".module-switch-row");
    expect(Number.parseInt(decl(row, "min-height"), 10)).toBe(56);
    // The markup contract (D1) stays untouched: the template still renders
    // real switches for every registry module. The control is unique in the
    // template (one v-for'd button; the string also appears once in a comment).
    // S1-E3 moved the switch rows into ModulesPanel, which renders them from
    // the visibility draft it receives as a prop.
    expect(modulesPanelVue).toContain('role="switch"');
    expect(modulesPanelVue.match(/:aria-checked="moduleVisibility\[module\.id\]"/g)?.length).toBe(1);
  });
});

describe("catalog cleanup (S1-C 2.6 / 2.7)", () => {
  const removedKeys = [
    "settings.languageHeading",
    "settings.profileHeading",
    "settings.weatherHeading",
    "settings.githubHeading",
    "settings.modulesHeading",
    "settings.appearanceHeading",
    "settings.appearanceMaterial",
    "settings.title",
    // The Weather category's navigation label left with the category itself
    // (S2-P2); the Weather section's own keys (`settings.weatherLocation`…)
    // stay live inside WeatherPanel.
    "settings.category.weather",
  ];

  it("keeps the unused heading keys, the stale eyebrow and the retired Weather category label removed", () => {
    for (const key of removedKeys) {
      expect(en[key], `en still carries ${key}`).toBeUndefined();
      expect(zhHans[key], `zh-Hans still carries ${key}`).toBeUndefined();
    }
  });

  it("keeps the remaining headings sentence case in both locales", () => {
    expect(en["settings.update.heading"]).toBe("Updates");
    expect(en["settings.application.heading"]).toBe("Application");
    expect(zhHans["settings.update.heading"]).toBe("更新");
    expect(zhHans["settings.application.heading"]).toBe("应用");
  });

  it("renders no static appearance caption (review F-8)", () => {
    expect(settingsPanelVue).not.toContain("appearanceMaterial");
  });
});

describe("deep-link close contract (S1-C F-4)", () => {
  it("every Settings close path resets both deep-link flags", () => {
    const closeFn = appVue.match(/function closeSettings\(\) \{([^}]*)\}/);
    expect(closeFn, "closeSettings must exist").toBeTruthy();
    expect(closeFn![1]).toContain("weatherSettingsRequested.value = false");
    expect(closeFn![1]).toContain("githubSettingsRequested.value = false");
    expect(appVue).toContain('@close="closeSettings"');
    // Escape, the Review hand-off and the save-success path route through the
    // same helper.
    expect(appVue).toContain("closeSettings();");
  });
});
