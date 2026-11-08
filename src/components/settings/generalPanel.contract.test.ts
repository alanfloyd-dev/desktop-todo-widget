import { describe, expect, it } from "vitest";
// Source-pinning contracts for the S1-E4 GeneralPanel extraction — the same
// `?raw` pattern as `weatherPanel.contract.test.ts` and
// `modulesPanel.contract.test.ts` (the Vitest node environment has no
// component mounting, so the boundary is pinned at the source level).
import settingsPanelVue from "../SettingsPanel.vue?raw";
import generalPanelVue from "./GeneralPanel.vue?raw";
// The real normalization helper for the lifecycle harness below (pure, and the
// same module the parent imports).
import { normalizeLanguage } from "../../i18n";

/**
 * S1-E4 moved the General category's controls into `GeneralPanel` while the
 * persisted fields stayed SettingsPanel drafts. These pins hold the boundary
 * in place: the child owns the presentation (rows, initials, the effective-
 * language hint), the parent keeps the dirty snapshot, the Save payload, the
 * draft mutations, the language preview + rollback, the avatar picker's
 * native commands and the provisional-asset cleanup, and the `v-show`
 * tabpanel. The avatar row's error is a parent-rendered typed slot, because
 * `assetError` is the parent's state (one ref shared with the background
 * picker) and the Rust source pin in `appearance.rs` requires the rendering
 * expression to stay in SettingsPanel.vue. The moved markup stays the
 * tabpanel's direct children (a multi-root fragment, no wrapper element).
 *
 * R-E4-01 (the avatar provisional-asset lifecycle) is additionally held by a
 * behavior harness at the bottom of this file: the parent's real
 * `chooseAsset` and `onBeforeUnmount` sources are extracted from the SFC and
 * executed against scripted native ports, so the async invariants — never
 * release the current provisional before its replacement is adopted — are
 * tested against the shipped source, not restated.
 */

/** Every catalog key the General category renders literally (in the child, after E4). */
const GENERAL_I18N_KEYS = [
  "settings.language.label",
  "settings.language.hint",
  "settings.language.effective",
  "settings.language.restartHint",
  "settings.displayName",
  "settings.displayNameHint",
  "settings.displayNamePlaceholder",
  "settings.avatar",
  "settings.avatarHint",
  "settings.avatarAriaLabel",
  "settings.chooseImage",
  "settings.removeAvatar",
  "settings.dayRollover",
  "settings.dayRolloverHint",
];

describe("GeneralPanel extraction: parent draft ownership (S1-E4)", () => {
  it("keeps every persisted General field in the dirty snapshot and the Save payload", () => {
    // The exact draft-to-payload lines appear exactly twice: once in the
    // dirtyDraft projection and once in the save emit — the "dirty" and
    // "would write" sets stay identical by construction.
    for (const line of [
      "dayRollover: dayRollover.value,",
      "language: language.value,",
      "displayName: displayName.value,",
      "avatarAssetId: avatarAssetId.value,",
    ]) {
      expect(settingsPanelVue.match(new RegExp(line.replace(/\./g, "\\."), "g"))?.length, line).toBe(2);
    }
  });

  it("writes the display-name and rollover drafts when the child reports an edit", () => {
    expect(settingsPanelVue).toMatch(
      /function updateDisplayName\(value: string\) \{\s*displayName\.value = value;\s*\}/,
    );
    expect(settingsPanelVue).toMatch(
      /function updateDayRollover\(value: string\) \{\s*dayRollover\.value = value;\s*\}/,
    );
  });

  it("keeps the language preview side effect with the draft write", () => {
    // selectLanguage writes the draft AND flips the app's one i18n instance —
    // splitting those two apart would fork the immediate preview from the
    // value Save persists.
    expect(settingsPanelVue).toMatch(
      /function selectLanguage\(value: Language\) \{\s*language\.value = value;\s*i18n\.setLanguage\(value\);\s*\}/,
    );
    expect(generalPanelVue).not.toContain("setLanguage");
  });

  it("keeps the unsaved-preview rollback and the languageSaved flag in the parent", () => {
    // The rollback runs in the parent's onBeforeUnmount, guarded by the
    // save-attempt flag a failed save deliberately leaves set.
    expect(settingsPanelVue).toContain("let languageSaved = false;");
    expect(settingsPanelVue).toContain("languageSaved = true;");
    expect(settingsPanelVue).toContain(
      "if (!languageSaved) i18n.setLanguage(normalizeLanguage(props.settings.language));",
    );
    expect(generalPanelVue).not.toContain("languageSaved");
    expect(generalPanelVue).not.toContain("onBeforeUnmount");
  });

  it("keeps the avatar actions, the native commands and the provisional-asset lifecycle in the parent", () => {
    // The child may not reach any of it: the picker, the normalization, the
    // store/discard commands and the cleanup set are one lifecycle, and the
    // Rust pins in appearance.rs read them from SettingsPanel.vue.
    for (const owned of [
      "choose_local_asset",
      "store_managed_asset",
      "discard_managed_asset",
      "normalizeAvatarImage(",
      "provisionalAssets",
      "function removeAvatar()",
    ]) {
      expect(settingsPanelVue, owned).toContain(owned);
      expect(generalPanelVue, owned).not.toContain(owned);
    }
  });

  it("the child cannot reach the settings save path", () => {
    for (const forbidden of ["settingsSnapshot", "settingsDraftIsDirty", "dirtyDraft", "update_product_settings"]) {
      expect(generalPanelVue, forbidden).not.toContain(forbidden);
    }
  });
});

