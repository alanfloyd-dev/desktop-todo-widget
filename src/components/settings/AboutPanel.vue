<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useI18n } from "../../i18n";
import { createUpdateChecker, previewUpdateBridge, updateSurface } from "../../updateCheck";
import type { MaintenanceAdmissionState } from "../../types";
import DeveloperDiagnostics from "../DeveloperDiagnostics.vue";

/**
 * The About category: the manual update check, the managed-install uninstall
 * entry, and the developer section.
 *
 * It renders inside the `v-show` tabpanel `SettingsPanel` owns, so it mounts and
 * unmounts with the Settings panel rather than with the active category, and
 * every piece of state below lives for exactly one panel session. It owns no
 * settings draft and takes no part in Save: nothing here is persisted.
 */
defineProps<{
  databasePath: string;
}>();

const { t } = useI18n();

const developerOpen = ref(false);

/**
 * Launch-admission classification from the Rust backend.
 *
 * The uninstall entry is a managed-installation capability: it renders only
 * when this instance admitted as `managed`. An unmanaged, legacy, or
 * development copy gets no entry at all (rather than a disabled one) because
 * there is genuinely nothing for the maintenance tool to uninstall — and the
 * backend would refuse anyway. `null` means the state has not arrived (or the
 * panel runs in a plain browser preview), which also hides the entry.
 */
const admissionMode = ref<MaintenanceAdmissionState["mode"] | null>(null);
const uninstallConfirming = ref(false);
const uninstallStarting = ref(false);
const uninstallError = ref("");

/**
 * The plain browser preview: no Tauri runtime at all. This is the app's
 * established native/preview boundary — the same check every bridge consumer
 * in `App.vue` makes.
 */
const isPreview = !("__TAURI_INTERNALS__" in window);
/**
 * Vite dev server (`pnpm tauri dev` included — it drives the same dev
 * server). A Tauri dev webview has `__TAURI_INTERNALS__` but is still a
 * development process, so it must never reach the updater backend; DEV is
 * the Vite-compiled boundary between dev and `tauri build` output.
 */
const isDevRuntime = import.meta.env.DEV;

/**
 * The Update section's environment rule (see `updateSurface`): visible for a
 * managed production install, the browser preview, and `tauri dev`; live
 * (real bridge, enabled controls) only for the managed production install.
 * Preview and dev render it display-only on the inert bridge — the same rule
 * as the uninstall entry for production, extended by an inert development
 * view so the layout stays developable.
 */
const surface = computed(() =>
  updateSurface({ isPreview, isDevRuntime, admissionMode: admissionMode.value }),
);
const updateSectionVisible = computed(() => surface.value.visible);
const updateBackendLive = computed(() => surface.value.backendLive);

/**
 * The manual update check state machine, created per panel session: state
 * resets when Settings reopens, which is the honest scope for a check that
 * talks to the providers right now.
 *
 * The real `invoke` is wired only outside preview and dev runtimes — every
 * other context gets the inert bridge, so no code path here can reach
 * `check_for_updates`/`download_update`/`install_ready_update` from a
 * development process.
 */
const updateChecker = createUpdateChecker(
  isPreview || isDevRuntime ? previewUpdateBridge : { invoke },
);
const updatePhase = updateChecker.phase;
const updateAvailableVersion = updateChecker.availableVersion;
const updateErrorKey = updateChecker.errorKey;

/**
 * The installed product version, read from the app's package metadata —
 * never hardcoded. The backend re-derives the authoritative installed
 * source version (receipt + PE) for every check and install; this value is
 * display-only. The browser preview never issues the doomed metadata call
 * (a Tauri dev webview can and shows its dev version).
 */
const currentVersion = ref("");
if (!isPreview) {
  void Promise.resolve()
    .then(() => getVersion())
    .then((version) => {
      currentVersion.value = version;
    })
    .catch(() => {
      currentVersion.value = "";
    });
}

const updateBusy = computed(
  () => updatePhase.value === "checking" || updatePhase.value === "downloading",
);

/** Outside a managed production install the whole section is display-only. */
const updateControlsDisabled = computed(() => updateBusy.value || !updateBackendLive.value);

const updateCheckLabel = computed(() => {
  if (updatePhase.value === "checking") return t("settings.update.checking");
  return t("settings.update.check");
});

const updateInstallLabel = computed(() => {
  if (updatePhase.value === "downloading") return t("settings.update.downloading");
  return t("settings.update.install");
});

onMounted(async () => {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    admissionMode.value = (
      await invoke<MaintenanceAdmissionState>("maintenance_admission_state")
    ).mode;
  } catch {
    admissionMode.value = null;
  }
});

/// Maps the backend's pinned refusal codes to localized copy. Any failure
/// leaves the app running and the entry usable.
function uninstallErrorLabel(code: unknown): string {
  const text = String(code);
  if (text.includes("already-started")) return t("settings.application.error.busy");
  if (text.includes("not-managed")) return t("settings.application.error.notManaged");
  return t("settings.application.error.launchFailed");
}

function beginUninstall() {
  if (admissionMode.value !== "managed" || uninstallStarting.value) return;
  uninstallError.value = "";
  uninstallConfirming.value = true;
}

function cancelUninstall() {
  uninstallConfirming.value = false;
  uninstallError.value = "";
}

