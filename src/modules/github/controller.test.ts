import { describe, expect, it, vi, type Mock } from "vitest";
import {
  browserGithubState,
  createGithubController,
  githubErrorLabel,
  type GithubBridge,
} from "./controller";
import type { GithubViewState } from "../../types";

/**
 * The bridge's `invoke` is runtime-generic (any result type); a test double
 * with concrete return types needs the cast to fit that signature.
 */
function asInvoke(mock: Mock<(command: string) => Promise<unknown>>): GithubBridge["invoke"] {
  return mock as unknown as GithubBridge["invoke"];
}

function snapshotState(overrides: Partial<GithubViewState> = {}): GithubViewState {
  return {
    configured: true,
    username: "octocat",
    snapshot: {
      username: "octocat",
      followers: 24799,
      stars: 21148,
      fetchedAt: 1_800_000_000,
    },
    cacheStatus: "fresh",
    cacheAgeSeconds: 30,
    refreshing: false,
    rateLimited: false,
    retryAfterSeconds: null,
    lastRefresh: "success",
    ...overrides,
  };
}

function accountAState(): GithubViewState {
  return snapshotState({
    username: "alanfloyd-dev",
    snapshot: { username: "alanfloyd-dev", followers: 117, stars: 19, fetchedAt: 1 },
  });
}

/**
 * A bridge whose refresh invocations are queued under the test's control —
 * the deterministic stand-in for network completion order. `github_state`
 * reads are local backend reads, so they answer immediately like the real
 * command does.
 */
function sequenceBridge(stateAnswer: () => GithubViewState | Promise<GithubViewState> = () => snapshotState()) {
  type Entry = {
    command: string;
    done: boolean;
    resolve?: (value: unknown) => void;
    reject?: (reason: unknown) => void;
  };
  const calls: Entry[] = [];
  const bridge: GithubBridge = {
    available: true,
    invoke: ((command: string) => {
      const entry: Entry = { command, done: false };
      calls.push(entry);
      if (command === "github_state") {
        entry.done = true;
        return Promise.resolve(stateAnswer());
      }
      return new Promise<unknown>((resolve, reject) => {
        entry.resolve = resolve as (value: unknown) => void;
        entry.reject = reject;
      });
    }) as GithubBridge["invoke"],
  };
  const refreshCount = () => calls.filter((c) => c.command === "refresh_github").length;
  const indexOf = (command: string, occurrence = 0) => {
    let seen = 0;
    for (let index = 0; index < calls.length; index += 1) {
      if (calls[index].command !== command || calls[index].done) continue;
      if (seen === occurrence) return index;
      seen += 1;
    }
    return -1;
  };
  return {
    bridge,
    calls,
    refreshCount,
    async settle(command: string, value: unknown, occurrence = 0) {
      const entry = calls[indexOf(command, occurrence)];
      entry.done = true;
      entry.resolve!(value);
    },
    async fail(command: string, reason: unknown, occurrence = 0) {
      const entry = calls[indexOf(command, occurrence)];
      entry.done = true;
      entry.reject!(reason);
    },
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("github controller — basics", () => {
  it("a manual refresh rides the refresh_github command once and reports success", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState();

    const pending = controller.refreshGithub(true);
    expect(controller.github.value.refreshing).toBe(true);
    await harness.settle("refresh_github", snapshotState({ cacheStatus: "fresh" }));
    await pending;

    expect(harness.refreshCount()).toBe(1);
    expect(harness.calls[0].command).toBe("refresh_github");
    expect(controller.github.value.refreshing).toBe(false);
    expect(controller.message.value).toEqual({ key: "status.githubUpdated" });
  });

  it("a refresh is refused before any invoke while unconfigured (zero-request gate)", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ configured: false, username: "" });

    await controller.refreshGithub(true);

    expect(harness.calls.length).toBe(0);
    expect(controller.github.value.refreshing).toBe(false);
  });

  it("a refresh is single-flighted: a same-account trigger while one is in flight is a no-op", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState();

    const first = controller.refreshGithub(true);
    const second = controller.refreshGithub(true);
    await harness.settle("refresh_github", snapshotState());
    await Promise.all([first, second]);

    expect(harness.refreshCount()).toBe(1);
  });

  it("loads state through github_state and falls back to the sample state on failure", async () => {
    // A rejecting `github_state` answer drives the catch path.
    const harness = sequenceBridge(() => Promise.reject("backend unavailable"));
    const controller = createGithubController(harness.bridge);
    await controller.loadGithub();
    // The weather controller's fallback rule: a failed state read renders
    // the sample card rather than blanking the module.
    expect(controller.github.value.configured).toBe(true);
    expect(controller.github.value.snapshot).not.toBeNull();

    const healthy = sequenceBridge();
    const ok = createGithubController(healthy.bridge);
    await ok.loadGithub();
    expect(ok.github.value.username).toBe("octocat");
  });

  it("the preview fallback carries a sample and the native fallback starts unconfigured", () => {
    const preview = browserGithubState(false);
    expect(preview.configured).toBe(true);
    expect(preview.snapshot?.followers).toBeGreaterThan(0);

    const native = browserGithubState(true);
    expect(native.configured).toBe(false);
    expect(native.snapshot).toBeNull();
  });
});