describe("GeneralPanel extraction: child interface (S1-E4)", () => {
  it("receives the required display state as explicit typed props", () => {
    for (const prop of [
      "language: Language;",
      "displayName: string;",
      "dayRollover: string;",
      "avatarAssetId: string | null;",
      "avatarPreviewUrl: string;",
      "avatarPreviewAvailable: boolean;",
    ]) {
      expect(generalPanelVue, prop).toContain(prop);
    }
    expect(generalPanelVue).toContain("defineProps<{");
  });

  it("reports persisted changes back through explicit typed events", () => {
    for (const event of [
      "selectLanguage: [value: Language];",
      "updateDisplayName: [value: string];",
      "updateDayRollover: [value: string];",
      "chooseAvatar: [];",
      "removeAvatar: [];",
    ]) {
      expect(generalPanelVue, event).toContain(event);
    }
  });

  it("declares the avatar error as a typed slot, and the parent renders it", () => {
    // The parent owns `assetError` (one ref shared with the background
    // picker) and the appearance.rs source pin names SettingsPanel.vue as
    // the place the avatar expression lives — so the element stays there and
    // the child only marks where it renders.
    expect(generalPanelVue).toContain("defineSlots<{ avatarError?: () => unknown }>");
    expect(generalPanelVue).toContain('<slot name="avatarError"></slot>');
    expect(generalPanelVue).not.toContain("assetError");
    expect(settingsPanelVue).toContain("#avatarError");
    expect(settingsPanelVue).toContain("assetError?.kind === 'avatar'");
  });

  it("the parent passes every binding exactly once on the child element (N-1)", () => {
    const usages = settingsPanelVue.match(/<GeneralPanel\b[^>]*>/g);
    expect(usages?.length).toBe(1);
    const tag = usages![0];
    // No conditional mounting may ride on the element itself.
    expect(tag).not.toContain("v-if");
    expect(tag).not.toContain("v-show");
    for (const binding of [
      ":language",
      ":display-name",
      ":day-rollover",
      ":avatar-asset-id",
      ":avatar-preview-url",
      ":avatar-preview-available",
      "@select-language",
      "@update-display-name",
      "@update-day-rollover",
      "@choose-avatar",
      "@remove-avatar",
    ]) {
      // Counted inside the component element's tag only — occurrences
      // elsewhere in the parent file are irrelevant to this contract.
      const occurrences = tag.split(binding).length - 1;
      expect(occurrences, `binding ${binding} must appear exactly once on the <GeneralPanel> element`).toBe(1);
    }
  });

  it("renders as a multi-root fragment: no wrapper between the tabpanel and the moved markup", () => {
    // The template opens straight onto the language row and closes right
    // after the rollover row — the moved elements stay the
    // `settings-category-panel`'s direct children, so the rendered DOM (and
    // every descendant selector in styles.css) is identical to the inline
    // layout this extraction replaced.
    expect(generalPanelVue).toMatch(
      /<template>\s*<div class="settings-row">/,
    );
    expect(generalPanelVue).toMatch(
      /<\/label>\s*<\/template>\s*$/,
    );
  });

  it("keeps every General translation key in the child template", () => {
    // Template attribute values quote with single quotes; script strings with
    // double quotes — pin the key itself, not one quote style.
    for (const key of GENERAL_I18N_KEYS) {
      expect(generalPanelVue, key).toMatch(new RegExp(`['"]${key}['"]`));
    }
  });
});

