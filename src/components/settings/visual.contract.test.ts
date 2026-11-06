import { describe, expect, it } from "vitest";
import { en, zhHans } from "../../i18n/catalog";
// Vite's `?raw` import for the Vue sources — the same pinning pattern as
// `updateCheck.test.ts` and `moduleSection.contract.test.ts`.
import settingsPanelVue from "../SettingsPanel.vue?raw";
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
    expect(settingsPanelVue).toContain('role="switch"');
    expect(settingsPanelVue.match(/:aria-checked="moduleVisibilityDraft\[module\.id\]"/g)?.length).toBe(1);
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
  ];

  it("keeps the seven unused heading keys plus the stale eyebrow removed", () => {
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
