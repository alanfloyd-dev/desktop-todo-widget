/**
 * A minimal promise chain: tasks enqueued on one queue run strictly in
 * submission order, each starting only after the previous one settles —
 * resolved or rejected.
 *
 * This is the frontend serialization the D1 concurrency gate asks for on the
 * collapse path. Two rapid toggles must not both read the same pre-toggle
 * stored document and compute the same patch: held behind the queue, the
 * second toggle's patch is built only after the first response has been
 * applied to the app state, so every patch is constructed from the state the
 * user actually sees.
 *
 * Deliberately not a mutex, an abort controller, or a latest-only switch: a
 * click is a real user intention and must eventually run, and a failed
 * request must not silently drop the ones behind it.
 */
export type SerializedTask<T> = () => Promise<T>;

export function createSerialQueue() {
  let tail: Promise<unknown> = Promise.resolve();
  return function enqueue<T>(task: SerializedTask<T>): Promise<T> {
    // Run the task whichever way the predecessor settled; hand the caller
    // the task's own outcome while chaining the queue onto a settled-safe
    // continuation so one failure cannot strand the tasks behind it.
    const run = tail.then(task, task);
    tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  };
}
