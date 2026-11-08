<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  APPEARANCE_MODES,
  DEFAULT_APPEARANCE,
  normalizeAvatarImage,
  sampleImageLuminance,
} from "../appearance";
import { normalizeLanguage, useI18n, type Language } from "../i18n";
import { moveLink, newLinkId } from "../modules/shortcuts/links";
import {
  DEFAULT_MODULE_VISIBILITY,
  resolveModuleVisibility,
  type ModuleId,
  type ModuleVisibility,
} from "../modules/registry";
import {
  SETTINGS_CATEGORIES,
  initialCategory,
  isSettingsCategoryId,
  nextTabCategory,
  type SettingsCategoryId,
} from "./settings/categories";
import {
  settingsDraftIsDirty,
  settingsSnapshot,
  type SettingsDraftSnapshot,
} from "./settings/dirty";
import type {
  AppearanceProfiles,
  AppearanceSettings,
  AssetPayload,
  LocationCandidate,
  ProductSettings,
  ProductWindowMode,
  QuickLink,
  TemperatureUnit,
  WeatherViewState,
} from "../types";
import AboutPanel from "./settings/AboutPanel.vue";
import GeneralPanel from "./settings/GeneralPanel.vue";
import ModulesPanel from "./settings/ModulesPanel.vue";
import WeatherPanel from "./settings/WeatherPanel.vue";

const props = defineProps<{
  settings: ProductSettings;
  databasePath: string;
  weather: WeatherViewState;
  weatherMessage: string;
  focusWeather: boolean;
  focusGithub: boolean;
  imageAvailable: boolean;
  wallpaperAvailable: boolean;
  avatarUrl: string;
  avatarAvailable: boolean;
  /**
   * The parent's reply to the last save attempt: empty while idle or after a
   * successful save, the failure text after a failed one. Rendered in this
   * panel's footer — the App-level error banner sits underneath this fixed
   * overlay and can never be seen while Settings is open.
   */
  saveError: string;
}>();
const emit = defineEmits<{
  close: [];
  refreshWeather: [];
  save: [patch: {
    dayRollover: string;
    weatherLocationLabel: string;
    weatherLatitude: number | null;
    weatherLongitude: number | null;
    weatherTimezone: string;
    weatherCountry: string;
    weatherAdmin1: string;
    temperatureUnit: TemperatureUnit;
    language: Language;
    appearance: string;
    appearanceProfiles: AppearanceProfiles;
    displayName: string;
    avatarAssetId: string | null;
    quickLinks: QuickLink[];
    moduleVisibility: ModuleVisibility;
    githubUsername: string;
  }];
}>();

const i18n = useI18n();
const { t } = i18n;

/**
 * True once this panel has handed a language choice to the parent's Save path.
 *
 * Language previews immediately (below), so a preview that is not saved has to
 * be rolled back when the panel goes away — but only for a save that was never
 * attempted. A save that failed leaves this flag `true`, which keeps the preview
 * on screen: the parent shows the error, and silently snapping the language back
 * would hide the fact that the click did something.
 */
let languageSaved = false;

const dayRollover = ref(props.settings.dayRollover);
/**
 * GitHub username draft (v1.3.0-G1). The Settings-minimal configuration the
 * GitHub module needs: one optional field, empty = unconfigured. The inline
 * error and the row's deep-link focus live in `ModulesPanel`, which reports
 * edits back through `update-github-username`; the backend re-validates
 * inside its transaction and remains authoritative.
 */
const githubUsername = ref(props.settings.githubUsername);
const weatherLocationLabel = ref(props.settings.weatherLocationLabel);
const weatherLatitude = ref(props.settings.weatherLatitude);
const weatherLongitude = ref(props.settings.weatherLongitude);
const weatherTimezone = ref(props.settings.weatherTimezone);
const weatherCountry = ref(props.settings.weatherCountry);
const weatherAdmin1 = ref(props.settings.weatherAdmin1);
const temperatureUnit = ref<TemperatureUnit>(props.settings.temperatureUnit);
const appearance = ref(props.settings.appearance);
/**
 * Appearance drafts, one per window mode.
 *
 * The panel edits a draft of each profile, and Save writes all three back. The
 * mode selector below is *not* persisted and never changes the real window mode:
 * it only chooses which profile the shared Appearance controls edit, so a
 * Desktop appearance can be prepared while the app runs as Sidebar.
 */