describe("GeneralPanel extraction: moved-as-a-unit responsibilities (S1-E4)", () => {
  it("the effective-language hint and the initials live in GeneralPanel, not SettingsPanel", () => {
    // Pure presentation, moved with the rows that render them. (The option
    // labels go through the shared `languageOptionLabel`, which the child
    // imports for both the buttons and the hint.)
    for (const owned of ["effectiveLanguage", "profileInitials"]) {
      expect(generalPanelVue, owned).toContain(owned);
      expect(settingsPanelVue, owned).not.toContain(owned);
    }
  });

  it("the language buttons stay real pressed-state controls driving one event", () => {
    expect(generalPanelVue).toContain(':aria-pressed="language === option"');
    expect(generalPanelVue).toContain("$emit('selectLanguage', option)");
  });

  it("the avatar preview renders from the parent's preview state with the initials fallback (N-2)", () => {
    // The preview element hosts both render states, in this exact structure:
    // the image shows exactly while the parent's preview state says an image
    // exists, its source is the parent's preview URL, and the fallback
    // renders the parent's displayName initials — both inside the preview
    // span, which the regex's nesting pins.
    expect(generalPanelVue).toMatch(
      /<span class="settings-avatar-preview" :aria-label="t\('settings\.avatarAriaLabel'\)">\s*<img v-if="avatarPreviewAvailable" :src="avatarPreviewUrl" alt="" \/>\s*<span v-else>\{\{ profileInitials\(displayName\) \}\}<\/span>\s*<\/span>/,
    );
    // The Remove entry is live exactly while a draft asset exists, and it
    // reports through the one removal event.
    expect(generalPanelVue).toContain(':disabled="!avatarAssetId"');
    expect(generalPanelVue).toContain(`@click="$emit('removeAvatar')"`);
  });
});

