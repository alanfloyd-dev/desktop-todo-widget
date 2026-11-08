<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { useI18n } from "../../i18n";
import { quickLinkUrlErrorKey } from "../../modules/shortcuts/links";
import { MODULE_REGISTRY, type ModuleId, type ModuleVisibility } from "../../modules/registry";
import type { QuickLink } from "../../types";

/**
 * The Modules category: the module visibility switches, the GitHub username
 * row and the Quick Links editor.
 *
 * It renders inside the `v-show` tabpanel `SettingsPanel` owns, so it mounts
 * and unmounts with the Settings panel rather than with the active category,
 * and every piece of state below lives for exactly one panel session. The
 * split with the parent follows the panel's ownership rule (the same one the
 * WeatherPanel extraction applies): everything persisted — the visibility
 * draft, the username and the Quick Links list — stays a SettingsPanel draft
 * (the dirty snapshot and Save read it there); this component owns only the
 * transient editor state and the presentation computeds, and reports every
 * persisted change back through events instead of touching the draft itself.
 * The template is a multi-root fragment on purpose: the moved markup stays
 * the tabpanel's direct children, so the rendered DOM — and with it every
 * descendant selector in styles.css — is identical to the inline layout this
 * extraction replaced.
 */
const props = defineProps<{
  /** The parent's module-visibility draft, rendered read-only here. */
  moduleVisibility: ModuleVisibility;
  /** The parent's GitHub username draft. */
  githubUsername: string;
  /** The parent's Quick Links draft list, rendered read-only here. */
  quickLinks: QuickLink[];
  /**
   * Deep link (the GitHub card's setup entry): the panel opened directly onto
   * this category, so the username field is the entry point and receives the
   * initial focus.
   */
  focusGithub: boolean;
}>();

const emit = defineEmits<{
  toggleModule: [id: ModuleId];
  resetModules: [];
  updateGithubUsername: [value: string];
  addLink: [draft: { name: string; url: string }];
  updateLink: [link: { id: string; name: string; url: string }];
  deleteLink: [id: string];
  moveLink: [move: { id: string; delta: -1 | 1 }];
}>();

const { t } = useI18n();

/**
 * The GitHub username row's inline error. Validation mirrors the backend's
 * grammar exactly (1–39 characters, letters/digits/hyphens, no leading or
 * trailing hyphen) so the inline error and the backend's refusal can never
 * disagree; the backend re-validates inside its transaction and remains
 * authoritative.
 */
const githubUsernameError = computed(() => {
  const value = props.githubUsername.trim();
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

/**
 * The GitHub username row's hint (decision D2): the field is always rendered,
 * and while the GitHub module is switched off the hint says what enabling it
 * does instead of describing a card that is not on the dashboard. Reads the
 * same draft the switch rows render, so the hint can never disagree with
 * them.
 */
const githubUsernameHint = computed(() =>
  props.moduleVisibility.github
    ? t("settings.githubUsernameHint")
    : t("settings.githubUsernameHintDisabled"),
);

/**
 * v-model bridge over the parent-owned username draft. The computed holds no
 * state of its own — reads flow down from the prop, writes emit up — so the
 * input keeps v-model's exact semantics (IME composition included) while the
 * only copy of the value stays the parent's draft.
 */
const usernameDraft = computed({
  get: () => props.githubUsername,
  set: (value: string) => emit("updateGithubUsername", value),
});

/** The username row, for the GitHub card deep-link's scrollIntoView. */
const githubRow = ref<HTMLElement | null>(null);
const githubInput = ref<HTMLInputElement | null>(null);

/**
 * Quick Links editor state. Transient and never persisted — an open editor
 * with half-typed fields must not make the panel claim it has unsaved
 * changes. `""` means "composing a new link", a link id means "editing that
 * link", `null` means no editor is open.
 */
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

/**
 * Validates the editor fields and reports the finished entry to the parent,
 * which owns the list and assigns a new link's id. Nothing here mutates the
 * draft: the rows re-render when the parent's list changes.
 */
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
    emit("updateLink", { id: editingLinkId.value, name, url });
  } else {
    emit("addLink", { name, url });
  }
  resetEditor();
}

/**
 * Reports the deletion and closes the editor if it was editing the deleted
 * row — the one piece of editor coupling the delete action carries.
 */
function deleteLink(id: string) {
  emit("deleteLink", id);
  if (editingLinkId.value === id) resetEditor();
}

onMounted(async () => {
  // Deep links (the GitHub card's setup entry) opened this panel on the
  // modules category — `initialCategory` chose it before first render, so the
  // target is visible and `scrollIntoView` below never runs against a hidden
  // panel. The tabpanel is the parent's `v-show`, so this runs exactly once
  // per panel session, on first mount.
  if (props.focusGithub) {
    await nextTick();
    githubRow.value?.scrollIntoView({ block: "start" });
    githubInput.value?.focus();
  }
});
</script>

<template>
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
        :aria-checked="moduleVisibility[module.id]"
        :aria-labelledby="`module-switch-label-${module.id}`"
        :aria-describedby="`module-switch-desc-${module.id}`"
        class="module-switch"
        :class="{ on: moduleVisibility[module.id] }"
        @click="$emit('toggleModule', module.id)"
      ><span class="module-switch-knob" aria-hidden="true"></span></button>
    </li>
  </ul>
  <div class="settings-module-reset">
    <button type="button" @click="$emit('resetModules')">{{ t("settings.modulesReset") }}</button>
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
      v-model="usernameDraft"
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
            @click="$emit('moveLink', { id: link.id, delta: -1 })"
          >↑</button>
          <button
            type="button"
            :disabled="index === quickLinks.length - 1"
            :title="t('quickLinks.moveDown')"
            :aria-label="t('quickLinks.moveDownAriaLabel', { name: link.name })"
            @click="$emit('moveLink', { id: link.id, delta: 1 })"
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
</template>