const appearanceProfiles = ref<AppearanceProfiles>(cloneProfiles(props.settings.appearanceProfiles));
/** The profile being edited. Opens on the mode the product is actually in. */
const appearanceMode = ref<ProductWindowMode>(props.settings.mode);
const appearanceSettings = computed<AppearanceSettings>(
  () => appearanceProfiles.value[appearanceMode.value],
);
const displayName = ref(props.settings.displayName);
const avatarAssetId = ref(props.settings.avatarAssetId);
const avatarPreviewUrl = ref(props.avatarUrl);
const avatarPreviewAvailable = ref(props.avatarAvailable);
const imagePreviewAvailable = ref(props.imageAvailable);
/**
 * Background image the user picked in this panel session, and for which profile.
 *
 * `imageAvailable` describes the profile the app has loaded — the current mode's.
 * A pick made here is known exactly, and for any other profile nothing is
 * claimed: see `editedImageUnavailable`.
 */
const pickedImage = ref<{ mode: ProductWindowMode; available: boolean } | null>(null);
const editedImageUnavailable = computed(() => {
  if (!appearanceSettings.value.imageAssetId) return false;
  if (pickedImage.value?.mode === appearanceMode.value) return !pickedImage.value.available;
  return appearanceMode.value === props.settings.mode && !imagePreviewAvailable.value;
});
/**
 * Failure of the last image pick, and which control produced it.
 *
 * The message is rendered inside that control's own row rather than at the bottom
 * of the panel: the picker is a native dialog that can legitimately refuse a file,
 * and a rejection the user cannot see is indistinguishable from "nothing
 * happened". `null` means no failure to show.
 */
const assetError = ref<{ kind: "avatar" | "background"; message: string } | null>(null);
const provisionalAssets = new Set<string>();

/**
 * Turns a native image failure into localized copy.
 *
 * The backend speaks a small, pinned vocabulary (see `appearance.rs`); anything
 * unrecognised becomes the "could not be loaded" message rather than leaking a raw
 * English string into a localized UI.
 */
function imageErrorLabel(reason: unknown): string {
  const text = String(reason);
  if (text.includes("too large")) return t("settings.error.imageTooLarge");
  if (text.includes("unsupported")) return t("settings.error.imageUnsupported");
  if (text.includes("could not be read")) return t("settings.error.imageUnreadable");
  return t("settings.error.imageUnreadable");
}

/**
 * Quick Links draft.
 *
 * This panel owns the whole list while it is open: the ModulesPanel editor
 * reports add, edit, delete and reorder back through events, the handlers
 * below mutate this copy, and Save writes the finished list in one patch.
 * That is why ids are assigned here — the list has to stay addressable across
 * a reorder before anything is persisted.
 */
const quickLinks = ref<QuickLink[]>(props.settings.quickLinks.map((link) => ({ ...link })));

/**
 * Module visibility draft.
 *
 * Initialized from the authoritative settings exactly like every other draft
 * on this panel: toggles and the reset below change this copy only, Save
 * emits it, and closing without Save discards it. The registry is the single
 * source of the ids and labels; the resolved shape guarantees exactly the
 * five known keys even if the stored document arrived in an unexpected form.
 */
const moduleVisibilityDraft = ref<ModuleVisibility>(
  resolveModuleVisibility(props.settings.moduleVisibility),
);

function toggleModule(id: ModuleId) {
  moduleVisibilityDraft.value = {
    ...moduleVisibilityDraft.value,
    [id]: !moduleVisibilityDraft.value[id],
  };
}

function resetModules() {
  moduleVisibilityDraft.value = { ...DEFAULT_MODULE_VISIBILITY };
}

/**
 * --- Modules draft mutations (S1-E3) ----------------------------------------
 *
 * The Modules category's controls live in `ModulesPanel`, but the persisted
 * fields are this panel's draft: the child owns the transient quick-link
 * editor and reports each change back, and these handlers write the username
 * and the link list exactly as the inline controls always did (the toggles
 * above stay the toggle's handlers). The dirty snapshot and Save read the
 * same refs, so the ownership split cannot fork the draft. New link ids are
 * still assigned here — the list has to stay addressable across a reorder
 * before anything is persisted.
 */
function updateGithubUsername(value: string) {
  githubUsername.value = value;
}

function addQuickLink(draft: { name: string; url: string }) {
  quickLinks.value.push({ id: newLinkId(), name: draft.name, url: draft.url });
}

