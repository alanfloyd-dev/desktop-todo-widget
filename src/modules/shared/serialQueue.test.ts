import { describe, expect, it } from "vitest";
import { setModuleCollapsed } from "../layout";
import { createSerialQueue } from "./serialQueue";

/** A promise resolved later by the test, with its public resolver. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("createSerialQueue", () => {
  it("starts a task only after the previous one settles, whatever the resolution order", async () => {
    const queue = createSerialQueue();
    const first = deferred<string>();
    const started: string[] = [];

    const run1 = queue(() => {
      started.push("first");
      return first.promise;
    });
    const run2 = queue(() => {
      started.push("second");
      return Promise.resolve("second-result");
    });

    // The second task is held even though its own promise is already ready:
    // ordering is by submission, not by which underlying promise settles first.
    await Promise.resolve();
    await Promise.resolve();
    expect(started).toEqual(["first"]);

    first.resolve("first-result");
    await expect(run1).resolves.toBe("first-result");
    await expect(run2).resolves.toBe("second-result");
    expect(started).toEqual(["first", "second"]);
  });

  it("hands each caller its own outcome and lets a failure through without stranding the queue", async () => {
    const queue = createSerialQueue();
    const gate = deferred<void>();
    const order: string[] = [];

    const failing = queue(async () => {
      await gate.promise;
      order.push("failed");
      throw new Error("request failed");
    });
    const following = queue(async () => {
      order.push("following");
      return "following-result";
    });

    gate.resolve();
    // …the caller still sees the rejection…
    await expect(failing).rejects.toThrow("request failed");
    // …and the task behind the failure ran.
    await expect(following).resolves.toBe("following-result");
    expect(order).toEqual(["failed", "following"]);
  });
});

describe("serialized module toggles (D1 concurrency gate)", () => {
  /** A raw stored layout document, exactly what `state.settings.moduleLayout` holds. */
  type RawDocument = {
    version: 1;
    entries: Array<{ id: string; order: number; colSpan: number; collapsed?: boolean }>;
  };

  /**
   * The App.vue collapse path, reduced to its deterministic parts: the app
   * state's raw stored document, a toggle that builds its patch from the
   * document *at execution time*, and a response application that replaces
   * the document. Native invoke is simulated; the browser-preview
   * `Object.assign` branch behaves the same way.
   */
  function simulateToggle(
    stored: RawDocument,
    id: "todo" | "shortcuts",
    invokeDelay: Promise<void>,
  ) {
    return async () => {
      const collapsedNow = stored.entries.find((entry) => entry.id === id)?.collapsed === true;
      const patch = setModuleCollapsed(stored, id, !collapsedNow);
      expect(patch).not.toBeNull();
      await invokeDelay;
      Object.assign(stored, patch);
      return patch;
    };
  }

  it("two rapid toggles both count: the second patch is built from the applied first response", async () => {
    const stored: RawDocument = {
      version: 1,
      entries: [{ id: "todo", order: 1, colSpan: 1 }],
    };
    const firstInvoke = deferred<void>();
    const secondInvoke = deferred<void>();
    const queue = createSerialQueue();

    // Two clicks in quick succession, the first response still in flight.
    const first = queue(simulateToggle(stored, "todo", firstInvoke.promise));
    const second = queue(simulateToggle(stored, "todo", secondInvoke.promise));
    firstInvoke.resolve();
    await first;
    secondInvoke.resolve();
    await second;

    // Each click toggled exactly once: collapsed then expanded again. Without
    // the queue both patches are built from the same pre-toggle document, both
    // collapse, and the second click is lost — the deterministic failure this
    // serialization removes.
    expect(stored.entries[0].collapsed).toBe(false);
  });

  it("without the queue, both patches read the same stale document and the second click is lost", async () => {
    const stored: RawDocument = {
      version: 1,
      entries: [{ id: "todo", order: 1, colSpan: 1 }],
    };
    const firstInvoke = deferred<void>();
    const secondInvoke = deferred<void>();

    // The pre-fix shape: both toggles run concurrently, and both read
    // `stored` before either response has been applied.
    const first = simulateToggle(stored, "todo", firstInvoke.promise);
    const second = simulateToggle(stored, "todo", secondInvoke.promise);
    const pending = [first(), second()];
    firstInvoke.resolve();
    secondInvoke.resolve();
    const patches = await Promise.all(pending);

    // Both computed collapsed=true from the same pre-toggle document: two
    // clicks collapse once. (The objects prove the lost click directly.)
    expect(patches[0]?.entries[0].collapsed).toBe(true);
    expect(patches[1]?.entries[0].collapsed).toBe(true);
  });

  it("toggles of different modules compose through the queue without dropping either", async () => {
    const stored: RawDocument = {
      version: 1,
      entries: [
        { id: "todo", order: 1, colSpan: 1 },
        { id: "shortcuts", order: 2, colSpan: 1 },
      ],
    };
    const firstInvoke = deferred<void>();
    const secondInvoke = deferred<void>();
    const queue = createSerialQueue();

    const first = queue(simulateToggle(stored, "todo", firstInvoke.promise));
    const second = queue(simulateToggle(stored, "shortcuts", secondInvoke.promise));
    firstInvoke.resolve();
    await first;
    secondInvoke.resolve();
    await second;

    expect(stored.entries.find((entry) => entry.id === "todo")?.collapsed).toBe(true);
    expect(stored.entries.find((entry) => entry.id === "shortcuts")?.collapsed).toBe(true);
    // The untouched entry keeps its exact shape.
    expect(stored.entries[0]).toEqual({
      id: "todo",
      order: 1,
      colSpan: 1,
      collapsed: true,
    });
  });
});
