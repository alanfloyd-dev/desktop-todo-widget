<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  APPEARANCE_MODES,
  DEFAULT_APPEARANCE,
  normalizeAvatarImage,
  profileInitials,
  sampleImageLuminance,
} from "../appearance";
import { languageOptionLabel, normalizeLanguage, useI18n, type Language } from "../i18n";
import { moveLink, newLinkId, quickLinkUrlErrorKey } from "../modules/shortcuts/links";
import {
  DEFAULT_MODULE_VISIBILITY,
  MODULE_REGISTRY,
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
 * GitHub module needs: one optional field, empty = unconfigured. Validation
 * mirrors the backend's grammar exactly (1–39 characters, letters/digits/
 * hyphens, no leading or trailing hyphen) so the inline error and the
 * backend's refusal can never disagree; the backend re-validates inside its
 * transaction and remains authoritative.
 */
const githubUsername = ref(props.settings.githubUsername);
const githubUsernameError = computed(() => {
  const value = githubUsername.value.trim();
  if (!value) return "";
  if (
    value.length > 39 ||
    value.startsWith("-") ||
    value.endsWith("-") ||
    /[^A-Za-z0-9-]/.test(value)
  ) {
    return t("github.error.invalidUsername");
  }
  return "";
});
const githubInput = ref<HTMLInputElement | null>(null);
/** The username row, for the GitHub card deep-link's scrollIntoView. */
const githubRow = ref<HTMLElement | null>(null);
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
 * The panel owns the whole list while it is open: add, edit, delete and reorder
 * all mutate this copy, and Save writes the finished list in one patch. That is
 * why ids are assigned here — the list has to stay addressable across a reorder
 * before anything is persisted.
 */
const quickLinks = ref<QuickLink[]>(props.settings.quickLinks.map((link) => ({ ...link })));
/** Id of the row being edited, or `null` when no editor is open. */
const editingLinkId = ref<string | null>(null);
const draftName = ref("");
const draftUrl = ref("");
const quickLinkError = ref("");

function resetEditor() {
  editingLinkId.value = null;
  draftName.value = "";
  draftUrl.value = "";
  quickLinkError.value = "";
}

function addLink() {
  resetEditor();
  editingLinkId.value = "";
}

function editLink(link: QuickLink) {
  editingLinkId.value = link.id;
  draftName.value = link.name;
  draftUrl.value = link.url;
  quickLinkError.value = "";
}

function commitEditor() {
  const name = draftName.value.trim();
  const url = draftUrl.value.trim();
  if (!name) {
    quickLinkError.value = t("quickLinks.error.nameRequired");
    return;
  }
  const urlErrorKey = quickLinkUrlErrorKey(url);
  if (urlErrorKey) {
    quickLinkError.value = t(urlErrorKey);
    return;
  }
  if (editingLinkId.value) {
    const existing = quickLinks.value.find((link) => link.id === editingLinkId.value);
    if (existing) {
      existing.name = name;
      existing.url = url;
    }
  } else {
    quickLinks.value.push({ id: newLinkId(), name, url });
  }
  resetEditor();
}

function deleteLink(id: string) {
  quickLinks.value = quickLinks.value.filter((link) => link.id !== id);
  if (editingLinkId.value === id) resetEditor();
}

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
 * The GitHub username row's hint (decision D2): the field is always rendered,
 * and while the GitHub module is switched off the hint says what enabling it
 * does instead of describing a card that is not on the dashboard.
 */
const githubUsernameHint = computed(() =>
  moduleVisibilityDraft.value.github
    ? t("settings.githubUsernameHint")
    : t("settings.githubUsernameHintDisabled"),
);

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
const effectiveLanguage = computed(() =>
  languageOptionLabel(i18n.locale.value, t),
);

