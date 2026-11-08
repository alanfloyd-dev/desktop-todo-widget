import { describe, expect, it } from "vitest";
// Source-pinning contracts for the S1-E3 ModulesPanel extraction — the same
// `?raw` pattern as `weatherPanel.contract.test.ts` (the Vitest node
// environment has no component mounting, so the boundary is pinned at the
// source level).
import settingsPanelVue from "../SettingsPanel.vue?raw";
import modulesPanelVue from "./ModulesPanel.vue?raw";
import appVue from "../../App.vue?raw";

/**
 * S1-E3 moved the Modules category's controls into `ModulesPanel` while the
 * persisted fields stayed SettingsPanel drafts. These pins hold the boundary
 * in place: the child owns the transient quick-link editor, the GitHub
 * presentation computeds and the deep-link focus, the parent keeps the dirty
 * snapshot, the Save payload, the draft mutations (and the new link's id
 * assignment) and the `v-show` tabpanel, and the moved markup stays the
 * tabpanel's direct children (a multi-root fragment, no wrapper element).
 */

/** Every catalog key the Modules category renders (in the child, after E3). */
const MODULES_I18N_KEYS = [
  "settings.modulesDescription",
  "settings.modulesReset",
  "settings.githubUsername",
  "settings.githubUsernameHint",
  "settings.githubUsernameHintDisabled",
  "settings.githubUsernamePlaceholder",
  "github.error.invalidUsername",
  "quickLinks.heading",
  "quickLinks.empty",
  "quickLinks.moveUp",
  "quickLinks.moveUpAriaLabel",
  "quickLinks.moveDown",
  "quickLinks.moveDownAriaLabel",
  "quickLinks.editAriaLabel",
  "quickLinks.edit",
  "quickLinks.deleteAriaLabel",
  "quickLinks.delete",
  "quickLinks.name",
  "quickLinks.namePlaceholder",
  "quickLinks.url",
  "quickLinks.urlPlaceholder",
  "quickLinks.error.nameRequired",
  "quickLinks.save",
  "quickLinks.cancel",
  "quickLinks.add",
];

