import { describe, expect, it } from "vitest";
// Source-pinning contracts for the S1-E5 AppearancePanel extraction — the same
// `?raw` pattern as `generalPanel.contract.test.ts` and its sibling panels
// (the Vitest node environment has no component mounting, so the boundary is
// pinned at the source level; executable logic runs through the behavior
// harnesses at the bottom of this file).
import settingsPanelVue from "../SettingsPanel.vue?raw";
import appearancePanelVue from "./AppearancePanel.vue?raw";
import { en, zhHans } from "../../i18n/catalog";
// The real default profile for the reset/isolation harnesses below (pure, and
// the same module the parent imports).
import { DEFAULT_APPEARANCE } from "../../appearance";
import type { AppearanceSettings, ProductWindowMode } from "../../types";

/**
 * S1-E5 moved the Appearance category's controls into `AppearancePanel` while
 * every persisted value stayed SettingsPanel state. These pins hold the
 * boundary in place: the child owns the presentation (the profile selector,
 * the five background materials and their controls, the text contrast modes),
 * the parent keeps the three-profile draft, the editor state (`appearanceMode`),
 * the dirty snapshot, the Save payload (legacy `appearance` field included),
 * the background picker's native commands, the provisional-asset cleanup and
 * the `v-show` tabpanel. The background row's error is a parent-rendered typed
 * slot, because `assetError` is the parent's state (one ref shared with the
 * avatar picker) and the Rust source pin in `appearance.rs` requires the
 * rendering expression to stay in SettingsPanel.vue — the same arrangement as
 * GeneralPanel's avatarError. The moved markup stays the tabpanel's direct
 * children (a multi-root fragment, no wrapper element).
 */

/** Every static catalog key the Appearance category renders literally (in the child, after E5). */
const APPEARANCE_I18N_KEYS = [
  "settings.appearanceFor",
  "settings.appearanceForHint",
  "settings.appearanceForAriaLabel",
  "settings.background",
  "settings.backgroundHint",
  "settings.backgroundAriaLabel",
  "settings.tint",
  "settings.tintHint",
  "settings.tintOpacity",
  "settings.tintOpacityHint",
  "settings.blur",
  "settings.blurHint",
  "settings.color",
  "settings.colorHint",
  "settings.gradientStart",
  "settings.gradientStartHint",
  "settings.gradientEnd",
  "settings.gradientEndHint",
  "settings.angle",
  "settings.angleHint",
  "settings.customImage",
  "settings.customImageHint",
  "settings.chooseImage",
  "settings.imageUnavailable",
  "settings.fit",
  "settings.fitHint",
  "settings.position",
  "settings.positionHint",
  "settings.desktopWallpaper",
  "settings.desktopWallpaperHint",
  "settings.wallpaperAvailable",
  "settings.wallpaperUnavailable",
  "settings.backgroundOpacity",
  "settings.backgroundOpacityHint",
  "settings.overlay",
  "settings.overlayHint",
  "settings.textContrast",
  "settings.textContrastHint",
  "settings.textContrastAriaLabel",
  "settings.customTextColor",
  "settings.customTextColorHint",
  "settings.resetAppearance",
];

/** Extracts a block from the SFC source: `startMarker` .. first `endMarker`. */
function extractBlock(source: string, startMarker: string, endMarker: string): string {
  const start = source.indexOf(startMarker);
  if (start === -1) return "";
  const end = source.indexOf(endMarker, start);
  if (end === -1) return "";
  return source.slice(start, end + endMarker.length);
}