function updateQuickLink(link: { id: string; name: string; url: string }) {
  const existing = quickLinks.value.find((item) => item.id === link.id);
  if (existing) {
    existing.name = link.name;
    existing.url = link.url;
  }
}

function deleteQuickLink(id: string) {
  quickLinks.value = quickLinks.value.filter((link) => link.id !== id);
}

function moveQuickLink(move: { id: string; delta: -1 | 1 }) {
  moveLink(quickLinks.value, move.id, move.delta);
}

/**
 * --- Category navigation (S1-B) -------------------------------------------
 *
 * One `activeCategory` drives both responsive navigation modes: the wide
 * horizontal tablist and the narrow category `<select>`. Which of the two is
 * *rendered* is a pure CSS decision (the same fixed panel spans the whole
 * window, so the viewport media query is the panel's width); the state and
 * the drafts are shared, so crossing the breakpoint cannot lose anything.
 */
const activeCategory = ref<SettingsCategoryId>(
  initialCategory(props.focusWeather, props.focusGithub),
);
const settingsNav = ref<HTMLElement | null>(null);
const settingsContent = ref<HTMLElement | null>(null);

function selectCategory(id: SettingsCategoryId) {
  activeCategory.value = id;
}

function onSelectCategory(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (isSettingsCategoryId(value)) activeCategory.value = value;
}

/** Changing category begins that category's content at the top. */
watch(activeCategory, () => {
  void nextTick(() => {
    settingsContent.value?.scrollTo({ top: 0 });
  });
});

/**
 * The tablist's arrow-key model (WAI-ARIA tabs): Left/Right wrap, Home/End
 * jump, and every other key is left alone. The reducer is pure —
 * `nextTabCategory` in `settings/categories.ts` — and unit-tested there.
 */
function onTablistKeydown(event: KeyboardEvent) {
  const next = nextTabCategory(activeCategory.value, event.key);
  if (!next) return;
  event.preventDefault();
  activeCategory.value = next;
  void nextTick(() => {
    settingsNav.value?.querySelector<HTMLButtonElement>(`#settings-tab-${next}`)?.focus();
  });
}

/**
 * --- Unsaved-change detection ----------------------------------------------
 *
 * `dirtyDraft` mirrors the Save payload exactly, so "dirty" and "would write"
 * are the same question by construction. UI-only state — the active category,
 * which appearance profile is being edited, weather search results, update
 * phases — is not part of the snapshot and never makes the panel dirty.
 */
const dirtyDraft = computed<SettingsDraftSnapshot>(() => ({
  dayRollover: dayRollover.value,
  weatherLocationLabel: weatherLocationLabel.value,
  weatherLatitude: weatherLatitude.value,
  weatherLongitude: weatherLongitude.value,
  weatherTimezone: weatherTimezone.value,
  weatherCountry: weatherCountry.value,
  weatherAdmin1: weatherAdmin1.value,
  temperatureUnit: temperatureUnit.value,
  language: language.value,
  appearance: appearance.value,
  appearanceProfiles: appearanceProfiles.value,
  displayName: displayName.value,
  avatarAssetId: avatarAssetId.value,
  quickLinks: quickLinks.value,
  moduleVisibility: moduleVisibilityDraft.value,
  githubUsername: githubUsername.value,
}));
const isDirty = computed(() =>
  settingsDraftIsDirty(dirtyDraft.value, settingsSnapshot(props.settings)),
);

/**
 * Language draft.
 *
 * Changing the selection previews immediately (the app-level i18n instance is
 * updated here, not on save) because a language picker that does not show its
 * own effect is unusable on a panel this long. The persisted value is still
 * written by Save, together with every other setting on this panel, and an
 * unsaved preview is reverted in `onBeforeUnmount`.
 */
const language = ref<Language>(normalizeLanguage(props.settings.language));

function selectLanguage(value: Language) {
  language.value = value;
  i18n.setLanguage(value);
}

/**
 * --- General draft mutations (S1-E4) ----------------------------------------
 *
 * The General category's controls live in `GeneralPanel`, but the persisted
 * fields are this panel's draft: the child reports each change back through
 * events, and these handlers write the display name and the rollover time
 * exactly as the inline controls always did. `selectLanguage` above stays the
 * language mutation too — the preview side effect (`setLanguage` on the app's
 * one i18n instance) and the `onBeforeUnmount` rollback read the same flag
 * and the same refs as before, so the child's event cannot fork them. The
 * avatar actions route to `chooseAsset('avatar')`/`removeAvatar` below, whose
 * native commands and provisional-asset lifecycle were never inline template
 * state.
 */