describe("github controller — identity-aware refresh (M1)", () => {
  it("a stale success never overwrites the switched identity's state or message", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const pending = controller.refreshGithub(true); // A's fetch, in flight
    controller.switchIdentity("octocat"); // user saves account B mid-flight
    await harness.settle("refresh_github", accountAState()); // A completes late
    await pending;

    expect(controller.github.value.username).toBe("octocat");
    expect(controller.github.value.snapshot).toBeNull();
    expect(controller.github.value.refreshing).toBe(false);
    // Even though the superseded refresh was manual, its success message
    // belongs to the old identity and must not surface for B.
    expect(controller.message.value).toBeNull();
  });

  it("a stale failure never overwrites the switched identity's state", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const pending = controller.refreshGithub(true);
    controller.switchIdentity("octocat");
    await harness.fail("refresh_github", { kind: "Timeout" });
    await pending;

    expect(controller.github.value.username).toBe("octocat");
    expect(controller.github.value.snapshot).toBeNull();
    expect(controller.message.value).toBeNull();
  });

  it("the new account's refresh waits for the in-flight fetch and then runs (M1 recovery)", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const aRefresh = controller.refreshGithub(false); // A's fetch in flight
    controller.switchIdentity("octocat"); // B saved while A is in flight
    const bRefresh = controller.refreshGithub(false); // B's trigger hits the queue
    await tick();

    // B's fetch has NOT been dropped: it is serialized behind A's, not lost —
    // and it has not been issued in parallel either.
    expect(harness.refreshCount()).toBe(1);

    await harness.settle("refresh_github", accountAState()); // A settles
    await aRefresh;
    await tick();
    expect(harness.refreshCount()).toBe(2); // B's fetch must run after A settles

    await harness.settle("refresh_github", snapshotState()); // B settles
    await bRefresh;
    expect(controller.github.value.username).toBe("octocat");
    expect(controller.github.value.snapshot?.followers).toBe(24799);
  });

  it("switching to C invalidates the work queued for B", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const aRefresh = controller.refreshGithub(false);
    controller.switchIdentity("bob");
    const bRefresh = controller.refreshGithub(false); // queued behind A
    await tick();
    controller.switchIdentity("carol"); // B superseded before A settles

    await harness.settle("refresh_github", accountAState());
    await aRefresh;
    await bRefresh;
    await tick();

    // B's queued fetch was abandoned: only A's fetch was ever issued, and
    // the card shows C's reset state.
    expect(harness.refreshCount()).toBe(1);
    expect(controller.github.value.username).toBe("carol");
    expect(controller.github.value.snapshot).toBeNull();
  });

  it("clearing the username invalidates the pending refresh and fetches nothing", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const pending = controller.refreshGithub(false);
    controller.switchIdentity(""); // username cleared mid-flight
    await harness.settle("refresh_github", accountAState());
    await pending;
    await controller.refreshGithub(false); // must be refused: unconfigured

    expect(harness.refreshCount()).toBe(1);
    expect(controller.github.value.configured).toBe(false);
    expect(controller.github.value.username).toBe("");
  });

  it("hiding the module invalidates the pending refresh intent", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const pending = controller.refreshGithub(false);
    controller.invalidate(); // module hidden while the fetch is in flight
    await harness.settle("refresh_github", accountAState());
    await pending;

    // The stale response was discarded; a re-show must schedule fresh work.
    expect(harness.refreshCount()).toBe(1);
    expect(controller.github.value.username).toBe("alanfloyd-dev");
  });
});

