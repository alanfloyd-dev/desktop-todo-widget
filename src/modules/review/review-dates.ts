import type { ReviewPeriod } from "../../types";

/**
 * Pure review date math, extracted verbatim from `ReviewPanel.vue`.
 *
 * Every review period is a range of machine-readable `YYYY-MM-DD` task days in
 * UTC. Rendering a value in the active UI language stays in the component (it
 * needs the i18n locale); everything here is locale-free and directly unit
 * testable.
 */

/** Parses a `YYYY-MM-DD` task day as a UTC date. */
export function utcDate(value: string) {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(Date.UTC(year, month - 1, day));
}

/** Formats a UTC date back to the `YYYY-MM-DD` task-day shape. */
export function isoDate(value: Date) {
  return value.toISOString().slice(0, 10);
}

/** Moves an anchor date by `amount` days (daily), weeks (weekly), or months (monthly). */
export function shiftDate(value: string, selectedPeriod: ReviewPeriod, amount: number) {
  const date = utcDate(value);
  if (selectedPeriod === "daily") date.setUTCDate(date.getUTCDate() + amount);
  if (selectedPeriod === "weekly") date.setUTCDate(date.getUTCDate() + amount * 7);
  if (selectedPeriod === "monthly") date.setUTCMonth(date.getUTCMonth() + amount);
  return isoDate(date);
}

/** The inclusive `YYYY-MM-DD` range a period covers, from any anchor inside it. */
export function bounds(selectedPeriod: ReviewPeriod, anchor: string) {
  const selected = utcDate(anchor);
  if (selectedPeriod === "daily") return { start: anchor, end: anchor };
  if (selectedPeriod === "weekly") {
    const mondayOffset = (selected.getUTCDay() + 6) % 7;
    selected.setUTCDate(selected.getUTCDate() - mondayOffset);
    const start = isoDate(selected);
    selected.setUTCDate(selected.getUTCDate() + 6);
    return { start, end: isoDate(selected) };
  }
  selected.setUTCDate(1);
  const start = isoDate(selected);
  selected.setUTCMonth(selected.getUTCMonth() + 1);
  selected.setUTCDate(0);
  return { start, end: isoDate(selected) };
}
