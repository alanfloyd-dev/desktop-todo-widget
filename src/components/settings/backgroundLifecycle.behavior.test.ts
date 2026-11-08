import { describe, expect, it } from "vitest";
// S1-E4.1 — background provisional-asset lifecycle (behavior harness).
//
// The same approach as the R-E4-01 avatar harness in
// `generalPanel.contract.test.ts`: the parent's real `chooseAsset` and the real
// `onBeforeUnmount` callback are extracted from the SettingsPanel.vue source and
// executed here against scripted ports (the native invoke surface, the i18n
// copy lookup and writable draft refs). The node environment cannot mount the
// component, but it can run the exact function bodies the app ships.
//
// The background kind differs from the avatar kind in one structural way that
// these tests encode: the avatar picks a file read-only (`persist: false`),
// normalizes it in the WebView and stores it through `store_managed_asset`,
// while the background's `choose_local_asset` call persists the picked file
// itself and returns the new asset id — there is no normalize/store step. So
// "read/normalization/store failure" for a background pick is exactly a
// `choose_local_asset` rejection or a malformed payload, and that is what the
// failure tests below script.
//
// The invariant under test (the background twin of R-E4-01): the asset the
// draft references stays on disk and tracked until a replacement has been
// picked, persisted by the native picker AND adopted by the draft; only then
// may the superseded provisional be untracked and discarded. A cancelled,
// failed or malformed pick leaves the draft, its tracking and the file
// untouched — otherwise Save persists a dangling reference and the background
// silently disappears after reopening Settings.
import settingsPanelVue from "../SettingsPanel.vue?raw";
import { normalizeLanguage } from "../../i18n";

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

/**
 * The appearance draft the background branch writes through.
 *
 * Unlike a plain ref, the profile object's `imageAssetId` field is where the
 * adoption lands, so that one write is logged too — the adopt-before-release
 * ordering is asserted against it.
 */
function trackedAppearanceSettings(
  initial: { imageAssetId: string | null; backgroundType: string; sampledLuminance: number | null },
  events: string[],
) {
  const state = { ...initial };
  const profile = {
    get imageAssetId() {
      return state.imageAssetId;
    },
    set imageAssetId(next: string | null) {
      events.push(`imageAssetId=${JSON.stringify(next)}`);
      state.imageAssetId = next;
    },
    backgroundType: initial.backgroundType,
    sampledLuminance: initial.sampledLuminance,
  };
  return {
    value: profile,
    state,
  };
}

interface HarnessOptions {
  /** The native picker's outcome; default resolves `null` (cancelled). */
  pick?: () => Promise<unknown>;
  /** Provisional ids tracked before the call (e.g. a previously picked background). */
  initialProvisional?: string[];
  /** The edited profile's background image draft value before the call. */
  initialDraftBackground?: string | null;
  /** A background image id the *persisted* settings reference (unmount tests). */
  savedProfileImageId?: string | null;
  /** Whether a save was attempted before unmount (unmount tests). */
  languageSaved?: boolean;
}