function updateDisplayName(value: string) {
  displayName.value = value;
}

function updateDayRollover(value: string) {
  dayRollover.value = value;
}

/**
 * --- Save -------------------------------------------------------------------
 *
 * One save in flight at a time: the click sets `savePending`, which disables
 * the button, and the parent's failure reply (the `saveError` prop) re-arms
 * it. A successful save closes the panel, so a stuck pending state cannot
 * outlive a real save. The failure text itself is rendered in the footer,
 * above this fixed overlay, where the user can actually see it.
 */
const savePending = ref(false);

function save() {
  if (savePending.value) return;
  savePending.value = true;
  languageSaved = true;
  emit("save", {
    dayRollover: dayRollover.value,
    weatherLocationLabel: weatherLocationLabel.value,
    weatherLatitude: weatherLatitude.value,
    weatherLongitude: weatherLongitude.value,
    weatherTimezone: weatherTimezone.value,
    weatherCountry: weatherCountry.value,
    weatherAdmin1: weatherAdmin1.value,
    temperatureUnit: temperatureUnit.value,
    language: language.value,
    appearance: appearance.value,
    appearanceProfiles: cloneProfiles(appearanceProfiles.value),
    displayName: displayName.value,
    avatarAssetId: avatarAssetId.value,
    quickLinks: quickLinks.value.map((link) => ({ ...link })),
    // The draft, not the stored value: this is the one place module
    // preferences change. The backend preserves unknown future ids the
    // payload cannot express (B2's replace semantics).
    moduleVisibility: { ...moduleVisibilityDraft.value },
    githubUsername: githubUsername.value,
    // No layout editor exists yet (a later C slice), and unrelated saves do
    // not send the layout at all (the D1 concurrency gate): absent means "no
    // change" on the backend, which is a strictly stronger preservation
    // guarantee than riding the document through — a payload echo read before
    // a collapse response landed would name every entry with pre-toggle
    // values and overwrite the just-committed collapse on merge. When a
    // layout editor arrives it sends a fresh raw-document patch instead.
  });
}

watch(
  () => props.saveError,
  (error) => {
    if (error) savePending.value = false;
  },
);

async function discard(assetId: string) {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    await invoke("discard_managed_asset", { assetId });
  } catch {
    // A saved asset is intentionally protected by the native command.
  }
}

/**
 * Picks an image for the profile avatar or the background material.
 *
 * The two kinds deliberately differ:
 *
 * - **Avatar** — the picked file is only *read* (`persist: false`), normalized in
 *   the WebView to {@link AVATAR_EDGE_PX} square PNG, and stored through
 *   `store_managed_asset`. The original, which may be a multi-megabyte photo, is
 *   never written to the profile, and the preview shows exactly the normalized
 *   bytes that will be persisted.
 * - **Background** — the picked file is still stored at full resolution, because
 *   it is scaled to a whole window.
 *
 * A refusal (too large to read, unsupported format, undecodable image) is reported
 * in the row that owns the control. For the avatar, a replacement is adopted
 * before the provisional it supersedes is released (R-E4-01), so a cancelled or
 * failed pick always leaves the current avatar usable and tracked.
 */
