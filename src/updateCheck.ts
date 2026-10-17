import { ref, type Ref } from "vue";

/**
 * The bridge the update checker runs through.
 *
 * The browser preview has no Tauri runtime; the checker is only constructed
 * for managed native installs, and tests inject a fake bridge to drive the
 * state transitions deterministically.
 */
export interface UpdateBridge {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

/**
 * The user-visible phases of a manual update check/install.
 *
 * - `idle` — nothing has been checked yet this panel session.
 * - `checking` — a check is in flight; further checks are ignored.
 * - `upToDate` — the bounded scan completed; nothing new this client can take.
 * - `available` — a newer version passed the full eligibility predicate;
 *   `availableVersion` is the verified manifest version, never a provider tag.
 * - `downloading` — the controlled prepare (discover → persist → stage) and
 *   the frozen install handoff are in flight; the app exits on success.
 * - `error` — the last action failed; `errorKey` names the localized copy and
 *   the failed action can be retried.
 */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "error";

/** Which action an `error` phase belongs to, so retry copy stays accurate. */
export type UpdateAction = "check" | "install";

/** The `check_for_updates` response vocabulary (see `updater/check.rs`). */
export interface UpdateCheckStatus {
  status: "up-to-date" | "update-available";
  version?: string;
}

/** The `download_update` response: the frozen install command's only input. */
export interface StagedUpdate {
  sessionId: string;
  version: string;
}

/**
 * Inert bridge for every non-managed surface (plain browser preview, Vite
 * dev server, `tauri dev`).
 *
 * It rejects every command without ever touching `@tauri-apps`, so a visible
 * Update section can be styled and developed while remaining structurally
 * unable to reach the updater backend. The stable `not-installed` marker maps
 * to the installed-only copy in `updateErrorLabel`.
 */
export const previewUpdateBridge: UpdateBridge = {
  invoke: () => Promise.reject(new Error("not-installed")),
};

/**
 * The launch-admission mode as the Update section sees it. `null` before the
 * backend state has arrived (or in a context that never reports one).
 */
export type UpdateAdmissionMode = "managed" | "unmanaged" | "development" | null;

/** The three runtime contexts the Update section distinguishes. */
export interface UpdateSurfaceContext {
  /** No Tauri runtime: the plain browser preview. */
  isPreview: boolean;
  /** Vite dev server (`import.meta.env.DEV`) — true for `tauri dev` too. */
  isDevRuntime: boolean;
  /** Launch admission from the backend. */
  admissionMode: UpdateAdmissionMode;
}

/** The Update section's visibility and backend reachability. */
export interface UpdateSurface {
  /** The section renders at all. */
  visible: boolean;
  /**
   * The updater backend may actually be reached. True only for a managed
   * **production** install — the single context where the controls are
   * enabled and the real bridge is wired. Preview and dev runtimes render
   * the section display-only, whatever the admission says, so a development
   * process can never drive the updater.
   */
  backendLive: boolean;
}

/**
 * The single visibility/enablement rule for the Update section.
 *
 * - Managed production install — visible, live.
 * - Browser preview / dev runtime — visible but inert (disabled controls,
 *   installed-only hint, inert bridge): development must never reach the
 *   updater backend, and a dev copy's receipt authority would fail closed
 *   anyway.
 * - Production native, unmanaged / development / unknown — hidden, matching
 *   the managed-only rule of the other install-dependent entries.
 */
export function updateSurface(context: UpdateSurfaceContext): UpdateSurface {
  const backendLive =
    context.admissionMode === "managed" && !context.isPreview && !context.isDevRuntime;
  return {
    visible: context.isPreview || context.isDevRuntime || context.admissionMode === "managed",
    backendLive,
  };
}

/**
 * Maps a backend refusal to localized copy. The backend speaks a small,
 * pinned string vocabulary (the `CheckError`/`InstallError` `Display`
 * shapes); anything unrecognised becomes the action's generic message rather
 * than leaking a raw string into a localized UI.
 */
function updateErrorLabel(action: UpdateAction, reason: unknown): string {
  const text = String(reason);
  if (text.includes("not-installed")) return "settings.update.previewOnly";
  if (text.includes("not-managed")) return "settings.update.error.notManaged";
  if (text.includes("installation-unresolved")) {
    return "settings.update.error.installationUnresolved";
  }
  if (action === "check") {
    return "settings.update.error.checkFailed";
  }
  return "settings.update.error.installFailed";
}

/**
 * The manual update check state machine (the frontend half of the in-app
 * updater surface).
 *
 * The frontend never downloads, parses, verifies, or stages anything: it
 * asks the backend to check, renders the backend's verdict, and — only after
 * the user confirms — hands the staged session id back to the frozen
 * `install_ready_update` command, which owns the entire install transaction
 * and the canonical shutdown.
 */
export function createUpdateChecker(bridge: UpdateBridge) {
  const phase = ref<UpdatePhase>("idle");
  const availableVersion = ref<string | null>(null);
  const errorKey = ref<string | null>(null);
  const errorAction = ref<UpdateAction>("check");

  async function check() {
    // Single-flight: while a check or a prepare/install is running, further
    // clicks are no-ops. The buttons are disabled too; the guard is the
    // state-machine half of the same rule.
    if (phase.value === "checking" || phase.value === "downloading") return;
    errorKey.value = null;
    phase.value = "checking";
    try {
      const status = await bridge.invoke<UpdateCheckStatus>("check_for_updates");
      if (status.status === "update-available" && status.version) {
        availableVersion.value = status.version;
        phase.value = "available";
      } else {
        availableVersion.value = null;
        phase.value = "upToDate";
      }
    } catch (reason) {
      availableVersion.value = null;
      errorAction.value = "check";
      errorKey.value = updateErrorLabel("check", reason);
      phase.value = "error";
    }
  }

  async function install() {
    if (phase.value !== "available") return;
    errorKey.value = null;
    phase.value = "downloading";
    try {
      const staged = await bridge.invoke<StagedUpdate>("download_update");
      // On success this never really "resolves into UI": the frozen install
      // command performs the canonical shutdown once the handoff is durable.
      // Until the process exits, the prepare phase stays on screen.
      await bridge.invoke("install_ready_update", { sessionId: staged.sessionId });
    } catch (reason) {
      const text = String(reason);
      // Between check and install the provider state can move on: a scan
      // that now finds nothing eligible is an honest "up to date", not a
      // failure. Every other refusal returns to the offer with localized
      // copy; the update is still available and retrying is meaningful.
      if (text.includes("no-eligible-candidate")) {
        availableVersion.value = null;
        phase.value = "upToDate";
        return;
      }
      errorAction.value = "install";
      errorKey.value = updateErrorLabel("install", reason);
      phase.value = "available";
    }
  }

  return {
    phase: phase as Ref<UpdatePhase>,
    availableVersion: availableVersion as Ref<string | null>,
    errorKey: errorKey as Ref<string | null>,
    errorAction: errorAction as Ref<UpdateAction>,
    check,
    install,
  };
}