function buildHarness(options: HarnessOptions = {}) {
  expect(CHOOSE_ASSET_SOURCE, "chooseAsset must be extractable from SettingsPanel.vue").toContain(
    "async function chooseAsset",
  );
  expect(CHOOSE_ASSET_SOURCE, "the background branch must be in the extracted body").toContain(
    "appearanceSettings.value.imageAssetId",
  );
  expect(UNMOUNT_SOURCE, "onBeforeUnmount must be extractable from SettingsPanel.vue").toContain(
    "onBeforeUnmount",
  );

  const events: string[] = [];
  const discards: string[] = [];

  const assetError = trackedRef<{ kind: string; message: string } | null>("assetError", null, events);
  const avatarAssetId = trackedRef<string | null>("avatarAssetId", null, events);
  const avatarPreviewUrl = trackedRef<string>("avatarPreviewUrl", "", events);
  const avatarPreviewAvailable = trackedRef<boolean>("avatarPreviewAvailable", false, events);
  const pickedImage = trackedRef<unknown>("pickedImage", null, events);
  const imagePreviewAvailable = trackedRef<boolean>("imagePreviewAvailable", false, events);
  const appearanceSettings = trackedAppearanceSettings(
    {
      imageAssetId: options.initialDraftBackground ?? null,
      backgroundType: options.initialDraftBackground ? "image" : "glass",
      sampledLuminance: null,
    },
    events,
  );
  const provisionalAssets = new Set<string>(options.initialProvisional ?? []);

  const invoke = async (command: string) => {
    events.push(`invoke:${command}`);
    if (command === "choose_local_asset") return await (options.pick ? options.pick() : Promise.resolve(null));
    // The background branch must never store: the picker persists the file
    // itself, so a store_managed_asset call here would be the avatar flow
    // leaking into the background kind.
    throw new Error(`unexpected command in harness: ${command}`);
  };
  const discard = async (assetId: string) => {
    events.push(`discard:${assetId}`);
    discards.push(assetId);
  };

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
    (dataUrl: string) => Promise.resolve(`normalized(${dataUrl})`),
    () => Promise.resolve(0.5),
    discard,
    provisionalAssets,
    assetError,
    avatarAssetId,
    avatarPreviewUrl,
    avatarPreviewAvailable,
    appearanceSettings,
    { value: "desktop" },
    pickedImage,
    imagePreviewAvailable,
  );

  // The unmount callback is registered through a stub, then invoked directly.
  // The persisted-settings shape mirrors the real props: three appearance
  // profiles plus the avatar id, so the cleanup's cross-profile "still saved"
  // scan runs exactly as shipped.
  const registered: Array<() => void> = [];
  const props = {
    settings: {
      language: "zh-Hans",
      avatarAssetId: null,
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
    { setLanguage: (language: string) => events.push(`setLanguage:${language}`) },
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
    provisional: provisionalAssets,
    appearance: appearanceSettings,
    pickedImage,
    imagePreviewAvailable,
    assetError,
    /** Simulates a completed save persisting the given background id. */
    markSaved(assetId: string | null) {
      props.settings.appearanceProfiles.desktop.imageAssetId = assetId;
    },
  };
}