async function chooseAsset(kind: "background" | "avatar") {
  assetError.value = null;
  if (!("__TAURI_INTERNALS__" in window)) {
    assetError.value = { kind, message: t("settings.error.localFilePicker") };
    return;
  }
  try {
    if (kind === "avatar") {
      const picked = await invoke<AssetPayload | null>("choose_local_asset", { kind, persist: false });
      if (!picked?.dataUrl) return;
      const normalized = await normalizeAvatarImage(picked.dataUrl);
      const stored = await invoke<AssetPayload>("store_managed_asset", { kind, dataUrl: normalized });
      if (!stored.assetId || !stored.dataUrl) {
        assetError.value = { kind, message: t("settings.error.imageUnreadable") };
        return;
      }
      // Adopt the replacement before releasing the provisional it supersedes
      // (R-E4-01): a cancelled, failed or rejected pick returns above with the
      // current avatar — and its provisional tracking — untouched, so Save can
      // never persist a reference whose file was already discarded. `previous`
      // is snapshotted from the draft here, after the picker returns, because
      // that is exactly the value this adoption replaces.
      const previous = avatarAssetId.value;
      provisionalAssets.add(stored.assetId);
      avatarAssetId.value = stored.assetId;
      avatarPreviewUrl.value = stored.dataUrl;
      avatarPreviewAvailable.value = true;
      if (previous && previous !== stored.assetId && provisionalAssets.has(previous)) {
        provisionalAssets.delete(previous);
        void discard(previous);
      }
      return;
    }

    // The background keeps its established release-early sequence (R-E4-01
    // covers the avatar only): the superseded provisional is released before
    // the picker opens, exactly as before the fix.
    const previous = appearanceSettings.value.imageAssetId;
    if (previous && provisionalAssets.has(previous)) {
      provisionalAssets.delete(previous);
      void discard(previous);
    }
    const selected = await invoke<AssetPayload | null>("choose_local_asset", { kind });
    if (!selected?.assetId || !selected.dataUrl) return;
    provisionalAssets.add(selected.assetId);
    appearanceSettings.value.imageAssetId = selected.assetId;
    appearanceSettings.value.backgroundType = "image";
    appearanceSettings.value.sampledLuminance = await sampleImageLuminance(selected.dataUrl);
    pickedImage.value = { mode: appearanceMode.value, available: selected.available };
    imagePreviewAvailable.value = selected.available;
  } catch (reason) {
    assetError.value = { kind, message: imageErrorLabel(reason) };
  }
}

function removeAvatar() {
  avatarAssetId.value = null;
  avatarPreviewUrl.value = "";
  avatarPreviewAvailable.value = false;
}

/**
 * Resets only the profile being edited.
 *
 * The other two modes keep their appearance: a single click must never discard
 * an appearance the user built for a different window mode.
 */
function resetAppearance() {
  appearanceProfiles.value[appearanceMode.value] = { ...DEFAULT_APPEARANCE };
  pickedImage.value = null;
  imagePreviewAvailable.value = false;
}

/**
 * --- Weather draft mutations -------------------------------------------------
 *
 * The Weather category's controls live in `WeatherPanel`, but the persisted
 * fields are this panel's draft: the child owns the transient search UI and
 * reports each choice back, and these handlers write the six location fields
 * and the unit exactly as the inline controls always did. The dirty snapshot
 * and Save read the same refs, so the ownership split cannot fork the draft.
 */
function selectLocation(candidate: LocationCandidate) {
  weatherLocationLabel.value = candidate.label;
  weatherLatitude.value = candidate.latitude;
  weatherLongitude.value = candidate.longitude;
  weatherTimezone.value = candidate.timezone;
  weatherCountry.value = candidate.country;
  weatherAdmin1.value = candidate.admin1;
}

function setTemperatureUnit(unit: TemperatureUnit) {
  temperatureUnit.value = unit;
}

// Deep links (the GitHub card's setup entry, the weather card's "Set
// location") open this panel on their own category — `initialCategory` chose
// it before first render, so the target is visible when the child mounts. The
// focus behaviors themselves live in the child that owns the row they scroll
// to: WeatherPanel for the weather link (S1-E2), ModulesPanel for the GitHub
// link (S1-E3).

onBeforeUnmount(() => {
  // Revert an unsaved language preview.
  //
  // `props.settings` still holds the persisted value here: the parent closes this
  // panel before it replaces the state with the command's response, so a saved
  // change is never rolled back to the value it just replaced.
  if (!languageSaved) i18n.setLanguage(normalizeLanguage(props.settings.language));

  for (const assetId of provisionalAssets) {
    const saved =
      props.settings.avatarAssetId === assetId ||
      Object.values(props.settings.appearanceProfiles).some(
        (profile) => profile.imageAssetId === assetId,
      );
    if (!saved) void discard(assetId);
  }
});

/** Detached copies, so an unsaved draft can never write through to the parent. */
function cloneProfiles(profiles: AppearanceProfiles): AppearanceProfiles {
  return {
    sidebar: { ...profiles.sidebar },
    floating: { ...profiles.floating },
    desktop: { ...profiles.desktop },
  };
}
</script>