async function confirmUninstall() {
  if (admissionMode.value !== "managed" || uninstallStarting.value) return;
  uninstallStarting.value = true;
  uninstallError.value = "";
  try {
    // The backend launches the canonical maintenance helper and then exits
    // the app. On success there is nothing left to render into.
    await invoke("start_uninstall");
  } catch (code) {
    uninstallError.value = uninstallErrorLabel(code);
    uninstallStarting.value = false;
  }
}
</script>

<template>
  <div class="about-panel">
    <!--
      Environment rule (see `updateSurface` in updateCheck.ts): rendered
      for a managed production install, the browser preview, and `tauri
      dev`; live only for the managed production install — the same rule
      as the Application section, extended by an inert development view.
      Outside it the controls are disabled, the installed-only hint shows,
      and the inert bridge makes every updater command unreachable. The
      states below are the check state machine's phases — a click is never
      a silent no-op: checking shows in the button, the outcome shows in
      the status line, and a failure shows as an error with the failed
      action retryable.
    -->
    <template v-if="updateSectionVisible">
      <div class="settings-section-heading">
        <strong id="settings-update-heading">{{ t("settings.update.heading") }}</strong>
      </div>
      <section class="update-settings" :aria-label="t('settings.update.heading')">
        <div class="settings-row">
          <span>
            <strong>{{ t("settings.update.currentVersion") }}</strong>
            <small>{{ t("settings.update.currentVersionHint") }}</small>
          </span>
          <span class="update-version">{{ currentVersion || "—" }}</span>
        </div>
        <div class="settings-row">
          <span>
            <strong>{{ t("settings.update.check") }}</strong>
            <small>{{ t("settings.update.checkHint") }}</small>
          </span>
          <button
            type="button"
            :disabled="updateControlsDisabled"
            @click="updateChecker.check()"
          >{{ updateCheckLabel }}</button>
        </div>
        <div class="update-status-row">
          <!--
            Outside a managed production install the explanation is
            persistent, not click-triggered: a disabled control that
            explains itself beats a dead one.
          -->
          <small v-if="!updateBackendLive" role="note">{{ t("settings.update.previewOnly") }}</small>
          <small v-else-if="updatePhase === 'upToDate'" role="status">{{ t("settings.update.upToDate") }}</small>
          <template v-else-if="updatePhase === 'available'">
            <small role="status">{{ t("settings.update.available", { version: updateAvailableVersion ?? "" }) }}</small>
            <button
              type="button"
              class="update-install"
              :disabled="updateControlsDisabled"
              @click="updateChecker.install()"
            >{{ updateInstallLabel }}</button>
            <small class="update-install-hint">{{ t("settings.update.installHint") }}</small>
            <!--
              An install failure returns to this offer with the errorKey set
              (the update is still offered and retrying is meaningful), so the
              error renders here too: the `error` phase below is only reachable
              from a failed check, and a phase-gated render would make every
              install refusal a silent no-op.
            -->
            <small
              v-if="updateErrorKey"
              class="settings-error"
              role="alert"
            >{{ t(updateErrorKey) }}</small>
          </template>
          <small v-else-if="updatePhase === 'downloading'" role="status">{{ t("settings.update.downloading") }}</small>
          <small
            v-else-if="updatePhase === 'error' && updateErrorKey"
            class="settings-error"
            role="alert"
          >{{ t(updateErrorKey) }}</small>
        </div>
      </section>
    </template>

    <!--
      Managed installations only: an unmanaged or development copy does not
      own the maintenance uninstaller, so the entry is absent instead of
      disabled. The lightweight confirm only announces the handoff — the
      keep/delete choice and every destructive step live in the native
      maintenance tool, never duplicated here.
    -->
    <template v-if="admissionMode === 'managed'">
      <div class="settings-section-heading">
        <strong>{{ t("settings.application.heading") }}</strong>
      </div>
      <section class="application-settings" :aria-label="t('settings.application.heading')">
        <div class="settings-row">
          <span>
            <strong>{{ t("settings.application.uninstall") }}</strong>
            <small>{{ t("settings.application.uninstallHint") }}</small>
          </span>
          <button
            type="button"
            :disabled="uninstallStarting"
            @click="beginUninstall"
          >{{ t("settings.application.uninstall") }}</button>
        </div>
        <div v-if="uninstallConfirming" class="quick-link-editor">
          <p class="uninstall-confirm-body">
            <strong>{{ t("settings.application.confirmTitle") }}</strong>
            {{ t("settings.application.confirmBody") }}
          </p>
          <div class="quick-link-editor-actions">
            <button type="button" :disabled="uninstallStarting" @click="confirmUninstall">
              {{ uninstallStarting ? t("settings.application.starting") : t("settings.application.confirmContinue") }}
            </button>
            <button type="button" :disabled="uninstallStarting" @click="cancelUninstall">
              {{ t("quickLinks.cancel") }}
            </button>
          </div>
        </div>
        <p v-if="uninstallError" class="settings-error" role="alert">{{ uninstallError }}</p>
      </section>
    </template>

    <section class="developer-section">
      <button type="button" class="developer-toggle" @click="developerOpen = !developerOpen">
        <span>{{ t("settings.developer") }}</span><span>{{ developerOpen ? "−" : "+" }}</span>
      </button>
      <div v-if="developerOpen" class="developer-content">
        <p class="database-path">{{ t("settings.developer.databasePath", { path: databasePath }) }}</p>
        <DeveloperDiagnostics />
      </div>
    </section>
  </div>
</template>