describe("GeneralPanel extraction: lifecycle (S1-E4)", () => {
  it("the parent mounts GeneralPanel unconditionally inside its v-show tabpanel", () => {
    // The wrapper stays the parent's and the child carries no `v-if`: no
    // General state is transient across categories, but the mounting shape
    // stays the one the other three extractions pinned.
    expect(settingsPanelVue).toMatch(
      /<section\s+v-show="activeCategory === 'general'"\s+id="settings-panel-general"\s+class="settings-category-panel"\s+role="tabpanel"\s+aria-labelledby="settings-tab-general"\s*>(?:\s*<!--[\s\S]*?-->\s*)*<GeneralPanel\b/,
    );
    expect(settingsPanelVue.match(/v-show="activeCategory === '/g)?.length).toBe(5);
    expect(settingsPanelVue.match(/role="tabpanel"/g)?.length).toBe(5);
  });

  it("the slot content is the only place the avatar error element remains", () => {
    // Exactly one avatar-error rendering (the slot), while the background
    // row keeps its own — the pair the Rust pin expects.
    expect(settingsPanelVue.match(/assetError\?\.kind === 'avatar'/g)?.length).toBe(1);
    expect(settingsPanelVue.match(/assetError\?.kind === 'background'/g)?.length).toBe(1);
    expect(settingsPanelVue).not.toContain('v-if="assetMessage"');
  });
});

/**
 * --- R-E4-01: avatar provisional-asset lifecycle ----------------------------
 *
 * The parent's real `chooseAsset` and the real `onBeforeUnmount` callback are
 * extracted from the SFC source and executed here against scripted ports (the
 * native invoke surface, the i18n copy lookup and writable draft refs). The
 * node environment cannot mount the component, but it can run the exact
 * function bodies the app ships, so the async lifecycle invariants are tested
 * as behavior, not restated as string matches:
 *
 * - the currently referenced provisional avatar is never released before a
 *   replacement has been picked, normalized, stored AND adopted by the draft;
 * - a cancelled, failed or rejected pick leaves the draft, the preview and
 *   the provisional tracking untouched;
 * - adoption happens first, the superseded provisional's release second;
 * - the unmount cleanup discards unsaved provisionals and never touches
 *   assets the persisted settings still reference.
 */

/** Extracts a block from the SFC source: `startMarker` .. first `endMarker`. */
function extractBlock(source: string, startMarker: string, endMarker: string): string {
  const start = source.indexOf(startMarker);
  if (start === -1) return "";
  const end = source.indexOf(endMarker, start);
  if (end === -1) return "";
  return source.slice(start, end + endMarker.length);
}

/** The parent's real chooseAsset, with its TypeScript generics stripped. */
const CHOOSE_ASSET_SOURCE = extractBlock(settingsPanelVue, "async function chooseAsset(", "\n}")
  .replace('function chooseAsset(kind: "background" | "avatar")', "function chooseAsset(kind)")
  .replace(/invoke<[^>]*>\(/g, "invoke(");

/** The parent's real unmount cleanup statement. */
const UNMOUNT_SOURCE = extractBlock(settingsPanelVue, "onBeforeUnmount(() => {", "\n});");

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

interface HarnessOptions {
  /** The native picker's outcome; default resolves `null` (cancelled). */
  pick?: () => Promise<unknown>;
  /** The store command's outcome; default rejects (no store scripted). */
  store?: () => Promise<unknown>;
  /** The WebView normalization step; default normalizes trivially. */
  normalize?: (dataUrl: string) => Promise<string>;
  /** Provisional ids tracked before the call (e.g. a previously picked avatar). */
  initialProvisional?: string[];
  /** The avatar draft value before the call. */
  initialDraftAvatar?: string | null;
  /** The avatar id the *persisted* settings reference (unmount tests). */
  savedAvatarId?: string | null;
  /** A background image id the persisted settings reference (unmount tests). */
  savedProfileImageId?: string | null;
  /** Whether a save was attempted before unmount (unmount tests). */
  languageSaved?: boolean;
  /** The persisted language preference (unmount tests). */
  settingsLanguage?: string;
}

function buildHarness(options: HarnessOptions = {}) {
  expect(CHOOSE_ASSET_SOURCE, "chooseAsset must be extractable from SettingsPanel.vue").toContain(
    "async function chooseAsset",
  );
  expect(UNMOUNT_SOURCE, "onBeforeUnmount must be extractable from SettingsPanel.vue").toContain(
    "onBeforeUnmount",
  );

  const events: string[] = [];
  const discards: string[] = [];
  const i18nCalls: string[] = [];

  const assetError = trackedRef<{ kind: string; message: string } | null>("assetError", null, events);
  const avatarAssetId = trackedRef<string | null>("avatarAssetId", options.initialDraftAvatar ?? null, events);
  const avatarPreviewUrl = trackedRef<string>("avatarPreviewUrl", "", events);
  const avatarPreviewAvailable = trackedRef<boolean>("avatarPreviewAvailable", false, events);
  const pickedImage = trackedRef<unknown>("pickedImage", null, events);
  const imagePreviewAvailable = trackedRef<boolean>("imagePreviewAvailable", false, events);
  const provisionalAssets = new Set<string>(options.initialProvisional ?? []);

  const invoke = async (command: string, args?: Record<string, unknown>) => {
    events.push(`invoke:${command}`);
    if (command === "choose_local_asset") return await (options.pick ? options.pick() : Promise.resolve(null));
    if (command === "store_managed_asset") {
      return await (options.store ? options.store() : Promise.reject(new Error("no store scripted")));
    }
    throw new Error(`unexpected command in harness: ${command}`);
  };
  const discard = async (assetId: string) => {
    events.push(`discard:${assetId}`);
    discards.push(assetId);
  };
  const normalizeAvatarImage = options.normalize ?? ((dataUrl: string) => Promise.resolve(`normalized(${dataUrl})`));

  const chooseAsset = new Function(
    "window",
    "invoke",
    "t",
    "imageErrorLabel",
    "normalizeAvatarImage",
    "sampleImageLuminance",
    "discard",
    "provisionalAssets",
    "assetError",
    "avatarAssetId",
    "avatarPreviewUrl",
    "avatarPreviewAvailable",
    "appearanceSettings",
    "appearanceMode",
    "pickedImage",
    "imagePreviewAvailable",
    `"use strict";\n${CHOOSE_ASSET_SOURCE}\nreturn chooseAsset;`,
  )(
    { __TAURI_INTERNALS__: {} },
    invoke,
    (key: string) => key,
    (reason: unknown) => `label(${String(reason)})`,
    normalizeAvatarImage,
    () => Promise.resolve(0.5),
    discard,
    provisionalAssets,
    assetError,
    avatarAssetId,
    avatarPreviewUrl,
    avatarPreviewAvailable,
    { value: { imageAssetId: null } },
    { value: "desktop" },
    pickedImage,
    imagePreviewAvailable,
  );

  // The unmount callback is registered through a stub, then invoked directly.
  const registered: Array<() => void> = [];
  const props = {
    settings: {
      language: options.settingsLanguage ?? "zh-Hans",
      avatarAssetId: options.savedAvatarId ?? null,
      appearanceProfiles: {
        sidebar: { imageAssetId: null },
        floating: { imageAssetId: null },
        desktop: { imageAssetId: options.savedProfileImageId ?? null },
      },
    },
  };
  new Function(
    "onBeforeUnmount",
    "languageSaved",
    "i18n",
    "normalizeLanguage",
    "props",
    "provisionalAssets",
    "discard",
    `"use strict";\n${UNMOUNT_SOURCE}`,
  )(
    (cb: () => void) => {
      registered.push(cb);
    },
    options.languageSaved ?? false,
    { setLanguage: (language: string) => { events.push(`setLanguage:${language}`); i18nCalls.push(language); } },
    normalizeLanguage,
    props,
    provisionalAssets,
    discard,
  );

  return {
    chooseAsset,
    runUnmount: () => registered.forEach((cb) => cb()),
    events,
    discards,
    i18nCalls,
    provisional: provisionalAssets,
    avatarAssetId,
    avatarPreviewUrl,
    avatarPreviewAvailable,
    assetError,
  };
}

describe("R-E4-01 avatar provisional lifecycle (behavior harness)", () => {
  it("keeps the provisional tracking intact while a replacement picker is pending", async () => {
    // The picker's native dialog is awaited; nothing may be released until it
    // returns and a replacement is adopted.
    let release!: (value: unknown) => void;
    const pending = new Promise((resolve) => {
      release = resolve;
    });
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => pending,
    });
    const flow = h.chooseAsset("avatar");
    await Promise.resolve();
    expect(h.events).toContain("invoke:choose_local_asset");
    expect(h.discards, "nothing may be discarded while the picker is open").toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    release(null);
    await flow;
    expect(h.provisional.has("avatar-b")).toBe(true);
  });

  it("picker cancellation does not invalidate the current draft", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => Promise.resolve(null),
    });
    await h.chooseAsset("avatar");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.avatarPreviewAvailable.value).toBe(false);
    expect(h.assetError.value).toBeNull();
  });

  it("a failed pick does not invalidate the current draft", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => Promise.reject(new Error("selected image is too large")),
    });
    await h.chooseAsset("avatar");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.assetError.value?.kind).toBe("avatar");
    expect(h.assetError.value?.message).toContain("too large");
  });

  it("a normalization failure does not invalidate the current draft", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => Promise.resolve({ dataUrl: "data:image/png;base64,AAAA" }),
      normalize: () => Promise.reject(new Error("decode failed")),
    });
    await h.chooseAsset("avatar");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.assetError.value?.kind).toBe("avatar");
  });

  it("a store failure does not invalidate the current draft", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => Promise.resolve({ dataUrl: "data:image/png;base64,AAAA" }),
      store: () => Promise.reject(new Error("store refused")),
    });
    await h.chooseAsset("avatar");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.assetError.value?.kind).toBe("avatar");
  });

  it("an invalid storage response does not invalidate the current draft", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-b"],
      initialDraftAvatar: "avatar-b",
      pick: () => Promise.resolve({ dataUrl: "data:image/png;base64,AAAA" }),
      store: () => Promise.resolve({ assetId: "", dataUrl: "" }),
    });
    await h.chooseAsset("avatar");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("avatar-b")).toBe(true);
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.assetError.value).toEqual({ kind: "avatar", message: "settings.error.imageUnreadable" });
  });

  it("a successful replacement adopts the draft and preview, then releases the superseded provisional", async () => {
    const h = buildHarness({
      initialProvisional: ["avatar-a"],
      initialDraftAvatar: "avatar-a",
      pick: () => Promise.resolve({ dataUrl: "data:image/png;base64,BBBB" }),
      store: () => Promise.resolve({ assetId: "avatar-b", dataUrl: "data:image/png;base64,BBBB" }),
    });
    await h.chooseAsset("avatar");
    expect(h.avatarAssetId.value).toBe("avatar-b");
    expect(h.avatarPreviewUrl.value).toBe("data:image/png;base64,BBBB");
    expect(h.avatarPreviewAvailable.value).toBe(true);
    expect([...h.provisional]).toEqual(["avatar-b"]);
    expect(h.discards).toEqual(["avatar-a"]);
    // The adoption strictly precedes the release: the draft can never name an
    // asset whose file was already discarded.
    const adoptedAt = h.events.indexOf(`avatarAssetId="avatar-b"`);
    const releasedAt = h.events.indexOf("discard:avatar-a");
    expect(adoptedAt).toBeGreaterThanOrEqual(0);
    expect(releasedAt).toBeGreaterThan(adoptedAt);
  });

  it("a previous avatar that is not provisional (already saved) is never released by the picker", async () => {
    const h = buildHarness({
      initialProvisional: [],
      initialDraftAvatar: "avatar-saved",
      pick: () => Promise.resolve({ dataUrl: "data:image/png;base64,CCCC" }),
      store: () => Promise.resolve({ assetId: "avatar-c", dataUrl: "data:image/png;base64,CCCC" }),
    });
    await h.chooseAsset("avatar");
    expect(h.avatarAssetId.value).toBe("avatar-c");
    expect(h.discards).toEqual([]);
    expect([...h.provisional]).toEqual(["avatar-c"]);
  });

  it("rapid replacement A→B→C releases each superseded provisional exactly once", async () => {
    let pickCount = 0;
    const stored = [
      { assetId: "avatar-b", dataUrl: "data:image/png;base64,BBBB" },
      { assetId: "avatar-c", dataUrl: "data:image/png;base64,CCCC" },
    ];
    let storeCount = 0;
    const h = buildHarness({
      initialProvisional: ["avatar-a"],
      initialDraftAvatar: "avatar-a",
      pick: () => Promise.resolve({ dataUrl: `data:image/png;base64,${"D".repeat(++pickCount)}` }),
      store: () => Promise.resolve(stored[storeCount++]),
    });
    await h.chooseAsset("avatar");
    expect(h.avatarAssetId.value).toBe("avatar-b");
    await h.chooseAsset("avatar");
    expect(h.avatarAssetId.value).toBe("avatar-c");
    // A was released when B was adopted; B when C was adopted; only C remains
    // tracked and eligible for the unmount cleanup.
    expect(h.discards).toEqual(["avatar-a", "avatar-b"]);
    expect([...h.provisional]).toEqual(["avatar-c"]);
    expect(h.avatarPreviewUrl.value).toBe("data:image/png;base64,CCCC");
  });

  it("unmount cleanup discards unsaved provisional assets (cancellation path)", () => {
    const h = buildHarness({ initialProvisional: ["avatar-b"], savedAvatarId: null });
    h.runUnmount();
    expect(h.discards).toEqual(["avatar-b"]);
    // The language rollback is part of the same callback and stays intact.
    expect(h.i18nCalls).toEqual(["zh-Hans"]);
  });

  it("unmount cleanup never discards assets the persisted settings still reference", () => {
    const saved = buildHarness({
      initialProvisional: ["avatar-b"],
      savedAvatarId: "avatar-b",
      languageSaved: true,
    });
    saved.runUnmount();
    expect(saved.discards).toEqual([]);
    const savedBackground = buildHarness({
      initialProvisional: ["background-x"],
      savedProfileImageId: "background-x",
      languageSaved: true,
    });
    savedBackground.runUnmount();
    expect(savedBackground.discards).toEqual([]);
  });

  it("the unmount language rollback follows the save-attempt flag", () => {
    // No save attempted → the unsaved preview is reverted to the persisted
    // preference; a save attempt (even a failed one) leaves the preview up.
    const rolled = buildHarness({ languageSaved: false, settingsLanguage: "en" });
    rolled.runUnmount();
    expect(rolled.i18nCalls).toEqual(["en"]);
    const kept = buildHarness({ languageSaved: true, settingsLanguage: "en" });
    kept.runUnmount();
    expect(kept.i18nCalls).toEqual([]);
  });
});

