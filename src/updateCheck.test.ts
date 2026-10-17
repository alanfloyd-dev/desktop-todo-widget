import { describe, expect, it, vi, type Mock } from "vitest";
import {
  createUpdateChecker,
  previewUpdateBridge,
  updateSurface,
  type UpdateBridge,
} from "./updateCheck";
// Vite's `?raw` import: the panel source as a string, for the structural
// wiring contract below (no Node fs types needed).
import panelSource from "./components/SettingsPanel.vue?raw";

/**
 * The bridge's `invoke` is runtime-generic (any result type); a test double
 * with concrete return types needs the cast to fit that signature.
 */
function asInvoke(mock: Mock<(command: string) => Promise<unknown>>): UpdateBridge["invoke"] {
  return mock as unknown as UpdateBridge["invoke"];
}

/**
 * A bridge whose `check_for_updates` answer is under the test's control:
 * `resolveCheck` releases the pending promise so a test can observe the
 * checking phase before the outcome lands.
 */
function deferredBridge() {
  let resolveCheck: (status: unknown) => void = () => {};
  const mock = vi.fn((command: string): Promise<unknown> => {
    if (command === "check_for_updates") {
      return new Promise((resolve) => {
        resolveCheck = resolve;
      });
    }
    return Promise.resolve({});
  });
  const bridge: UpdateBridge = { invoke: asInvoke(mock) };
  // Late-bound on purpose: the capture reads the closure variable, which the
  // pending promise's executor replaces once the check starts.
  return { bridge, invoke: mock, resolveCheck: (status: unknown) => resolveCheck(status) };
}

/** An update bridge scripted per command. */
function scriptedBridge(
  routes: Record<string, () => Promise<unknown>>,
): { bridge: UpdateBridge; invoke: Mock<(command: string) => Promise<unknown>> } {
  const mock = vi.fn((command: string): Promise<unknown> => {
    const route = routes[command];
    return route ? route() : Promise.resolve({});
  });
  return { bridge: { invoke: asInvoke(mock) }, invoke: mock };
}