describe("github controller — automatic triggers (M1 props 2/4)", () => {
  it("a fresh cache means no redundant request", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ cacheStatus: "fresh" });

    controller.refreshIfStale(true);
    await tick();

    expect(harness.calls.length).toBe(0);
  });

  it("a rate-limited window means no retry storm", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({
      cacheStatus: "stale",
      rateLimited: true,
      retryAfterSeconds: 120,
    });

    for (let attempt = 0; attempt < 3; attempt += 1) {
      controller.refreshIfStale(true);
    }
    await tick();

    // Rate-limited triggers must not reach the bridge.
    expect(harness.calls.length).toBe(0);
  });

  it("a stale visible configured cache gets exactly one background refresh", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ cacheStatus: "stale" });

    controller.refreshIfStale(true);
    controller.refreshIfStale(true); // a second trigger before completion
    await tick();
    expect(harness.refreshCount()).toBe(1); // same-account triggers deduplicate
    await harness.settle("refresh_github", snapshotState({ cacheStatus: "fresh" }));

    controller.refreshIfStale(false); // hidden module
    controller.refreshIfStale(true); // fresh again
    await tick();
    expect(harness.refreshCount()).toBe(1);
  });
});

describe("github controller — error surfacing (M2/M3)", () => {
  it("an automatic NotFound sets the not-found fact; a later success clears it", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ cacheStatus: "stale" });

    const refresh = controller.refreshGithub(false); // automatic
    await harness.fail("refresh_github", { kind: "NotFound" });
    await refresh;

    expect(controller.notFound.value).toBe("octocat");
    expect(controller.message.value).toBeNull(); // automatic failures stay quiet

    const retry = controller.refreshGithub(false); // next trigger
    await harness.settle("refresh_github", snapshotState({ cacheStatus: "fresh" }));
    await retry;
    expect(controller.notFound.value).toBeNull();
    expect(controller.github.value.snapshot).not.toBeNull();
  });

  it("a manual refresh maps cooldown, in-progress, and rate-limit kinds to distinct messages", async () => {
    expect(githubErrorLabel({ kind: "Cooldown" }, "x").key).toBe("github.error.cooldown");
    expect(githubErrorLabel({ kind: "InProgress" }, "x").key).toBe("github.error.inProgress");
    expect(githubErrorLabel({ kind: "RateLimited", message: 300 }, "x")).toEqual({
      key: "github.rateLimited",
      params: { minutes: 5 },
    });
    expect(githubErrorLabel({ kind: "Network" }, "x").key).toBe("github.error.generic");
  });

  it("a manual failure keeps the snapshot and re-syncs from the backend cache", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ cacheStatus: "stale", cacheAgeSeconds: 3600 });

    const pending = controller.refreshGithub(true);
    await harness.fail("refresh_github", { kind: "RateLimited", message: 300 });
    await pending;

    // The failure re-syncs from the backend cache (the harness's
    // `github_state` answer) instead of blanking the card.
    expect(controller.github.value).toEqual(snapshotState());
    expect(controller.message.value).toEqual({
      key: "github.rateLimited",
      params: { minutes: 5 },
    });
  });
});

describe("github controller — account-scoped messages (M4)", () => {
  it("switching identity clears the message and the not-found fact", async () => {
    const harness = sequenceBridge();
    const controller = createGithubController(harness.bridge);
    controller.github.value = snapshotState({ cacheStatus: "stale" });

    const pending = controller.refreshGithub(true);
    await harness.fail("refresh_github", { kind: "NotFound" });
    await pending;
    // M2: the not-found fact lives on the card body (via `notFound`), not in
    // the transient message line.
    expect(controller.notFound.value).toBe("octocat");
    expect(controller.message.value).toBeNull();

    controller.switchIdentity("someone-else");
    expect(controller.message.value).toBeNull();
    expect(controller.notFound.value).toBeNull();
  });

  it("a state read that started under a previous identity never lands", async () => {
    // The harness answers github_state with A's state no matter when it is
    // asked — the read was issued while A was configured.
    const harness = sequenceBridge(() => accountAState());
    const controller = createGithubController(harness.bridge);
    controller.github.value = accountAState();

    const load = controller.loadGithub(); // reads while A is configured
    controller.switchIdentity("octocat"); // B saved before the read returns
    await tick();
    await load;

    // The A-answer arrives late: the epoch check must discard it whole, or
    // A's snapshot would flash onto B's card.
    expect(controller.github.value.username).toBe("octocat");
    expect(controller.github.value.snapshot).toBeNull();
  });
});