describe("R-E4-01 avatar provisional lifecycle (source pins)", () => {
  it("the avatar picker's cancel-return path releases nothing", () => {
    // Between the avatar branch opening and the cancel return there is no
    // discard: the release-early block must never come back above the pick.
    expect(settingsPanelVue).toMatch(
      /if \(kind === "avatar"\) \{\s*const picked = await invoke<AssetPayload \| null>\("choose_local_asset", \{ kind, persist: false \}\);\s*if \(!picked\?\.dataUrl\) return;/,
    );
  });

  it("adoption precedes the superseded release, in one block", () => {
    expect(settingsPanelVue).toMatch(
      /const previous = avatarAssetId\.value;\s*provisionalAssets\.add\(stored\.assetId\);\s*avatarAssetId\.value = stored\.assetId;\s*avatarPreviewUrl\.value = stored\.dataUrl;\s*avatarPreviewAvailable\.value = true;\s*if \(previous && previous !== stored\.assetId && provisionalAssets\.has\(previous\)\) \{\s*provisionalAssets\.delete\(previous\);\s*void discard\(previous\);\s*\}/,
    );
  });

  it("the background adopts its replacement before releasing the superseded provisional (E4.1)", () => {
    // E4 scoped R-E4-01 to the avatar; E4.1 gave the background the same
    // ordering. The block is pinned whole: snapshot after the pick returns,
    // full adoption (draft, type, luminance, preview), then the release —
    // the release-early form that dangled the draft on a cancelled pick must
    // not come back.
    expect(settingsPanelVue).toMatch(
      /const previous = appearanceSettings\.value\.imageAssetId;\s*provisionalAssets\.add\(selected\.assetId\);\s*appearanceSettings\.value\.imageAssetId = selected\.assetId;\s*appearanceSettings\.value\.backgroundType = "image";\s*appearanceSettings\.value\.sampledLuminance = await sampleImageLuminance\(selected\.dataUrl\);\s*pickedImage\.value = \{ mode: appearanceMode\.value, available: selected\.available \};\s*imagePreviewAvailable\.value = selected\.available;\s*if \(previous && previous !== selected\.assetId && provisionalAssets\.has\(previous\)\) \{\s*provisionalAssets\.delete\(previous\);\s*void discard\(previous\);\s*\}/,
    );
  });

  it("exactly two release sites exist (one per kind), both draft-owner-side", () => {
    expect(settingsPanelVue.match(/void discard\(previous\)/g)?.length).toBe(2);
  });
});