describe("ModulesPanel extraction: parent draft ownership (S1-E3)", () => {
  it("keeps the GitHub username in the dirty snapshot and the Save payload", () => {
    // The exact draft-to-payload line appears exactly twice: once in the
    // dirtyDraft projection and once in the save emit — the "dirty" and
    // "would write" sets stay identical by construction.
    expect(
      settingsPanelVue.match(/githubUsername: githubUsername\.value,/g)?.length,
    ).toBe(2);
  });

  it("keeps module visibility and the link list in both projections, in their own shapes", () => {
    // The dirty snapshot reads the raw draft; the Save payload clones both —
    // a save can never write through the draft's objects to the parent.
    expect(
      settingsPanelVue.match(/moduleVisibility: moduleVisibilityDraft\.value,/g)?.length,
    ).toBe(1);
    expect(settingsPanelVue).toContain("moduleVisibility: { ...moduleVisibilityDraft.value },");
    expect(
      settingsPanelVue.match(/quickLinks: quickLinks\.value,/g)?.length,
    ).toBe(1);
    expect(
      settingsPanelVue.match(/quickLinks: quickLinks\.value\.map\(\(link\) => \(\{ \.\.\.link \}\)\),/g)?.length,
    ).toBe(1);
  });

  it("writes the visibility draft when the child reports a toggle or a reset", () => {
    expect(settingsPanelVue).toMatch(
      /function toggleModule\(id: ModuleId\) \{\s*moduleVisibilityDraft\.value = \{\s*\.\.\.moduleVisibilityDraft\.value,\s*\[id\]: !moduleVisibilityDraft\.value\[id\],\s*\};\s*\}/,
    );
    expect(settingsPanelVue).toMatch(
      /function resetModules\(\) \{\s*moduleVisibilityDraft\.value = \{ \.\.\.DEFAULT_MODULE_VISIBILITY \};\s*\}/,
    );
  });

  it("writes the username draft when the child reports an edit", () => {
    expect(settingsPanelVue).toMatch(
      /function updateGithubUsername\(value: string\) \{\s*githubUsername\.value = value;\s*\}/,
    );
  });

  it("mutates the link list only through the four reported actions, assigning new ids here", () => {
    // Id assignment stays with the draft owner: the list has to stay
    // addressable across a reorder before anything is persisted.
    expect(settingsPanelVue).toMatch(
      /function addQuickLink\(draft: \{ name: string; url: string \}\) \{\s*quickLinks\.value\.push\(\{ id: newLinkId\(\), name: draft\.name, url: draft\.url \}\);\s*\}/,
    );
    expect(settingsPanelVue).toMatch(
      /function updateQuickLink\(link: \{ id: string; name: string; url: string \}\) \{\s*const existing = quickLinks\.value\.find\(\(item\) => item\.id === link\.id\);\s*if \(existing\) \{\s*existing\.name = link\.name;\s*existing\.url = link\.url;\s*\}\s*\}/,
    );
    expect(settingsPanelVue).toMatch(
      /function deleteQuickLink\(id: string\) \{\s*quickLinks\.value = quickLinks\.value\.filter\(\(link\) => link\.id !== id\);\s*\}/,
    );
    expect(settingsPanelVue).toMatch(
      /function moveQuickLink\(move: \{ id: string; delta: -1 \| 1 \}\) \{\s*moveLink\(quickLinks\.value, move\.id, move\.delta\);\s*\}/,
    );
  });

  it("the child cannot reach the settings save path or assign link ids", () => {
    for (const forbidden of [
      "settingsSnapshot",
      "settingsDraftIsDirty",
      "dirtyDraft",
      "update_product_settings",
      "newLinkId",
    ]) {
      expect(modulesPanelVue, forbidden).not.toContain(forbidden);
    }
  });
});

describe("ModulesPanel extraction: child interface (S1-E3)", () => {
  it("receives the required display state as explicit typed props", () => {
    for (const prop of [
      "moduleVisibility: ModuleVisibility;",
      "githubUsername: string;",
      "quickLinks: QuickLink[];",
      "focusGithub: boolean;",
    ]) {
      expect(modulesPanelVue, prop).toContain(prop);
    }
    expect(modulesPanelVue).toContain("defineProps<{");
  });

  it("reports persisted changes back through explicit typed events", () => {
    for (const event of [
      "toggleModule: [id: ModuleId];",
      "resetModules: [];",
      "updateGithubUsername: [value: string];",
      "addLink: [draft: { name: string; url: string }];",
      "updateLink: [link: { id: string; name: string; url: string }];",
      "deleteLink: [id: string];",
      "moveLink: [move: { id: string; delta: -1 | 1 }];",
    ]) {
      expect(modulesPanelVue, event).toContain(event);
    }
  });

  it("the parent passes every binding exactly once on the child element", () => {
    const usages = settingsPanelVue.match(/<ModulesPanel\b[^>]*>/g);
    expect(usages?.length).toBe(1);
    const tag = usages![0];
    // No conditional mounting may ride on the element itself.
    expect(tag).not.toContain("v-if");
    expect(tag).not.toContain("v-show");
    for (const binding of [
      ":module-visibility",
      ":github-username",
      ":quick-links",
      ":focus-github",
      "@toggle-module",
      "@reset-modules",
      "@update-github-username",
      "@add-link",
      "@update-link",
      "@delete-link",
      "@move-link",
    ]) {
      expect(tag, binding).toContain(binding);
    }
  });

  it("renders as a multi-root fragment: no wrapper between the tabpanel and the moved markup", () => {
    // The template opens straight onto the intro paragraph and closes right
    // after the quick-links section — the moved elements stay the
    // `settings-category-panel`'s direct children, so the rendered DOM (and
    // every descendant selector in styles.css) is identical to the inline
    // layout this extraction replaced.
    expect(modulesPanelVue).toMatch(
      /<template>\s*<p class="settings-intro">/,
    );
    expect(modulesPanelVue).toMatch(
      /<\/section>\s*<\/template>\s*$/,
    );
  });

  it("keeps every Modules translation key in the child template", () => {
    // Template attribute values quote with single quotes; script strings with
    // double quotes — pin the key itself, not one quote style.
    for (const key of MODULES_I18N_KEYS) {
      expect(modulesPanelVue, key).toMatch(new RegExp(`['"]${key}['"]`));
    }
  });
});

