# Review and Reports

Review is a read-only reflection of local task history. It describes what is recorded without assigning a score, comparing performance, or generating advice.

## Periods

- Daily Review uses one task day.
- Weekly Review uses Monday through Sunday.
- Monthly Review uses the calendar month containing the selected task day.

The default selected date is the current task day calculated by Rust with the existing rollover setting. With the default 04:00 rollover, a review opened at 02:00 still selects the previous calendar date. The header's calendar date does not override this rule.

Each period reports Planned, Completed, Carried, Cancelled, and Pending. Weekly and Monthly views also show factual distributions by task day and current category. Daily Review lists the individual task facts for that day.

## Source of truth

Every view is dynamically aggregated from `tasks.scheduled_date`, `tasks.status`, `tasks.completed_at`, the nullable category relation, and `tasks.carried_from`. No daily, weekly, or monthly result is saved to SQLite.

Planned is the count of raw task rows scheduled inside the requested range. Each row contributes to one status count. A carried source remains `carried` on its original task day; the successor is a separate row and is counted only on its own scheduled task day. This prevents a successor completion from rewriting or double-counting the original day's history.

## Product boundary

The main Widget exposes one lightweight `Review` entry beside the current task-day label. The Review surface is read-only: it does not edit, carry, cancel, reorder, or create tasks. It contains no completion rate, productivity or efficiency score, streak, achievement, comparison judgment, or AI-generated summary.
