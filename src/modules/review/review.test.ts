import { describe, expect, it } from "vitest";
import type { ReviewReport } from "../../types";
import { fetchReviewReport, type ReviewBridge } from "./bridge";
import { bounds, isoDate, shiftDate, utcDate } from "./review-dates";

describe("review date bounds", () => {
  it("a daily period covers exactly its anchor day", () => {
    expect(bounds("daily", "2026-10-18")).toEqual({ start: "2026-10-18", end: "2026-10-18" });
  });

  it("a weekly period runs Monday through Sunday", () => {
    // 2026-10-14 is a Wednesday and 2026-10-18 is a Sunday; both land on the
    // same Monday-start week.
    expect(bounds("weekly", "2026-10-14")).toEqual({ start: "2026-10-12", end: "2026-10-18" });
    expect(bounds("weekly", "2026-10-18")).toEqual({ start: "2026-10-12", end: "2026-10-18" });
  });

  it("a weekly period crossing the year boundary stays in UTC days", () => {
    // 2026-01-01 is a Thursday, so its week starts Monday 2025-12-29.
    expect(bounds("weekly", "2026-01-01")).toEqual({ start: "2025-12-29", end: "2026-01-04" });
  });

  it("a monthly period covers the first through the last day of its month", () => {
    expect(bounds("monthly", "2026-10-18")).toEqual({ start: "2026-10-01", end: "2026-10-31" });
  });

  it("a monthly period uses the real month length, leap years included", () => {
    expect(bounds("monthly", "2026-02-10")).toEqual({ start: "2026-02-01", end: "2026-02-28" });
    expect(bounds("monthly", "2028-02-10")).toEqual({ start: "2028-02-01", end: "2028-02-29" });
  });

  it("a monthly period in December ends inside its own year", () => {
    expect(bounds("monthly", "2026-12-15")).toEqual({ start: "2026-12-01", end: "2026-12-31" });
  });
});

describe("review date shifting", () => {
  it("daily moves one UTC day at a time, across month and year boundaries", () => {
    expect(shiftDate("2026-10-31", "daily", 1)).toBe("2026-11-01");
    expect(shiftDate("2026-03-01", "daily", -1)).toBe("2026-02-28");
    expect(shiftDate("2026-12-31", "daily", 1)).toBe("2027-01-01");
    expect(shiftDate("2027-01-01", "daily", -1)).toBe("2026-12-31");
  });

  it("weekly moves in whole weeks", () => {
    expect(shiftDate("2026-10-14", "weekly", 1)).toBe("2026-10-21");
    expect(shiftDate("2026-10-14", "weekly", -2)).toBe("2026-09-30");
  });

  it("monthly moves one calendar month, across the year boundary", () => {
    expect(shiftDate("2026-01-15", "monthly", 1)).toBe("2026-02-15");
    expect(shiftDate("2026-12-15", "monthly", 1)).toBe("2027-01-15");
    expect(shiftDate("2026-05-31", "monthly", -4)).toBe("2026-01-31");
  });

  it("monthly from a month-end anchor keeps the UTC month-addition semantics", () => {
    // Documents the current behavior: Jan 31 + 1 month lands on Mar 3
    // because February has no 31st. Any change here is a deliberate
    // semantics change, not a refactor.
    expect(shiftDate("2026-01-31", "monthly", 1)).toBe("2026-03-03");
  });
});

describe("task-day shapes", () => {
  it("utcDate parses in UTC and isoDate round-trips the task-day shape", () => {
    expect(utcDate("2026-10-18").getUTCDay()).toBe(0);
    expect(isoDate(utcDate("2026-10-18"))).toBe("2026-10-18");
  });
});

describe("review bridge", () => {
  /**
   * The bridge's `invoke` is runtime-generic (any result type); a test double
   * with concrete return types needs the cast to fit that signature. The same
   * idiom as the weather controller tests.
   */
  function fakeBridge(response: ReviewReport) {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    const bridge: ReviewBridge = {
      invoke: ((command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args: args ?? {} });
        return Promise.resolve(response);
      }) as unknown as ReviewBridge["invoke"],
    };
    return { bridge, calls };
  }

  const report = {
    period: "weekly",
    anchorDate: "2026-10-12",
    periodStart: "2026-10-12",
    periodEnd: "2026-10-18",
    currentTaskDay: "2026-10-18",
    totals: { planned: 1, completed: 1, carried: 0, cancelled: 0, pending: 0 },
    days: [],
    categories: [],
    tasks: [],
  } satisfies ReviewReport;

  it("forwards the pinned command name and both arguments, and passes the report through", async () => {
    const { bridge, calls } = fakeBridge(report);
    const result = await fetchReviewReport(bridge, "weekly", "2026-10-12");
    expect(calls).toEqual([
      { command: "review_report", args: { period: "weekly", anchorDate: "2026-10-12" } },
    ]);
    expect(result).toBe(report);
  });

  it("a first open without an anchor passes null through to the backend", async () => {
    const { bridge, calls } = fakeBridge(report);
    await fetchReviewReport(bridge, "daily", null);
    expect(calls).toEqual([
      { command: "review_report", args: { period: "daily", anchorDate: null } },
    ]);
  });
});
