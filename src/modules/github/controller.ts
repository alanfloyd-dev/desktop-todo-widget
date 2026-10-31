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
      search: {
        username: "octocat",
        openIssues: 5,
        pullRequests: 42,
        fetchedAt: Math.floor(Date.now() / 1000),
      },
      searchCacheStatus: "fresh",
      searchCacheAgeSeconds: 0,
      searchRefreshing: false,
      searchLastRefresh: "browser-preview",
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
    search: null,
    searchCacheStatus: "missing",
    searchCacheAgeSeconds: null,
    searchRefreshing: false,
    searchLastRefresh: "none",
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
 *
 * The `plane` names which refresh failed: the profile plane's incomplete
 * error is about the repository traversal, while the search plane's is about
 * the provider declaring its index answer incomplete — different facts, so
 * different copy. Not-found is profile-plane vocabulary by construction: the
 * search endpoint cannot prove an account exists — a nonexistent author is
 * refused with a validation error, not an answer — and this mapping keeps
 * its refusals generic so the fact stays with the profile fetch.
 */
export function githubErrorLabel(
  reason: unknown,
  username: string,
  plane: "stats" | "search" = "stats",
): GithubMessage {
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
    return plane === "search"
      ? { key: "github.error.searchIncomplete" }
      : { key: "github.error.incomplete" };
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
 * The two independent refresh planes. The profile plane (G1) is the
 * followers/stars fetch; the search plane (G2) is the issues/pull-requests
 * fetch. Each runs its own backend command behind its own queue slot, so a
 * search refresh can neither collide with nor be dropped by an in-flight
 * profile refresh — while the shared identity epoch keeps both of them
 * honest across account switches.
 */
type RefreshPlane = "stats" | "search";

const PLANE_COMMAND: Record<RefreshPlane, string> = {
  stats: "refresh_github",
  search: "refresh_github_search",
};

/**
 * The single GitHub state owner for the whole product surface, the weather
 * controller's shape at a smaller scope plus two concerns weather does not
 * have: the configured identity can change while a fetch is in flight, and
 * since G2 there are two independent planes to keep honest. Ownership rules:
 *
 * - `github` is the only copy of the `GithubViewState`; both the dashboard
 *   card and the Settings GitHub section render from it.
 * - Every async completion carries the identity generation (`epoch`) it
 *   started under and is discarded whole when that generation has been
 *   superseded — a response from account A can never overwrite account B's
 *   statistics, loading state, or message (M1/M4). The epoch is shared by
 *   both planes: a switch invalidates the previous identity's work on either.
 * - Refreshes serialize per plane behind that plane's in-flight invoke. That
 *   invoke resolves exactly when the backend releases its single-flight flag,
 *   so awaiting it is awaiting the flag: the new account's refresh starts
 *   after the old fetch settles instead of being dropped by an `InProgress`
 *   refusal (M1). No timers, no polling, no retries — and same-account
 *   triggers keep the ordinary single-flight no-op. The planes' queues are
 *   independent: a search refresh never waits behind a profile refresh.
 * - `refreshGithub`/`refreshSearch` are guarded before they can reach the
 *   bridge: the runtime must exist and the module must be configured. The
 *   configured guard is the frontend half of the zero-request contract
 *   (ADR-DG-001 Amendment ①) — the backend refuses an empty username again
 *   before its own refresh gate, so neither side alone is trusted to keep an
 *   unconfigured module off the network.
 * - A failed manual refresh keeps the previous snapshot (re-synced from the
 *   backend cache via `loadGithub`) and surfaces the failure as a message
 *   key; it never blanks the card. The not-found fact belongs to the profile
 *   plane alone: only that plane's fetch can prove the account nonexistent,
 *   and a search success or failure never touches it.
 */
export function createGithubController(bridge: GithubBridge) {
  const github = ref<GithubViewState>(browserGithubState(bridge.available));
  const message = ref<GithubMessage | null>(null);
  /**
   * The account the backend has explicitly reported as nonexistent, for the
   * current identity only. Distinct from "no cached data": the card renders
   * a not-found explanation instead of the empty placeholder (M2). Cleared
   * by a successful profile-plane refresh or by any identity change.
   */
  const notFound = ref<string | null>(null);

  /**
   * The identity generation. Bumped by every configuration change — username
   * switch, cleared username, module hidden — so pending work for a previous
   * generation becomes inert at its next completion point, on both planes.
   */
  let epoch = 0;

  /** One plane's queue slot: the in-flight invoke and the epochs around it. */
  interface PlaneQueue {
    inFlight: Promise<void> | null;
    inFlightEpoch: number;
    queuedEpoch: number;
  }
  const planes: Record<RefreshPlane, PlaneQueue> = {
    stats: { inFlight: null, inFlightEpoch: -1, queuedEpoch: -1 },
    search: { inFlight: null, inFlightEpoch: -1, queuedEpoch: -1 },
  };

  /**
   * Invalidates every pending refresh intent without changing configuration
   * (the module was hidden): queued fetches for this generation abandon at
   * their next completion point, on both planes.
   */
  function invalidate() {
    epoch += 1;
  }

  /**
   * Applies a configuration change: the card resets to the new identity's
   * honest state, the previous identity's message and not-found fact die
   * with it (M4), and every pending refresh intent for the previous
   * identity is invalidated — on both planes.
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
    await refreshPlane("stats", manual);
  }

  /** The search plane's manual/automatic refresh entry (G2). */
  async function refreshSearch(manual = false) {
    await refreshPlane("search", manual);
  }

  async function refreshPlane(plane: RefreshPlane, manual: boolean) {
    if (!bridge.available || !github.value.configured) return;
    const slot = planes[plane];
    // Same-account single-flight, unchanged: a trigger while this identity's
    // refresh is already queued or in flight is a no-op.
    if (slot.queuedEpoch === epoch || slot.inFlightEpoch === epoch) return;
    await performRefresh(plane, slot, epoch, manual);
  }

  async function performRefresh(
    plane: RefreshPlane,
    slot: PlaneQueue,
    myEpoch: number,
    manual: boolean,
  ) {
    slot.queuedEpoch = myEpoch;
    try {
      // Serialize behind the in-flight fetch. The awaited promise settles
      // exactly when the backend single-flight flag is released, so the new
      // account's refresh starts after the old fetch is done — it can neither
      // collide with it nor be dropped by an InProgress refusal. One awaited
      // promise per queued refresh; no timers, no polling, no retries.
      while (slot.inFlight) {
        await slot.inFlight;
        // Superseded while queued (switch, clear, hide): abandon silently.
        if (myEpoch !== epoch) return;
      }
      // Re-validate every gate after the wait.
      if (!bridge.available || !github.value.configured) return;
      if (myEpoch !== epoch) return;
      // The transient message line belongs to the profile plane's refresh
      // cycle: a search refresh starting must not erase a profile failure
      // notice it cannot replace (search failures surface on their own).
      if (plane === "stats") message.value = null;
      const refreshingField = plane === "stats" ? "refreshing" : "searchRefreshing";
      github.value[refreshingField] = true;
      const run = (async () => {
        try {
          const view = await bridge.invoke<GithubViewState>(PLANE_COMMAND[plane], { manual });
          // A response from a superseded identity is discarded whole: it must
          // not overwrite the new identity's statistics, loading state, or
          // message, and it must not clear its not-found fact.
          if (myEpoch !== epoch) return;
          github.value = view;
          // Only a profile-plane success may clear the not-found fact: the
          // search plane has no authority over an account-existence claim.
          if (plane === "stats") {
            notFound.value = null;
            if (manual) message.value = { key: "status.githubUpdated" };
          }
        } catch (reason) {
          if (myEpoch !== epoch) return;
          github.value[refreshingField] = false;
          const kind = errorKind(reason);
          if (plane === "stats" && kind.includes("NotFound")) {
            // A terminal fact about the current identity: surfaced on the
            // card whether the fetch was manual or automatic (M2), once —
            // repeated automatic triggers re-set the same value, never a
            // growing stream of notifications.
            notFound.value = github.value.username.trim();
          } else if (manual) {
            // The search plane stays quiet on automatic failures, exactly
            // like the profile plane: a background refresh must not toast.
            message.value = githubErrorLabel(reason, github.value.username, plane);
          }
          await loadGithub();
        } finally {
          if (myEpoch === epoch) github.value[refreshingField] = false;
        }
      })();
      slot.inFlightEpoch = myEpoch;
      slot.inFlight = run;
      try {
        await run;
      } finally {
        if (slot.inFlight === run) {
          slot.inFlight = null;
          slot.inFlightEpoch = -1;
        }
      }
    } finally {
      if (slot.queuedEpoch === myEpoch) slot.queuedEpoch = -1;
    }
  }

  /**
   * The one automatic refresh trigger, shared by startup and the settings-
   * save path: a visible, configured module gets one background refresh per
   * plane whose cache is not fresh. Hidden modules, unconfigured usernames,
   * fresh caches, and rate-limited windows stay off the network — no retry
   * storm. The planes are judged independently: a fresh profile snapshot
   * never masks a stale search cache or vice versa, and the shared
   * rate-limit backoff pauses both.
   */
  function refreshIfStale(visible: boolean) {
    if (!visible || !github.value.configured) return;
    if (github.value.rateLimited) return;
    if (github.value.cacheStatus !== "fresh") void refreshGithub(false);
    if (github.value.searchCacheStatus !== "fresh") void refreshSearch(false);
  }

  return {
    github: github as Ref<GithubViewState>,
    message,
    notFound,
    loadGithub,
    refreshGithub,
    refreshSearch,
    refreshIfStale,
    switchIdentity,
    invalidate,
  };
}