describe("AppearancePanel extraction: parent draft ownership (S1-E5)", () => {
  it("keeps every persisted Appearance field in the dirty snapshot and the Save payload", () => {
    // The draft-to-payload lines appear exactly twice — once in the dirtyDraft
    // projection and once in the save emit — so "dirty" and "would write"
    // stay the same set by construction. The profiles go through
    // `cloneProfiles` on save (detached copies), the legacy `appearance`
    // string field stays in both.
    expect(settingsPanelVue.match(/appearanceProfiles: appearanceProfiles\.value,/g)?.length).toBe(1);
    expect(settingsPanelVue.match(/appearanceProfiles: cloneProfiles\(appearanceProfiles\.value\),/g)?.length).toBe(1);
    expect(settingsPanelVue.match(/appearance: appearance\.value,/g)?.length).toBe(2);
  });

  it("keeps the profile editor state and the current-profile computed in the parent", () => {
    // `appearanceMode` opens on the mode the product actually runs in, and
    // `appearanceSettings` is the parent's projection onto the edited
    // profile — the child receives both as read-only props.
    expect(settingsPanelVue).toContain(
      "const appearanceMode = ref<ProductWindowMode>(props.settings.mode);",
    );
    expect(settingsPanelVue).toContain(
      "computed<AppearanceSettings>(\n  () => appearanceProfiles.value[appearanceMode.value],\n)",
    );
    for (const owned of ["appearanceMode", "appearanceSettings", "cloneProfiles"]) {
      expect(appearancePanelVue, owned).not.toContain(owned);
    }
  });

  it("keeps the image-picker state, the asset lifecycle and reset in the parent", () => {
    // The Rust pins in appearance.rs read the picker commands and the error
    // rendering from SettingsPanel.vue, and the provisional-asset lifecycle
    // is one unit with them: the child may not reach any of it.
    for (const owned of [
      "choose_local_asset",
      "store_managed_asset",
      "discard_managed_asset",
      "normalizeAvatarImage(",
      "sampleImageLuminance(",
      "provisionalAssets",
      "pickedImage",
      "imagePreviewAvailable",
      "pickedImage.value?.mode === appearanceMode.value",
      "function chooseAsset(",
      "function resetAppearance()",
      "DEFAULT_APPEARANCE",
      "invoke(",
    ]) {
      expect(settingsPanelVue, owned).toContain(owned);
      expect(appearancePanelVue, owned).not.toContain(owned);
    }
    // The unavailable-image hint is computed by the parent (from its picker
    // state) and reaches the child as a read-only prop.
    expect(settingsPanelVue).toContain("const editedImageUnavailable = computed(");
    expect(appearancePanelVue).toContain("editedImageUnavailable: boolean;");
  });

  it("the child cannot reach the settings save path", () => {
    for (const forbidden of ["settingsSnapshot", "settingsDraftIsDirty", "dirtyDraft", "update_product_settings"]) {
      expect(appearancePanelVue, forbidden).not.toContain(forbidden);
    }
    expect(appearancePanelVue).toContain("defineEmits<{");
  });

  it("reset clears the image-preview state with the profile", () => {
    // The reset is the parent's: it replaces the edited profile with the
    // defaults and drops the picked-image preview state (see the behavior
    // harness below for the executed version).
    expect(settingsPanelVue).toMatch(
      /function resetAppearance\(\) \{\s*appearanceProfiles\.value\[appearanceMode\.value\] = \{ \.\.\.DEFAULT_APPEARANCE \};\s*pickedImage\.value = null;\s*imagePreviewAvailable\.value = false;\s*\}/,
    );
  });

  it("writes the edited profile when the child reports an edit", () => {
    // One handler per child event, each writing exactly the field the event
    // names (the executed versions drive the isolation harness below).
    for (const [field, type] of [
      ["backgroundType", "BackgroundType"],
      ["glassTintColor", "string"],
      ["glassTintOpacity", "number"],
      ["blurPx", "number"],
      ["solidColor", "string"],
      ["gradientStartColor", "string"],
      ["gradientEndColor", "string"],
      ["gradientAngle", "number"],
      ["imageFit", "ImageFit"],
      ["imagePosition", "ImagePosition"],
      ["backgroundOpacity", "number"],
      ["overlayStrength", "number"],
      ["textContrast", "TextContrast"],
      ["customTextColor", "string"],
    ] as const) {
      expect(settingsPanelVue, field).toMatch(
        new RegExp(
          `function update${field.charAt(0).toUpperCase()}${field.slice(1)}\\(value: ${type}\\) \\{\\s*appearanceSettings\\.value\\.${field} = value;\\s*\\}`,
        ),
      );
    }
    expect(settingsPanelVue).toMatch(
      /function selectAppearanceProfile\(mode: ProductWindowMode\) \{\s*appearanceMode\.value = mode;\s*\}/,
    );
  });
});