function selectLanguage(value: Language) {
  language.value = value;
  i18n.setLanguage(value);
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
 * in the row that owns the control.
 */
async function chooseAsset(kind: "background" | "avatar") {
  assetError.value = null;
  if (!("__TAURI_INTERNALS__" in window)) {
    assetError.value = { kind, message: t("settings.error.localFilePicker") };
    return;
  }
  try {
    const previous = kind === "avatar"
      ? avatarAssetId.value
      : appearanceSettings.value.imageAssetId;
    if (previous && provisionalAssets.has(previous)) {
      provisionalAssets.delete(previous);
      void discard(previous);
    }

    if (kind === "avatar") {
      const picked = await invoke<AssetPayload | null>("choose_local_asset", { kind, persist: false });
      if (!picked?.dataUrl) return;
      const normalized = await normalizeAvatarImage(picked.dataUrl);
      const stored = await invoke<AssetPayload>("store_managed_asset", { kind, dataUrl: normalized });
      if (!stored.assetId || !stored.dataUrl) {
        assetError.value = { kind, message: t("settings.error.imageUnreadable") };
        return;
      }
      provisionalAssets.add(stored.assetId);
      avatarAssetId.value = stored.assetId;
      avatarPreviewUrl.value = stored.dataUrl;
      avatarPreviewAvailable.value = true;
      return;
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

onMounted(async () => {
  // Deep links (the GitHub card's setup entry) opened this panel on their own
  // category — `initialCategory` chose it before first render, so the target
  // is visible and `scrollIntoView` below never runs against a hidden panel.
  // The weather deep link's focus behavior lives in WeatherPanel, which owns
  // the section and input refs it scrolls to and focuses.
  if (props.focusGithub) {
    await nextTick();
    githubRow.value?.scrollIntoView({ block: "start" });
    githubInput.value?.focus();
  }
});

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
        <div class="settings-row">
          <span>
            <strong>{{ t("settings.language.label") }}</strong>
            <small>
              {{ t("settings.language.hint") }}<template v-if="language === 'system'"> · {{ t("settings.language.effective", { language: effectiveLanguage }) }}</template>
              · {{ t("settings.language.restartHint") }}
            </small>
          </span>
          <div class="appearance-options" role="group" :aria-label="t('settings.language.label')">
            <button
              v-for="option in (['system', 'en', 'zh-Hans'] as const)"
              :key="option"
              type="button"
              :class="{ active: language === option }"
              :aria-pressed="language === option"
              @click="selectLanguage(option)"
            >
              {{ languageOptionLabel(option, t) }}
            </button>
          </div>
        </div>
        <label class="settings-row">
          <span><strong>{{ t("settings.displayName") }}</strong><small>{{ t("settings.displayNameHint") }}</small></span>
          <input v-model="displayName" type="text" :placeholder="t('settings.displayNamePlaceholder')" />
        </label>
        <div class="settings-row avatar-settings-row">
          <span><strong>{{ t("settings.avatar") }}</strong><small>{{ t("settings.avatarHint") }}</small></span>
          <div class="avatar-settings-control">
            <span class="settings-avatar-preview" :aria-label="t('settings.avatarAriaLabel')">
              <img v-if="avatarPreviewAvailable" :src="avatarPreviewUrl" alt="" />
              <span v-else>{{ profileInitials(displayName) }}</span>
            </span>
            <button type="button" @click="chooseAsset('avatar')">{{ t("settings.chooseImage") }}</button>
            <button type="button" :disabled="!avatarAssetId" @click="removeAvatar">{{ t("settings.removeAvatar") }}</button>
            <!--
              The failure is shown here, under the button that caused it, instead of
              at the bottom of the panel: the picker is a native dialog whose
              rejections are otherwise invisible, and "the click did nothing" is the
              worst possible answer.
            -->
            <small v-if="assetError?.kind === 'avatar'" class="settings-error asset-error" role="status">{{ assetError.message }}</small>
          </div>
        </div>
        <label class="settings-row">
          <span><strong>{{ t("settings.dayRollover") }}</strong><small>{{ t("settings.dayRolloverHint") }}</small></span>
          <input v-model="dayRollover" type="time" />
        </label>
      </section>

      <!-- ======================== Modules / 模块 ======================== -->
      <section
        v-show="activeCategory === 'modules'"
        id="settings-panel-modules"
        class="settings-category-panel"
        role="tabpanel"
        aria-labelledby="settings-tab-modules"
      >
        <p class="settings-intro">{{ t("settings.modulesDescription") }}</p>
        <!--
          The module switches (decision D1): one row per registry module, a
          real `role="switch"` control each, and one localized line saying
          what the module does. They edit the draft only — the dashboard
          updates on Save, and closing without Save discards them. Ids and
          labels come from the module registry; GitHub ships disabled there
          (ADR-DG-001 Amendment ①) and the registry default is untouched.
        -->
        <ul class="module-switch-list">
          <li v-for="module in MODULE_REGISTRY" :key="module.id" class="module-switch-row">
            <span class="module-switch-text">
              <strong :id="`module-switch-label-${module.id}`">{{ t(module.labelKey) }}</strong>
              <small :id="`module-switch-desc-${module.id}`">{{ t(`settings.moduleDesc.${module.id}`) }}</small>
            </span>
            <button
              type="button"
              role="switch"
              :aria-checked="moduleVisibilityDraft[module.id]"
              :aria-labelledby="`module-switch-label-${module.id}`"
              :aria-describedby="`module-switch-desc-${module.id}`"
              class="module-switch"
              :class="{ on: moduleVisibilityDraft[module.id] }"
              @click="toggleModule(module.id)"
            ><span class="module-switch-knob" aria-hidden="true"></span></button>
          </li>
        </ul>
        <div class="settings-module-reset">
          <button type="button" @click="resetModules">{{ t("settings.modulesReset") }}</button>
        </div>

        <!--
          GitHub username (decision D2): always rendered and reachable, with a
          hint that explains the enabling step while the module is off. The
          inline error mirrors the backend's username grammar; the backend
          re-validates inside its transaction.
        -->
        <label ref="githubRow" class="settings-row">
          <span>
            <strong>{{ t("settings.githubUsername") }}</strong>
            <small>{{ githubUsernameHint }}</small>
          </span>
          <input
            ref="githubInput"
            v-model="githubUsername"
            type="text"
            :placeholder="t('settings.githubUsernamePlaceholder')"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <p v-if="githubUsernameError" class="settings-error" role="alert">{{ githubUsernameError }}</p>

        <div class="settings-section-heading"><strong>{{ t("quickLinks.heading") }}</strong></div>
        <section class="quick-links-settings" :aria-label="t('quickLinks.heading')">
          <p v-if="!quickLinks.length" class="quick-links-empty">{{ t("quickLinks.empty") }}</p>
          <ul v-else class="quick-links-rows">
            <li v-for="(link, index) in quickLinks" :key="link.id" class="quick-link-row">
              <span class="quick-link-fields">
                <strong>{{ link.name }}</strong>
                <small>{{ link.url }}</small>
              </span>
              <span class="quick-link-actions">
                <button
                  type="button"
                  :disabled="index === 0"
                  :title="t('quickLinks.moveUp')"
                  :aria-label="t('quickLinks.moveUpAriaLabel', { name: link.name })"
                  @click="moveLink(quickLinks, link.id, -1)"
                >↑</button>
                <button
                  type="button"
                  :disabled="index === quickLinks.length - 1"
                  :title="t('quickLinks.moveDown')"
                  :aria-label="t('quickLinks.moveDownAriaLabel', { name: link.name })"
                  @click="moveLink(quickLinks, link.id, 1)"
                >↓</button>
                <button
                  type="button"
                  :aria-label="t('quickLinks.editAriaLabel', { name: link.name })"
                  @click="editLink(link)"
                >{{ t("quickLinks.edit") }}</button>
                <button
                  type="button"
                  :aria-label="t('quickLinks.deleteAriaLabel', { name: link.name })"
                  @click="deleteLink(link.id)"
                >{{ t("quickLinks.delete") }}</button>
              </span>
            </li>
          </ul>

          <!--
            One inline editor for both Add and Edit: the same two fields and the same
            validation, so there is no second code path to keep in step.
          -->
          <div v-if="editingLinkId !== null" class="quick-link-editor">
            <label class="settings-row">
              <span><strong>{{ t("quickLinks.name") }}</strong></span>
              <input
                v-model="draftName"
                type="text"
                :placeholder="t('quickLinks.namePlaceholder')"
                @keydown.enter.prevent="commitEditor"
              />
            </label>
            <label class="settings-row">
              <span><strong>{{ t("quickLinks.url") }}</strong></span>
              <input
                v-model="draftUrl"
                type="url"
                :placeholder="t('quickLinks.urlPlaceholder')"
                @keydown.enter.prevent="commitEditor"
              />
            </label>
            <p v-if="quickLinkError" class="settings-error" role="alert">{{ quickLinkError }}</p>
            <div class="quick-link-editor-actions">
              <button type="button" @click="commitEditor">{{ t("quickLinks.save") }}</button>
              <button type="button" @click="resetEditor">{{ t("quickLinks.cancel") }}</button>
            </div>
          </div>
          <button v-else type="button" class="quick-link-add" @click="addLink">
            {{ t("quickLinks.add") }}
          </button>
        </section>
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
