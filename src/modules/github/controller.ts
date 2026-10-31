import { ref, type Ref } from "vue";
import type { GithubViewState } from "../../types";

/**
 * The bridge the GitHub controller runs through.
 *
 * The same native/preview boundary as the weather controller: the browser
 * preview has no Tauri runtime, `available` is false there, and every
 * command call is skipped. Tests inject a fake bridge to drive the state
 * transitions deterministically.
 */
export interface GithubBridge {
  available: boolean;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

/**
 * GitHub status as a translation key, so it re-renders on language change.
 * The controller stores keys (and their substitution params) only; the owner
 * resolves them with the app's single i18n instance.
 */
export interface GithubMessage {
  key: string;
  params?: Record<string, string | number>;
}

/**
 * The preview fallback state. The browser preview renders a static sample so
 * the surface stays inspectable without a backend (and without the preview
 * ever touching the network); the native path starts unconfigured and is
 * populated by `github_state` on startup.
 */
export function browserGithubState(nativeBridgeAvailable: boolean): GithubViewState {
  if (!nativeBridgeAvailable) {
    return {
      configured: true,
      username: "octocat",
      snapshot: {
        username: "octocat",
        followers: 12,
        stars: 128,
        fetchedAt: Math.floor(Date.now() / 1000),
      },
      cacheStatus: "fresh",
      cacheAgeSeconds: 0,
      refreshing: false,
      rateLimited: false,
      retryAfterSeconds: null,
      lastRefresh: "browser-preview",
    };
  }
  return {
    configured: false,
    username: "",
    snapshot: null,
    cacheStatus: "missing",
    cacheAgeSeconds: null,
    refreshing: false,
    rateLimited: false,
    retryAfterSeconds: null,
    lastRefresh: "none",
  };
}

/** Extracts the backend's pinned error kind from a rejected invoke. */
function errorKind(reason: unknown): string {
  return typeof reason === "object" && reason && "kind" in reason
    ? String((reason as { kind: unknown }).kind)
    : String(reason);
}

/**
 * Maps a backend GitHub failure to localized copy. The backend speaks a
 * small, pinned vocabulary (see `github.rs`); anything unrecognised becomes
 * the generic message rather than leaking a raw string into a localized UI.
 * Cooldown, in-progress, rate-limit, not-found, and generic unavailability
 * are distinct situations and get distinct copy (M3).
 */
export function githubErrorLabel(reason: unknown, username: string): GithubMessage {
  const kind = errorKind(reason);
  if (kind.includes("RateLimited")) {
    const message = typeof reason === "object" && reason && "message" in reason
      ? (reason as { message: unknown }).message
      : null;
    const seconds = typeof message === "number" && Number.isFinite(message) ? message : null;
    return {
      key: "github.rateLimited",
      params: { minutes: Math.max(1, Math.ceil((seconds ?? 600) / 60)) },
    };
  }
  if (kind.includes("NotFound")) {
    return { key: "github.error.notFound", params: { name: username } };
  }
  if (kind.includes("InvalidUsername")) {
    return { key: "github.error.invalidUsername" };
  }
  if (kind.includes("Incomplete")) {
    return { key: "github.error.incomplete" };
  }
  if (kind.includes("Cooldown")) {
    return { key: "github.error.cooldown" };
  }
  if (kind.includes("InProgress")) {
    return { key: "github.error.inProgress" };
  }
  return { key: "github.error.generic" };
}

/**
 * The single GitHub state owner for the whole product surface, the weather
 * controller's shape at a smaller scope plus one concern weather does not
 * have: the configured identity can change while a fetch is in flight.
 * Ownership rules:
 *
 * - `github` is the only copy of the `GithubViewState`; both the dashboard
 *   card and the Settings GitHub section render from it.
 * - Every async completion carries the identity generation (`epoch`) it
 *   started under and is discarded whole when that generation has been
 *   superseded — a response from account A can never overwrite account B's
 *   statistics, loading state, or message (M1/M4).
 * - Refreshes serialize behind the in-flight invoke. That invoke resolves
 *   exactly when the backend releases its single-flight flag, so awaiting it
 *   is awaiting the flag: the new account's refresh starts after the old
 *   fetch settles instead of being dropped by an `InProgress` refusal (M1).
 *   No timers, no polling, no retries — and same-account triggers keep the
 *   ordinary single-flight no-op.
 * - `refreshGithub` is guarded before it can reach the bridge: the runtime
 *   must exist and the module must be configured. The configured guard is
 *   the frontend half of the zero-request contract (ADR-DG-001 Amendment ①)
 *   — the backend refuses an empty username again before its own refresh
 *   gate, so neither side alone is trusted to keep an unconfigured module
 *   off the network.
 * - A failed manual refresh keeps the previous snapshot (re-synced from the
 *   backend cache via `loadGithub`) and surfaces the failure as a message
 *   key; it never blanks the card.
 */
export function createGithubController(bridge: GithubBridge) {
  const github = ref<GithubViewState>(browserGithubState(bridge.available));
  const message = ref<GithubMessage | null>(null);
  /**
   * The account the backend has explicitly reported as nonexistent, for the
   * current identity only. Distinct from "no cached data": the card renders
   * a not-found explanation instead of the empty placeholder (M2). Cleared
   * by a successful refresh or by any identity change.
   */
  const notFound = ref<string | null>(null);

  /**
   * The identity generation. Bumped by every configuration change — username
   * switch, cleared username, module hidden — so pending work for a previous
   * generation becomes inert at its next completion point.
   */
  let epoch = 0;
  /**
   * The in-flight refresh invoke and the epoch it belongs to. The promise is
   * the backend completion signal described above; `inFlightEpoch === -1`
   * means nothing is in flight or queued.
   */
  let inFlight: Promise<void> | null = null;
  let inFlightEpoch = -1;
  /** The epoch that already owns the queue (in flight or waiting behind it). */
  let queuedEpoch = -1;

  /**
   * Invalidates every pending refresh intent without changing configuration
   * (the module was hidden): queued fetches for this generation abandon at
   * their next completion point.
   */
  function invalidate() {
    epoch += 1;
  }

  /**
   * Applies a configuration change: the card resets to the new identity's
   * honest state, the previous identity's message and not-found fact die
   * with it (M4), and every pending refresh intent for the previous
   * identity is invalidated.
   */
  function switchIdentity(username: string) {
    epoch += 1;
    const trimmed = username.trim();
    message.value = null;
    notFound.value = null;
    github.value = {
      ...browserGithubState(bridge.available),
      configured: trimmed !== "",
      username: trimmed,
    };
  }

  async function loadGithub() {
    if (!bridge.available) return;
    const myEpoch = epoch;
    try {
      const view = await bridge.invoke<GithubViewState>("github_state");
      // A state read that started under a previous identity never lands: the
      // backend answers with the settings it saw, which may name the old
      // account and carry its snapshot.
      if (myEpoch !== epoch) return;
      github.value = view;
    } catch {
      if (myEpoch !== epoch) return;
      // GitHub statistics are isolated from product startup; a missing or
      // corrupt cache must not prevent Todo or window state from rendering.
      github.value = browserGithubState(false);
    }
  }

  async function refreshGithub(manual = false) {
    if (!bridge.available || !github.value.configured) return;
    // Same-account single-flight, unchanged: a trigger while this identity's
    // refresh is already queued or in flight is a no-op.
    if (queuedEpoch === epoch || inFlightEpoch === epoch) return;
    await performRefresh(epoch, manual);
  }

  async function performRefresh(myEpoch: number, manual: boolean) {
    queuedEpoch = myEpoch;
    try {
      // Serialize behind the in-flight fetch. The awaited promise settles
      // exactly when the backend single-flight flag is released, so the new
      // account's refresh starts after the old fetch is done — it can neither
      // collide with it nor be dropped by an InProgress refusal. One awaited
      // promise per queued refresh; no timers, no polling, no retries.
      while (inFlight) {
        await inFlight;
        // Superseded while queued (switch, clear, hide): abandon silently.
        if (myEpoch !== epoch) return;
      }
      // Re-validate every gate after the wait.
      if (!bridge.available || !github.value.configured) return;
      if (myEpoch !== epoch) return;
      message.value = null;
      github.value.refreshing = true;
      const run = (async () => {
        try {
          const view = await bridge.invoke<GithubViewState>("refresh_github", { manual });
          // A response from a superseded identity is discarded whole: it must
          // not overwrite the new identity's statistics, loading state, or
          // message, and it must not clear its not-found fact.
          if (myEpoch !== epoch) return;
          github.value = view;
          notFound.value = null;
          if (manual) message.value = { key: "status.githubUpdated" };
        } catch (reason) {
          if (myEpoch !== epoch) return;
          github.value.refreshing = false;
          const kind = errorKind(reason);
          if (kind.includes("NotFound")) {
            // A terminal fact about the current identity: surfaced on the
            // card whether the fetch was manual or automatic (M2), once —
            // repeated automatic triggers re-set the same value, never a
            // growing stream of notifications.
            notFound.value = github.value.username.trim();
          } else if (manual) {
            message.value = githubErrorLabel(reason, github.value.username);
          }
          await loadGithub();
        } finally {
          if (myEpoch === epoch) github.value.refreshing = false;
        }
      })();
      inFlightEpoch = myEpoch;
      inFlight = run;
      try {
        await run;
      } finally {
        if (inFlight === run) {
          inFlight = null;
          inFlightEpoch = -1;
        }
      }
    } finally {
      if (queuedEpoch === myEpoch) queuedEpoch = -1;
    }
  }

  /**
   * The one automatic refresh trigger, shared by startup and the settings-
   * save path: a visible, configured module whose cache is not fresh gets
   * one background refresh. Hidden modules, unconfigured usernames, fresh
   * caches, and rate-limited windows stay off the network — no retry storm.
   */
  function refreshIfStale(visible: boolean) {
    if (!visible || !github.value.configured) return;
    if (github.value.cacheStatus === "fresh") return;
    if (github.value.rateLimited) return;
    void refreshGithub(false);
  }

  return {
    github: github as Ref<GithubViewState>,
    message,
    notFound,
    loadGithub,
    refreshGithub,
    refreshIfStale,
    switchIdentity,
    invalidate,
  };
}