describe("AppearancePanel extraction: child interface (S1-E5)", () => {
  it("receives the required display state as explicit typed props", () => {
    for (const prop of [
      "mode: ProductWindowMode;",
      "settings: AppearanceSettings;",
      "wallpaperAvailable: boolean;",
      "editedImageUnavailable: boolean;",
    ]) {
      expect(appearancePanelVue, prop).toContain(prop);
    }
    expect(appearancePanelVue).toContain("defineProps<{");
  });

  it("reports persisted changes back through explicit typed events", () => {
    for (const event of [
      "selectProfile: [mode: ProductWindowMode];",
      "updateBackgroundType: [value: BackgroundType];",
      "updateGlassTintColor: [value: string];",
      "updateGlassTintOpacity: [value: number];",
      "updateBlurPx: [value: number];",
      "updateSolidColor: [value: string];",
      "updateGradientStartColor: [value: string];",
      "updateGradientEndColor: [value: string];",
      "updateGradientAngle: [value: number];",
      "updateImageFit: [value: ImageFit];",
      "updateImagePosition: [value: ImagePosition];",
      "updateBackgroundOpacity: [value: number];",
      "updateOverlayStrength: [value: number];",
      "updateTextContrast: [value: TextContrast];",
      "updateCustomTextColor: [value: string];",
      "chooseBackground: [];",
      "resetAppearance: [];",
    ]) {
      expect(appearancePanelVue, event).toContain(event);
    }
  });

  it("the v-model bridges hold no state: reads flow from the prop, writes emit up", () => {
    // Every bridge reads the parent's `settings` prop and emits; none of the
    // child's computeds writes anywhere local, so the only copy of each value
    // stays the parent's draft.
    expect(appearancePanelVue.match(/get: \(\) => props\.settings\./g)?.length).toBe(12);
    expect(appearancePanelVue.match(/set: \(value: (?:string|number)\) => emit\(/g)?.length).toBe(10);
    // The two select bridges narrow the option strings before emitting.
    expect(appearancePanelVue).toMatch(
      /const imageFit = computed\(\{\s*get: \(\) => props\.settings\.imageFit,\s*set: \(value: string\) => \{\s*if \(value === "cover" \|\| value === "contain" \|\| value === "stretch"\) \{\s*emit\("updateImageFit", value\);\s*\}\s*\},\s*\}\);/,
    );
    expect(appearancePanelVue).toMatch(
      /const imagePosition = computed\(\{\s*get: \(\) => props\.settings\.imagePosition,\s*set: \(value: string\) => \{\s*if \(value === "center" \|\| value === "top" \|\| value === "bottom"\) \{\s*emit\("updateImagePosition", value\);\s*\}\s*\},\s*\}\);/,
    );
  });

  it("declares the background error as a typed slot, and the parent renders it", () => {
    // The parent owns `assetError` (one ref shared with the avatar picker)
    // and the appearance.rs source pin names SettingsPanel.vue as the place
    // the background expression lives — so the element stays there and the
    // child only marks where it renders.
    expect(appearancePanelVue).toContain("defineSlots<{ backgroundError?: () => unknown }>");
    expect(appearancePanelVue).toContain('<slot name="backgroundError"></slot>');
    expect(appearancePanelVue).not.toContain("assetError");
    expect(settingsPanelVue).toContain("#backgroundError");
    expect(settingsPanelVue).toContain("assetError?.kind === 'background'");
    // Exactly one background-error rendering (the slot), next to the avatar
    // one — the pair the Rust pin expects.
    expect(settingsPanelVue.match(/assetError\?\.kind === 'background'/g)?.length).toBe(1);
  });

  it("the parent passes every binding exactly once on the child element (N-1)", () => {
    const usages = settingsPanelVue.match(/<AppearancePanel\b[^>]*>/g);
    expect(usages?.length).toBe(1);
    const tag = usages![0];
    // No conditional mounting may ride on the element itself.
    expect(tag).not.toContain("v-if");
    expect(tag).not.toContain("v-show");
    for (const binding of [
      ":mode",
      ":settings",
      ":wallpaper-available",
      ":edited-image-unavailable",
      "@select-profile",
      "@update-background-type",
      "@update-glass-tint-color",
      "@update-glass-tint-opacity",
      "@update-blur-px",
      "@update-solid-color",
      "@update-gradient-start-color",
      "@update-gradient-end-color",
      "@update-gradient-angle",
      "@update-image-fit",
      "@update-image-position",
      "@update-background-opacity",
      "@update-overlay-strength",
      "@update-text-contrast",
      "@update-custom-text-color",
      "@choose-background",
      "@reset-appearance",
    ]) {
      // Counted inside the component element's tag only — occurrences
      // elsewhere in the parent file are irrelevant to this contract.
      const occurrences = tag.split(binding).length - 1;
      expect(occurrences, `binding ${binding} must appear exactly once on the <AppearancePanel> element`).toBe(1);
    }
  });

  it("renders as a multi-root fragment: no wrapper between the tabpanel and the moved markup", () => {
    // The template opens straight onto the profile-selector row (after its
    // comment) and closes right after the reset button — the moved elements
    // stay the `settings-category-panel`'s direct children, so the rendered
    // DOM (and every descendant selector in styles.css) is identical to the
    // inline layout this extraction replaced.
    expect(appearancePanelVue).toMatch(
      /<template>\s*<!--\s*\n\s*Which profile the controls below edit\./,
    );
    expect(appearancePanelVue).toMatch(
      /<\/button>\s*<\/template>\s*$/,
    );
  });

  it("keeps every Appearance translation key in the child template, resolvable in both locales", () => {
    // Template attribute values quote with single quotes; script strings with
    // double quotes — pin the key itself, not one quote style.
    for (const key of APPEARANCE_I18N_KEYS) {
      expect(appearancePanelVue, key).toMatch(new RegExp(`['"]${key}['"]`));
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
    // The dynamic groups resolve per option in both locales.
    expect(appearancePanelVue).toContain("menu.mode.");
    for (const option of ["sidebar", "floating", "desktop"]) {
      const key = `menu.mode.${option}`;
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
    for (const option of ["glass", "solid", "gradient", "image", "wallpaper"]) {
      const key = `settings.backgroundType.${option}`;
      expect(appearancePanelVue, key).toContain("settings.backgroundType.");
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
    for (const option of ["auto", "light", "dark", "custom"]) {
      const key = `settings.textContrast.${option}`;
      expect(appearancePanelVue, key).toContain("settings.textContrast.");
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
    for (const option of ["cover", "contain", "stretch"]) {
      const key = `settings.fit.${option}`;
      expect(appearancePanelVue, key).toContain(`value="${option}"`);
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
    for (const option of ["center", "top", "bottom"]) {
      const key = `settings.position.${option}`;
      expect(appearancePanelVue, key).toContain(`value="${option}"`);
      expect((en as Record<string, unknown>)[key], `en missing ${key}`).toBeTruthy();
      expect((zhHans as Record<string, unknown>)[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
  });

  it("keeps the ARIA group semantics on the three button groups", () => {
    // The profile selector, the material selector and the text contrast group
    // keep their role/label pairs; the profile buttons stay pressed-state
    // controls (the material buttons never had aria-pressed — unchanged).
    expect(appearancePanelVue.match(/role="group"/g)?.length).toBe(3);
    expect(appearancePanelVue).toContain(`:aria-label="t('settings.appearanceForAriaLabel')"`);
    expect(appearancePanelVue).toContain(`:aria-label="t('settings.backgroundAriaLabel')"`);
    expect(appearancePanelVue).toContain(`:aria-label="t('settings.textContrastAriaLabel')"`);
    expect(appearancePanelVue).toContain(':aria-pressed="mode === option"');
  });
});

describe("AppearancePanel extraction: controls and conditional rendering (S1-E5)", () => {
  it("renders all five background materials from one option list with per-material conditions", () => {
    expect(appearancePanelVue).toContain(
      "v-for=\"option in ['glass', 'solid', 'gradient', 'image', 'wallpaper'] as const\"",
    );
    expect(appearancePanelVue).toMatch(/<template v-if="settings\.backgroundType === 'glass'">/);
    expect(appearancePanelVue).toMatch(/v-if="settings\.backgroundType === 'solid'"/);
    expect(appearancePanelVue).toMatch(/<template v-if="settings\.backgroundType === 'gradient'">/);
    expect(appearancePanelVue).toMatch(/<template v-if="settings\.backgroundType === 'image'">/);
    expect(appearancePanelVue).toMatch(/v-if="settings\.backgroundType === 'wallpaper'"/);
    expect(appearancePanelVue.match(/settings\.backgroundType === '/g)?.length).toBe(5);
  });

  it("preserves the numeric controls and their exact input ranges", () => {
    // Five ranges with the shipped min/max/step, all `v-model.number` so the
    // draft keeps receiving numbers; the color inputs stay plain string
    // v-models.
    for (const [field, min, max, step] of [
      ["glassTintOpacity", "0", "1", "0.01"],
      ["blurPx", "0", "24", "1"],
      ["gradientAngle", "0", "360", "1"],
      ["backgroundOpacity", "0", "1", "0.01"],
      ["overlayStrength", "0", "0.72", "0.01"],
    ] as const) {
      expect(appearancePanelVue).toContain(
        `v-model.number="${field}" type="range" min="${min}" max="${max}" step="${step}"`,
      );
    }
    expect(appearancePanelVue.match(/v-model\.number=/g)?.length).toBe(5);
    for (const field of ["glassTintColor", "solidColor", "gradientStartColor", "gradientEndColor", "customTextColor"]) {
      expect(appearancePanelVue, field).toContain(`v-model="${field}" type="color"`);
    }
    expect(appearancePanelVue.match(/type="color"/g)?.length).toBe(5);
    // The fit/position selects keep their full option lists.
    expect(appearancePanelVue).toContain(
      '<option value="cover">{{ t("settings.fit.cover") }}</option><option value="contain">{{ t("settings.fit.contain") }}</option><option value="stretch">{{ t("settings.fit.stretch") }}</option>',
    );
    expect(appearancePanelVue).toContain(
      '<option value="center">{{ t("settings.position.center") }}</option><option value="top">{{ t("settings.position.top") }}</option><option value="bottom">{{ t("settings.position.bottom") }}</option>',
    );
  });

  it("routes the background picker through the parent's asset lifecycle", () => {
    // The child reports the click; the parent's chooseAsset('background') —
    // with its adopt-before-release ordering — handles it, exactly like the
    // avatar's routing in GeneralPanel.
    expect(appearancePanelVue).toContain(`$emit('chooseBackground')`);
    expect(settingsPanelVue).toContain(`@choose-background="chooseAsset('background')"`);
    // Exactly two release sites remain (avatar + background), both
    // parent-side — the count the lifecycle harnesses pin elsewhere.
    expect(settingsPanelVue.match(/void discard\(previous\)/g)?.length).toBe(2);
  });

  it("shows the unavailable-image hint from the parent's computed", () => {
    // The pre-existing cross-mode hint semantics ride the parent's
    // editedImageUnavailable computation unchanged (the known P3 stays out of
    // this extraction's scope).
    expect(appearancePanelVue).toContain(
      '<small v-if="editedImageUnavailable" class="asset-unavailable">{{ t("settings.imageUnavailable") }}</small>',
    );
  });

  it("renders the wallpaper availability from the parent's prop", () => {
    expect(appearancePanelVue).toContain(
      '{{ wallpaperAvailable ? t("settings.wallpaperAvailable") : t("settings.wallpaperUnavailable") }}',
    );
    expect(appearancePanelVue).toContain('<span class="asset-status">');
  });

  it("renders the text contrast modes and the conditional custom color", () => {
    expect(appearancePanelVue).toContain(
      "v-for=\"option in ['auto', 'light', 'dark', 'custom'] as const\"",
    );
    expect(appearancePanelVue).toMatch(/v-if="settings\.textContrast === 'custom'"/);
    expect(appearancePanelVue).toContain(`$emit('updateTextContrast', option)`);
    expect(appearancePanelVue).toContain(`class="reset-appearance"`);
    expect(appearancePanelVue).toContain(`$emit('resetAppearance')`);
  });
});

describe("AppearancePanel extraction: lifecycle (S1-E5)", () => {
  it("the parent mounts AppearancePanel unconditionally inside its v-show tabpanel", () => {
    // The wrapper stays the parent's and the child carries no mounting
    // condition: Appearance state is not transient across categories, and the
    // mounting shape stays the one the other four extractions pinned.
    expect(settingsPanelVue).toMatch(
      /<section\s+v-show="activeCategory === 'appearance'"\s+id="settings-panel-appearance"\s+class="settings-category-panel"\s+role="tabpanel"\s+aria-labelledby="settings-tab-appearance"\s*>(?:\s*<!--[\s\S]*?-->\s*)*<AppearancePanel\b/,
    );
    // S2-P2 removed the Weather tabpanel: exactly four remain.
    expect(settingsPanelVue.match(/v-show="activeCategory === '/g)?.length).toBe(4);
    expect(settingsPanelVue.match(/role="tabpanel"/g)?.length).toBe(4);
  });

  it("the child carries no lifecycle of its own", () => {
    // It mounts and unmounts with the Settings panel; the asset cleanup, the
    // language rollback and every other side effect stay in the parent's
    // onBeforeUnmount.
    expect(appearancePanelVue).not.toContain("onMounted");
    expect(appearancePanelVue).not.toContain("onBeforeUnmount");
    expect(appearancePanelVue).not.toContain("invoke");
    expect(settingsPanelVue).toContain("onBeforeUnmount(() => {");
  });

  it("the Save/Cancel footer stays in the parent", () => {
    // One save in flight at a time, the failure text in the footer, the
    // cancel path — none of it is the child's business.
    for (const owned of ["savePending", "settings-footer", "settings-cancel", 'class="save-settings"', "$emit('close')"]) {
      expect(settingsPanelVue, owned).toContain(owned);
      expect(appearancePanelVue, owned).not.toContain(owned);
    }
  });
});

/**
 * --- Executed parent handlers (behavior harness) ----------------------------
 *
 * The node environment cannot mount the component, but the parent's real
 * handler bodies — extracted from the SFC source and executed against a
 * three-profile draft — can prove the two isolation invariants as behavior
 * rather than restated string matches:
 *
 * - a field edit lands in the edited profile only, never by identity into
 *   another profile, and survives switching the edited profile;
 * - selecting a profile changes only the editor state, never the mode the
 *   product persists;
 * - reset replaces only the edited profile, with the defaults, and clears
 *   the picked-image preview state.
 */

/** A minimal writable ref whose writes are logged, in order, into `events`. */
function trackedRef<T>(name: string, initial: T, events: string[]) {
  let value = initial;
  return {
    get value() {
      return value;
    },
    set value(next: T) {
      events.push(`${name}=${JSON.stringify(next)}`);
      value = next;
    },
  };
}

function extractHandler(name: string): string {
  const source = extractBlock(settingsPanelVue, `function ${name}(`, "\n}");
  expect(source, `${name} must be extractable from SettingsPanel.vue`).toContain(
    `function ${name}(`,
  );
  // The bodies are plain JavaScript; strip the parameter type annotation the
  // SFC carries (the same stripping the asset-lifecycle harnesses do for
  // generics).
  return source.replace(/: (?:ProductWindowMode|BackgroundType|ImageFit|ImagePosition|TextContrast|string|number)(?=\))/, "");
}

/** Three profiles with a distinct sentinel value each, so cross-talk is visible. */
function sentinelProfiles(): Record<ProductWindowMode, AppearanceSettings> {
  const angles: Record<ProductWindowMode, number> = { sidebar: 10, floating: 20, desktop: 30 };
  return {
    sidebar: { ...DEFAULT_APPEARANCE, gradientAngle: angles.sidebar },
    floating: { ...DEFAULT_APPEARANCE, gradientAngle: angles.floating },
    desktop: { ...DEFAULT_APPEARANCE, gradientAngle: angles.desktop },
  };
}

function buildMutationHarness(mode: ProductWindowMode) {
  const events: string[] = [];
  const profiles = sentinelProfiles();
  const original = { ...profiles };
  const appearanceMode = trackedRef<ProductWindowMode>("appearanceMode", mode, events);
  // The parent's projection: the edited profile, recomputed from the mode.
  const appearanceSettings = {
    get value(): AppearanceSettings {
      return profiles[appearanceMode.value];
    },
  };
  const pickedImage = trackedRef<unknown>("pickedImage", { mode, available: true }, events);
  const imagePreviewAvailable = trackedRef<boolean>("imagePreviewAvailable", true, events);

  const handlers = new Function(
    "appearanceMode",
    "appearanceSettings",
    "appearanceProfiles",
    "pickedImage",
    "imagePreviewAvailable",
    "DEFAULT_APPEARANCE",
    `"use strict";\n${[
      extractHandler("selectAppearanceProfile"),
      extractHandler("updateGradientAngle"),
      extractHandler("updateBackgroundType"),
      extractHandler("resetAppearance"),
    ].join("\n")};\nreturn { selectAppearanceProfile, updateGradientAngle, updateBackgroundType, resetAppearance };`,
  )(appearanceMode, appearanceSettings, { value: profiles }, pickedImage, imagePreviewAvailable, DEFAULT_APPEARANCE);

  return {
    handlers,
    profiles,
    original,
    appearanceMode,
    appearanceSettings,
    pickedImage,
    imagePreviewAvailable,
    events,
  };
}

describe("Appearance profile isolation (behavior harness)", () => {
  it("a field edit lands in the edited profile only and survives switching", () => {
    const h = buildMutationHarness("floating");
    h.handlers.updateGradientAngle(90);
    // Floating edited; the other two keep their sentinel values and their
    // identities — no write ever reached them, not even by mutation.
    expect(h.profiles.floating.gradientAngle).toBe(90);
    expect(h.profiles.sidebar.gradientAngle).toBe(10);
    expect(h.profiles.desktop.gradientAngle).toBe(30);
    expect(h.profiles.sidebar).toBe(h.original.sidebar);
    expect(h.profiles.desktop).toBe(h.original.desktop);
    // Switching the editor to desktop must not carry the edit over, and the
    // floating edit persists.
    h.handlers.selectAppearanceProfile("desktop");
    expect(h.appearanceMode.value).toBe("desktop");
    expect(h.appearanceSettings.value.gradientAngle).toBe(30);
    h.handlers.updateGradientAngle(200);
    expect(h.profiles.desktop.gradientAngle).toBe(200);
    expect(h.profiles.floating.gradientAngle).toBe(90);
    expect(h.profiles.sidebar.gradientAngle).toBe(10);
    expect(h.profiles.sidebar).toBe(h.original.sidebar);
    expect(h.profiles.floating).toBe(h.original.floating);
  });

  it("a material switch lands in the edited profile only", () => {
    const h = buildMutationHarness("sidebar");
    h.handlers.updateBackgroundType("solid");
    expect(h.profiles.sidebar.backgroundType).toBe("solid");
    expect(h.profiles.floating.backgroundType).toBe("glass");
    expect(h.profiles.desktop.backgroundType).toBe("glass");
    expect(h.profiles.floating).toBe(h.original.floating);
    expect(h.profiles.desktop).toBe(h.original.desktop);
  });

  it("profile selection changes only the editor state, never the product mode", () => {
    const h = buildMutationHarness("floating");
    // The handler's only write is the editor ref; the product's persisted
    // mode is not in its reachable state at all (it lives in the parent's
    // `props.settings.mode`, which this harness's scope does not even carry).
    h.handlers.selectAppearanceProfile("desktop");
    expect(h.appearanceMode.value).toBe("desktop");
    expect(h.events).toEqual(["appearanceMode=\"desktop\""]);
    // All three profiles are untouched by a selection.
    expect(h.profiles.sidebar).toBe(h.original.sidebar);
    expect(h.profiles.floating).toBe(h.original.floating);
    expect(h.profiles.desktop).toBe(h.original.desktop);
  });

  it("reset replaces only the edited profile with the defaults and clears the preview state", () => {
    const h = buildMutationHarness("desktop");
    h.handlers.resetAppearance();
    // The edited profile is a fresh defaults object...
    expect(h.profiles.desktop).toEqual(DEFAULT_APPEARANCE);
    expect(h.profiles.desktop).not.toBe(h.original.desktop);
    // ...the other two keep their values and identities...
    expect(h.profiles.sidebar.gradientAngle).toBe(10);
    expect(h.profiles.floating.gradientAngle).toBe(20);
    expect(h.profiles.sidebar).toBe(h.original.sidebar);
    expect(h.profiles.floating).toBe(h.original.floating);
    // ...and the picked-image preview state is dropped.
    expect(h.pickedImage.value).toBeNull();
    expect(h.imagePreviewAvailable.value).toBe(false);
  });

  it("resetting a second profile never resurrects or leaks the first reset", () => {
    const h = buildMutationHarness("floating");
    h.handlers.resetAppearance();
    h.handlers.selectAppearanceProfile("sidebar");
    h.handlers.resetAppearance();
    expect(h.profiles.floating).toEqual(DEFAULT_APPEARANCE);
    expect(h.profiles.sidebar).toEqual(DEFAULT_APPEARANCE);
    expect(h.profiles.desktop.gradientAngle).toBe(30);
    expect(h.profiles.desktop).toBe(h.original.desktop);
  });
});