describe("ModulesPanel extraction: moved-as-a-unit responsibilities (S1-E3)", () => {
  it("the quick-link editor lives in ModulesPanel, not SettingsPanel", () => {
    // A second owner would mean two editor state machines — the WeatherPanel
    // extraction's rule, applied here.
    for (const owned of [
      "editingLinkId",
      "draftName",
      "draftUrl",
      "quickLinkError",
      "resetEditor",
      "commitEditor",
      "quickLinkUrlErrorKey",
    ]) {
      expect(modulesPanelVue, owned).toContain(owned);
      expect(settingsPanelVue, owned).not.toContain(owned);
    }
  });

  it("the GitHub presentation and focus targets live in ModulesPanel, not SettingsPanel", () => {
    for (const owned of [
      "githubUsernameError",
      "githubUsernameHint",
      "githubRow",
      "githubInput",
    ]) {
      expect(modulesPanelVue, owned).toContain(owned);
      expect(settingsPanelVue, owned).not.toContain(owned);
    }
  });

  it("the switch markup contract (D1) survives the move verbatim", () => {
    expect(modulesPanelVue).toContain('role="switch"');
    expect(modulesPanelVue.match(/:aria-checked="moduleVisibility\[module\.id\]"/g)?.length).toBe(1);
    expect(modulesPanelVue).toContain(
      ':aria-labelledby="`module-switch-label-${module.id}`"',
    );
    expect(modulesPanelVue).toContain(
      ':aria-describedby="`module-switch-desc-${module.id}`"',
    );
  });
});

describe("ModulesPanel extraction: lifecycle and deep link (S1-E3)", () => {
  it("the parent mounts ModulesPanel unconditionally inside its v-show tabpanel", () => {
    // The wrapper stays the parent's and the child carries no `v-if`: editor
    // state must survive switching categories, exactly like the weather
    // search state does.
    expect(settingsPanelVue).toMatch(
      /<section\s+v-show="activeCategory === 'modules'"\s+id="settings-panel-modules"\s+class="settings-category-panel"\s+role="tabpanel"\s+aria-labelledby="settings-tab-modules"\s*>(?:\s*<!--[\s\S]*?-->\s*)*<ModulesPanel\b/,
    );
  });

  it("the GitHub deep link still opens the category and focuses the username field", () => {
    // initialCategory keeps choosing Modules before first render (so the
    // tabpanel is visible when the child mounts), and the child owns the
    // scrollIntoView + focus pair on its own refs — the same split the
    // weather deep link got in S1-E2. After the move the parent references
    // the flag exactly once: the initialCategory call.
    expect(settingsPanelVue).toContain("initialCategory(props.focusWeather, props.focusGithub)");
    expect(settingsPanelVue.match(/props\.focusGithub/g)?.length).toBe(1);
    expect(modulesPanelVue).toMatch(
      /if \(props\.focusGithub\) \{\s*await nextTick\(\);\s*githubRow\.value\?\.scrollIntoView\(\{ block: "start" \}\);\s*githubInput\.value\?\.focus\(\);\s*\}/,
    );
    expect(appVue).toContain(':focus-github="githubSettingsRequested"');
  });
});