/** Drain the microtask queue so a settled `check`/`install` has landed. */
async function settled() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("manual update check state machine", () => {
  it("moves idle → checking → available with the verified manifest version", async () => {
    const harness = deferredBridge();
    const checker = createUpdateChecker(harness.bridge);
    expect(checker.phase.value).toBe("idle");

    const pending = checker.check();
    expect(checker.phase.value).toBe("checking");

    harness.resolveCheck({ status: "update-available", version: "1.2.1" });
    await pending;
    expect(checker.phase.value).toBe("available");
    expect(checker.availableVersion.value).toBe("1.2.1");
    expect(checker.errorKey.value).toBeNull();
  });

  it("moves idle → checking → upToDate when the scan finds nothing new", async () => {
    const harness = deferredBridge();
    const checker = createUpdateChecker(harness.bridge);
    const pending = checker.check();
    expect(checker.phase.value).toBe("checking");

    harness.resolveCheck({ status: "up-to-date" });
    await pending;
    expect(checker.phase.value).toBe("upToDate");
    expect(checker.availableVersion.value).toBeNull();
  });

  it("an unavailable discovery layer is an error with retryable check copy", async () => {
    const checker = createUpdateChecker({
      invoke: asInvoke(vi.fn(() => Promise.reject(new Error("discovery-unavailable: GitHub: …")))),
    });
    await checker.check();
    expect(checker.phase.value).toBe("error");
    expect(checker.errorKey.value).toBe("settings.update.error.checkFailed");
  });

  it("an unresolved installation maps to its own localized copy", async () => {
    const checker = createUpdateChecker({
      invoke: asInvoke(vi.fn(() => Promise.reject(new Error("installation-unresolved")))),
    });
    await checker.check();
    expect(checker.errorKey.value).toBe("settings.update.error.installationUnresolved");
  });

  it("ignores repeated checks while one is in flight", async () => {
    const harness = deferredBridge();
    const checker = createUpdateChecker(harness.bridge);
    void checker.check();
    void checker.check();
    void checker.check();
    expect(harness.invoke).toHaveBeenCalledTimes(1);

    harness.resolveCheck({ status: "up-to-date" });
    await settled();
    expect(harness.invoke).toHaveBeenCalledTimes(1);
    expect(checker.phase.value).toBe("upToDate");
  });

  it("carries no version until the backend reports one", async () => {
    // The current-version row renders `getVersion()` output, and the offered
    // version is always the backend's verified manifest version — the
    // frontend never hardcodes or derives one.
    const harness = deferredBridge();
    const checker = createUpdateChecker(harness.bridge);
    expect(checker.availableVersion.value).toBeNull();

    const pending = checker.check();
    harness.resolveCheck({ status: "update-available", version: "9.9.9" });
    await pending;
    expect(checker.availableVersion.value).toBe("9.9.9");
  });

  it("install hands the staged session id to the frozen install command", async () => {
    const { bridge, invoke } = scriptedBridge({
      check_for_updates: () => Promise.resolve({ status: "update-available", version: "1.2.1" }),
      download_update: () => Promise.resolve({ sessionId: "sid-1", version: "1.2.1" }),
      install_ready_update: () => Promise.resolve(null),
    });
    const checker = createUpdateChecker(bridge);
    await checker.check();
    expect(checker.phase.value).toBe("available");

    await checker.install();
    expect(invoke).toHaveBeenCalledWith("download_update");
    expect(invoke).toHaveBeenCalledWith("install_ready_update", { sessionId: "sid-1" });
    // Success ends in the backend's canonical shutdown; the panel never
    // leaves the prepare phase of its own accord.
    expect(checker.phase.value).toBe("downloading");
  });

  it("a no-eligible-candidate refusal during install is an up-to-date answer", async () => {
    const { bridge } = scriptedBridge({
      check_for_updates: () => Promise.resolve({ status: "update-available", version: "1.2.1" }),
      download_update: () => Promise.reject(new Error("no-eligible-candidate")),
    });
    const checker = createUpdateChecker(bridge);
    await checker.check();
    await checker.install();
    expect(checker.phase.value).toBe("upToDate");
    expect(checker.availableVersion.value).toBeNull();
  });

  it("an install failure returns to the offer with retryable install copy", async () => {
    let downloads = 0;
    const { bridge, invoke } = scriptedBridge({
      check_for_updates: () => Promise.resolve({ status: "update-available", version: "1.2.1" }),
      download_update: () => {
        downloads += 1;
        return downloads === 1
          ? Promise.reject(new Error("download-failed: hash mismatch"))
          : Promise.resolve({ sessionId: "sid-2", version: "1.2.1" });
      },
    });
    const checker = createUpdateChecker(bridge);
    await checker.check();
    await checker.install();
    expect(checker.phase.value).toBe("available");
    expect(checker.availableVersion.value).toBe("1.2.1");
    expect(checker.errorKey.value).toBe("settings.update.error.installFailed");

    // The failed action is retryable.
    await checker.install();
    expect(invoke).toHaveBeenCalledWith("install_ready_update", { sessionId: "sid-2" });
    expect(checker.errorKey.value).toBeNull();
  });

  it("install is refused outside the available phase", async () => {
    const { bridge, invoke } = scriptedBridge({
      check_for_updates: () => Promise.resolve({ status: "up-to-date" }),
    });
    const checker = createUpdateChecker(bridge);
    await checker.install();
    expect(invoke).not.toHaveBeenCalledWith("download_update");
    expect(checker.phase.value).toBe("idle");
  });

  it("a not-managed refusal maps to its own localized copy", async () => {
    const checker = createUpdateChecker({
      invoke: asInvoke(vi.fn(() => Promise.reject(new Error("not-managed")))),
    });
    await checker.check();
    expect(checker.errorKey.value).toBe("settings.update.error.notManaged");
  });

  describe("update surface environments", () => {
    it("browser preview: visible, inert, never live", () => {
      expect(
        updateSurface({ isPreview: true, isDevRuntime: true, admissionMode: null }),
      ).toEqual({ visible: true, backendLive: false });
    });

    it("tauri dev (DEV=true, Tauri runtime): visible but inert whatever the admission", () => {
      // A dev process is classified "development" — and even if a managed
      // receipt were somehow present, a dev runtime never drives the updater.
      expect(
        updateSurface({ isPreview: false, isDevRuntime: true, admissionMode: "development" }),
      ).toEqual({ visible: true, backendLive: false });
      expect(
        updateSurface({ isPreview: false, isDevRuntime: true, admissionMode: "managed" }),
      ).toEqual({ visible: true, backendLive: false });
      expect(
        updateSurface({ isPreview: false, isDevRuntime: true, admissionMode: null }),
      ).toEqual({ visible: true, backendLive: false });
    });

    it("managed production install: visible and live", () => {
      expect(
        updateSurface({ isPreview: false, isDevRuntime: false, admissionMode: "managed" }),
      ).toEqual({ visible: true, backendLive: true });
    });

    it("production native, unmanaged or development: hidden", () => {
      expect(
        updateSurface({ isPreview: false, isDevRuntime: false, admissionMode: "unmanaged" }),
      ).toEqual({ visible: false, backendLive: false });
      expect(
        updateSurface({ isPreview: false, isDevRuntime: false, admissionMode: "development" }),
      ).toEqual({ visible: false, backendLive: false });
      // Admission not yet arrived (or failed): hidden, never half-enabled.
      expect(
        updateSurface({ isPreview: false, isDevRuntime: false, admissionMode: null }),
      ).toEqual({ visible: false, backendLive: false });
    });

    it("the preview bridge refuses every command with the installed-only copy", async () => {
      // `previewUpdateBridge` holds no reference to the Tauri runtime, so
      // this check never reaches the updater backend by construction.
      const checker = createUpdateChecker(previewUpdateBridge);
      await checker.check();
      expect(checker.phase.value).toBe("error");
      expect(checker.errorKey.value).toBe("settings.update.previewOnly");
    });

    it("the panel wires the surface rule, the inert bridge for dev, and disabled controls", () => {
      // Source-pinned contract (the repo's established pattern for
      // harness-bound surfaces): the panel renders the section from the
      // shared surface rule, selects the inert bridge for every
      // preview-or-dev runtime, and disables the controls outside a live
      // backend.
      expect(panelSource).toContain('v-if="updateSectionVisible"');
      expect(panelSource).toContain(
        "createUpdateChecker(\n  isPreview || isDevRuntime ? previewUpdateBridge : { invoke },\n)",
      );
      expect(panelSource).toContain(':disabled="updateControlsDisabled"');
      expect(panelSource).toContain('v-if="!updateBackendLive" role="note"');
    });

    it("the preview bridge marker never collides with the backend vocabulary", () => {
      // The refusal mapping keys off the `not-installed` substring; no
      // backend refusal string may contain it, or a native failure could be
      // mislabeled as preview-only.
      const backendRefusals = [
        "not-managed",
        "installation-unresolved",
        "discovery-unavailable: GitHub: transport failure",
        "no-eligible-candidate",
        "download-failed: hash mismatch",
        "session-unknown",
        "session-not-staged",
        "source-version-changed",
        "launch-failed: spawn",
        "handoff-not-durable",
        "check-failed: join",
      ];
      for (const refusal of backendRefusals) {
        expect(refusal.includes("not-installed")).toBe(false);
      }
    });
  });
});