describe("E4.1 background provisional lifecycle (behavior harness)", () => {
  it("keeps the provisional tracking intact while a replacement picker is pending", async () => {
    // The picker's native dialog is awaited; nothing may be released until it
    // returns and a replacement is adopted.
    let release!: (value: unknown) => void;
    const pending = new Promise((resolve) => {
      release = resolve;
    });
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () => pending,
    });
    const flow = h.chooseAsset("background");
    expect(h.events).toContain("invoke:choose_local_asset");
    expect(h.discards, "nothing may be discarded while the picker is open").toEqual([]);
    expect(h.provisional.has("background-a")).toBe(true);
    release(null);
    await flow;
    expect(h.provisional.has("background-a")).toBe(true);
    expect(h.appearance.value.imageAssetId).toBe("background-a");
  });

  it("picker cancellation does not invalidate the current background draft", async () => {
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () => Promise.resolve(null),
    });
    await h.chooseAsset("background");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("background-a")).toBe(true);
    expect(h.appearance.value.imageAssetId).toBe("background-a");
    expect(h.appearance.value.backgroundType).toBe("image");
    expect(h.assetError.value).toBeNull();
  });

  it("a rejected pick (read/validation failure) does not invalidate the current background draft", async () => {
    // The background kind has no separate normalize/store step — the picker
    // persists the file itself — so a "read or store failure" here is the
    // choose_local_asset command rejecting (too large, unreadable, unsupported).
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () => Promise.reject(new Error("selected image is too large")),
    });
    await h.chooseAsset("background");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("background-a")).toBe(true);
    expect(h.appearance.value.imageAssetId).toBe("background-a");
    expect(h.assetError.value).toEqual({
      kind: "background",
      message: "label(Error: selected image is too large)",
    });
  });

  it("a malformed pick payload adopts nothing and releases nothing", async () => {
    // The payload guard is the background's equivalent of the avatar's store
    // guard: a response without a usable asset id must not disturb the
    // current draft or its provisional tracking.
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () => Promise.resolve({ assetId: null, dataUrl: "data:image/png;base64,BBBB" }),
    });
    await h.chooseAsset("background");
    expect(h.discards).toEqual([]);
    expect(h.provisional.has("background-a")).toBe(true);
    expect(h.appearance.value.imageAssetId).toBe("background-a");
    expect(h.pickedImage.value).toBeNull();
  });

  it("a successful replacement adopts the draft and preview, then releases the superseded provisional", async () => {
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () =>
        Promise.resolve({
          assetId: "background-b",
          dataUrl: "data:image/png;base64,BBBB",
          available: true,
        }),
    });
    await h.chooseAsset("background");
    expect(h.appearance.value.imageAssetId).toBe("background-b");
    expect(h.appearance.value.backgroundType).toBe("image");
    expect(h.appearance.value.sampledLuminance).toBe(0.5);
    expect(h.pickedImage.value).toEqual({ mode: "desktop", available: true });
    expect(h.imagePreviewAvailable.value).toBe(true);
    expect([...h.provisional]).toEqual(["background-b"]);
    expect(h.discards).toEqual(["background-a"]);
    // The adoption strictly precedes the release: the draft can never name an
    // asset whose file was already discarded.
    const adoptedAt = h.events.indexOf(`imageAssetId="background-b"`);
    const releasedAt = h.events.indexOf("discard:background-a");
    expect(adoptedAt).toBeGreaterThanOrEqual(0);
    expect(releasedAt).toBeGreaterThan(adoptedAt);
  });

  it("A→B→Cancel keeps B valid: the third pick releases nothing, A was released exactly once", async () => {
    let pickCount = 0;
    const outcomes = [
      { assetId: "background-b", dataUrl: "data:image/png;base64,BBBB", available: true },
      null,
    ];
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () => Promise.resolve(outcomes[pickCount++] ?? null),
    });
    await h.chooseAsset("background");
    expect(h.appearance.value.imageAssetId).toBe("background-b");
    expect(h.discards).toEqual(["background-a"]);
    await h.chooseAsset("background");
    expect(h.discards, "a cancelled pick must not release the adopted provisional").toEqual([
      "background-a",
    ]);
    expect(h.provisional.has("background-b")).toBe(true);
    expect(h.appearance.value.imageAssetId).toBe("background-b");
  });

  it("A→B→Save→reopen: B is protected by its persisted reference, A was released at adoption", async () => {
    const h = buildHarness({
      initialProvisional: ["background-a"],
      initialDraftBackground: "background-a",
      pick: () =>
        Promise.resolve({
          assetId: "background-b",
          dataUrl: "data:image/png;base64,BBBB",
          available: true,
        }),
    });
    await h.chooseAsset("background");
    // Save persists B; "reopen" is a fresh panel whose unmount cleanup must
    // never treat the persisted B as provisional.
    h.markSaved("background-b");
    h.runUnmount();
    expect(h.discards).toEqual(["background-a"]);
    expect(h.appearance.value.imageAssetId).toBe("background-b");
  });

  it("a previous background that is not provisional (already saved) is never released by the picker", async () => {
    const h = buildHarness({
      initialProvisional: [],
      initialDraftBackground: "background-saved",
      pick: () =>
        Promise.resolve({
          assetId: "background-b",
          dataUrl: "data:image/png;base64,BBBB",
          available: true,
        }),
    });
    await h.chooseAsset("background");
    expect(h.appearance.value.imageAssetId).toBe("background-b");
    expect(h.discards, "saved assets are the backend's cleanup_replaced_assets business").toEqual([]);
    expect([...h.provisional]).toEqual(["background-b"]);
  });

  it("unmount cleanup discards unsaved provisional background assets", () => {
    const h = buildHarness({ initialProvisional: ["background-p"], savedProfileImageId: null });
    h.runUnmount();
    expect(h.discards).toEqual(["background-p"]);
  });

  it("unmount cleanup never discards a background the persisted settings still reference", () => {
    const h = buildHarness({
      initialProvisional: ["background-p"],
      savedProfileImageId: "background-p",
      languageSaved: true,
    });
    h.runUnmount();
    expect(h.discards).toEqual([]);
  });
});