<template>
  <aside class="settings-panel" :aria-label="t('settings.panelAriaLabel')">
    <header class="settings-header">
      <h2>{{ t("app.name") }}</h2>
      <button
        type="button"
        :aria-label="t('settings.closeAriaLabel')"
        @click="$emit('close')"
      >{{ t("settings.close") }}</button>
    </header>

    <!--
      One shared `activeCategory`, two renderings of the same navigation
      (decision D5): the wide tablist below and, in windows too narrow for
      five tabs, the category select. CSS decides which one renders; both
      bind the same state, so crossing the breakpoint preserves the active
      category and every draft.
    -->
    <nav
      ref="settingsNav"
      class="settings-nav settings-nav--tabs"
      role="tablist"
      :aria-label="t('settings.category.label')"
      @keydown="onTablistKeydown"
    >
      <button
        v-for="category in SETTINGS_CATEGORIES"
        :id="`settings-tab-${category.id}`"
        :key="category.id"
        type="button"
        role="tab"
        :aria-selected="activeCategory === category.id"
        :aria-controls="`settings-panel-${category.id}`"
        :class="{ active: activeCategory === category.id }"
        :tabindex="activeCategory === category.id ? 0 : -1"
        @click="selectCategory(category.id)"
      >{{ t(category.labelKey) }}</button>
    </nav>
    <div class="settings-nav settings-nav--select">
      <label class="settings-category-field">
        <span>{{ t("settings.category.label") }}</span>
        <select :value="activeCategory" @change="onSelectCategory">
          <option
            v-for="category in SETTINGS_CATEGORIES"
            :key="category.id"
            :value="category.id"
          >{{ t(category.labelKey) }}</option>
        </select>
      </label>
    </div>

    <div ref="settingsContent" class="settings-content">
      <!-- ======================= Appearance / 外观 ======================= -->
      <section
        v-show="activeCategory === 'appearance'"
        id="settings-panel-appearance"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-appearance"
      >
        <!--
          Which profile the controls below edit. This is Settings UI state only:
          it never changes the window mode the product runs in, and it is not
          persisted, so it reopens on the mode actually in use. The mode names
          are the same ones the window menu uses.
        -->
        <div class="settings-row appearance-background-row">
          <span>
            <strong>{{ t("settings.appearanceFor") }}</strong>
            <small>{{ t("settings.appearanceForHint") }}</small>
          </span>
          <div class="appearance-options" role="group" :aria-label="t('settings.appearanceForAriaLabel')">
            <button
              v-for="option in APPEARANCE_MODES"
              :key="option"
              type="button"
              :class="{ active: appearanceMode === option }"
              :aria-pressed="appearanceMode === option"
              @click="appearanceMode = option"
            >
              {{ t(`menu.mode.${option}`) }}
            </button>
          </div>
        </div>
        <div class="settings-row appearance-background-row">
          <span><strong>{{ t("settings.background") }}</strong><small>{{ t("settings.backgroundHint") }}</small></span>
          <div class="appearance-options" role="group" :aria-label="t('settings.backgroundAriaLabel')">
            <button
              v-for="option in ['glass', 'solid', 'gradient', 'image', 'wallpaper'] as const"
              :key="option"
              type="button"
              :class="{ active: appearanceSettings.backgroundType === option }"
              @click="appearanceSettings.backgroundType = option"
            >
              {{ t(`settings.backgroundType.${option}`) }}
            </button>
          </div>
        </div>

        <template v-if="appearanceSettings.backgroundType === 'glass'">
          <label class="settings-row">
            <span><strong>{{ t("settings.tint") }}</strong><small>{{ t("settings.tintHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.glassTintColor" type="color" /><code>{{ appearanceSettings.glassTintColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.tintOpacity") }}</strong><small>{{ t("settings.tintOpacityHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.glassTintOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.glassTintOpacity * 100) }}%</output></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.blur") }}</strong><small>{{ t("settings.blurHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.blurPx" type="range" min="0" max="24" step="1" /><output>{{ appearanceSettings.blurPx }}px</output></div>
          </label>
        </template>

        <label v-if="appearanceSettings.backgroundType === 'solid'" class="settings-row">
          <span><strong>{{ t("settings.color") }}</strong><small>{{ t("settings.colorHint") }}</small></span>
          <div class="color-control"><input v-model="appearanceSettings.solidColor" type="color" /><code>{{ appearanceSettings.solidColor }}</code></div>
        </label>

        <template v-if="appearanceSettings.backgroundType === 'gradient'">
          <label class="settings-row">
            <span><strong>{{ t("settings.gradientStart") }}</strong><small>{{ t("settings.gradientStartHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientStartColor" type="color" /><code>{{ appearanceSettings.gradientStartColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.gradientEnd") }}</strong><small>{{ t("settings.gradientEndHint") }}</small></span>
            <div class="color-control"><input v-model="appearanceSettings.gradientEndColor" type="color" /><code>{{ appearanceSettings.gradientEndColor }}</code></div>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.angle") }}</strong><small>{{ t("settings.angleHint") }}</small></span>
            <div class="range-control"><input v-model.number="appearanceSettings.gradientAngle" type="range" min="0" max="360" step="1" /><output>{{ appearanceSettings.gradientAngle }}°</output></div>
          </label>
        </template>

        <template v-if="appearanceSettings.backgroundType === 'image'">
          <div class="settings-row">
            <span><strong>{{ t("settings.customImage") }}</strong><small>{{ t("settings.customImageHint") }}</small></span>
            <div class="asset-actions">
              <button type="button" @click="chooseAsset('background')">{{ t("settings.chooseImage") }}</button>
              <small v-if="editedImageUnavailable" class="asset-unavailable">{{ t("settings.imageUnavailable") }}</small>
              <small v-if="assetError?.kind === 'background'" class="settings-error asset-error" role="status">{{ assetError.message }}</small>
            </div>
          </div>
          <label class="settings-row">
            <span><strong>{{ t("settings.fit") }}</strong><small>{{ t("settings.fitHint") }}</small></span>
            <select v-model="appearanceSettings.imageFit"><option value="cover">{{ t("settings.fit.cover") }}</option><option value="contain">{{ t("settings.fit.contain") }}</option><option value="stretch">{{ t("settings.fit.stretch") }}</option></select>
          </label>
          <label class="settings-row">
            <span><strong>{{ t("settings.position") }}</strong><small>{{ t("settings.positionHint") }}</small></span>
            <select v-model="appearanceSettings.imagePosition"><option value="center">{{ t("settings.position.center") }}</option><option value="top">{{ t("settings.position.top") }}</option><option value="bottom">{{ t("settings.position.bottom") }}</option></select>
          </label>
        </template>

        <div v-if="appearanceSettings.backgroundType === 'wallpaper'" class="settings-row">
          <span><strong>{{ t("settings.desktopWallpaper") }}</strong><small>{{ t("settings.desktopWallpaperHint") }}</small></span>
          <span class="asset-status">{{ wallpaperAvailable ? t("settings.wallpaperAvailable") : t("settings.wallpaperUnavailable") }}</span>
        </div>

        <label class="settings-row">
          <span><strong>{{ t("settings.backgroundOpacity") }}</strong><small>{{ t("settings.backgroundOpacityHint") }}</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.backgroundOpacity" type="range" min="0" max="1" step="0.01" /><output>{{ Math.round(appearanceSettings.backgroundOpacity * 100) }}%</output></div>
        </label>
        <label class="settings-row">
          <span><strong>{{ t("settings.overlay") }}</strong><small>{{ t("settings.overlayHint") }}</small></span>
          <div class="range-control"><input v-model.number="appearanceSettings.overlayStrength" type="range" min="0" max="0.72" step="0.01" /><output>{{ Math.round(appearanceSettings.overlayStrength * 100) }}%</output></div>
        </label>
        <div class="settings-row appearance-background-row">
          <span><strong>{{ t("settings.textContrast") }}</strong><small>{{ t("settings.textContrastHint") }}</small></span>
          <div class="appearance-options" role="group" :aria-label="t('settings.textContrastAriaLabel')">
            <button
              v-for="option in ['auto', 'light', 'dark', 'custom'] as const"
              :key="option"
              type="button"
              :class="{ active: appearanceSettings.textContrast === option }"
              @click="appearanceSettings.textContrast = option"
            >
              {{ t(`settings.textContrast.${option}`) }}
            </button>
          </div>
        </div>
        <label v-if="appearanceSettings.textContrast === 'custom'" class="settings-row">
          <span><strong>{{ t("settings.customTextColor") }}</strong><small>{{ t("settings.customTextColorHint") }}</small></span>
          <div class="color-control"><input v-model="appearanceSettings.customTextColor" type="color" /><code>{{ appearanceSettings.customTextColor }}</code></div>
        </label>
        <button type="button" class="reset-appearance" @click="resetAppearance">{{ t("settings.resetAppearance") }}</button>
      </section>

      <!-- ======================== General / 常规 ======================== -->
      <section
        v-show="activeCategory === 'general'"
        id="settings-panel-general"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-general"
      >
        <!--
          The General category's controls (S1-E4). The child owns the
          presentation; the persisted fields stay drafts of this panel — the
          child reports each change back through events, and the dirty
          snapshot and Save below read the same refs they always read. The
          language preview and the avatar picker's native side effects stay
          here, and the avatar row's error renders through the avatarError
          slot because `assetError` is this panel's state (shared with the
          background picker, which clears it).
        -->
        <GeneralPanel
          :language="language"
          :display-name="displayName"
          :day-rollover="dayRollover"
          :avatar-asset-id="avatarAssetId"
          :avatar-preview-url="avatarPreviewUrl"
          :avatar-preview-available="avatarPreviewAvailable"
          @select-language="selectLanguage"
          @update-display-name="updateDisplayName"
          @update-day-rollover="updateDayRollover"
          @choose-avatar="chooseAsset('avatar')"
          @remove-avatar="removeAvatar"
        >
          <template #avatarError>
            <small v-if="assetError?.kind === 'avatar'" class="settings-error asset-error" role="status">{{ assetError.message }}</small>
          </template>
        </GeneralPanel>
      </section>

      <!-- ======================== Modules / 模块 ======================== -->
      <section
        v-show="activeCategory === 'modules'"
        id="settings-panel-modules"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-modules"
      >
        <!--
          The Modules category's controls (S1-E3). The child owns the transient
          quick-link editor and the deep-link focus; the persisted fields stay
          drafts of this panel — the child reports each change back through
          events, and the dirty snapshot and Save below read the same refs they
          always read.
        -->
        <ModulesPanel
          :module-visibility="moduleVisibilityDraft"
          :github-username="githubUsername"
          :quick-links="quickLinks"
          :focus-github="focusGithub"
          @toggle-module="toggleModule"
          @reset-modules="resetModules"
          @update-github-username="updateGithubUsername"
          @add-link="addQuickLink"
          @update-link="updateQuickLink"
          @delete-link="deleteQuickLink"
          @move-link="moveQuickLink"
        />
      </section>

      <!-- ======================== Weather / 天气 ======================== -->
      <section
        v-show="activeCategory === 'weather'"
        id="settings-panel-weather"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-weather"
      >
        <!--
          The Weather category's controls (S1-E2). The child owns the transient
          search UI, the attribution action and the deep-link focus; the
          persisted fields stay drafts of this panel — the child reports each
          choice back through events, and the dirty snapshot and Save below
          read the same refs they always read.
        -->
        <WeatherPanel
          :weather-location-label="weatherLocationLabel"
          :weather-timezone="weatherTimezone"
          :weather-country="weatherCountry"
          :weather-admin1="weatherAdmin1"
          :temperature-unit="temperatureUnit"
          :weather="weather"
          :weather-message="weatherMessage"
          :focus-weather="focusWeather"
          @select-location="selectLocation"
          @update-temperature-unit="setTemperatureUnit"
          @refresh-weather="$emit('refreshWeather')"
        />
      </section>

      <!-- ========================= About / 关于 ========================= -->
      <section
        v-show="activeCategory === 'about'"
        id="settings-panel-about"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-about"
      >
        <AboutPanel :database-path="databasePath" />
      </section>
    </div>

    <footer class="settings-footer">
      <!--
        A failed save keeps the panel open and reports here, above the
        overlay: the App-level error banner renders underneath this fixed
        panel and can never be seen while Settings is open (S1-A finding
        F-1). The draft is untouched on failure, the language preview stays
        (its established failed-save rule), and the Save button re-arms via
        the same prop.
      -->
      <p v-if="saveError" class="settings-error settings-save-error" role="alert">
        <strong>{{ t("settings.saveFailed") }}</strong>
        {{ saveError }}
      </p>
      <div class="settings-footer-row">
        <p v-if="isDirty" class="settings-dirty" role="status">{{ t("settings.unsavedChanges") }}</p>
        <span class="settings-footer-spacer" aria-hidden="true"></span>
        <button type="button" class="settings-cancel" @click="$emit('close')">{{ t("settings.cancel") }}</button>
        <button type="button" class="save-settings" :disabled="savePending" @click="save">
          {{ savePending ? t("settings.saving") : t("settings.save") }}
        </button>
      </div>
    </footer>
  </aside>
</template>
