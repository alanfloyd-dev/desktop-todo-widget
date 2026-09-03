use crate::{database::Database, settings::AppState, task_day};
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::State;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewPeriod {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCounts {
    pub planned: i64,
    pub completed: i64,
    pub carried: i64,
    pub cancelled: i64,
    pub pending: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDay {
    pub date: String,
    pub counts: StatusCounts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCategory {
    pub category_id: Option<String>,
    pub label: String,
    pub counts: StatusCounts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTask {
    pub id: String,
    pub title: String,
    pub category_name: Option<String>,
    pub status: String,
    pub scheduled_date: String,
    pub completed_at: Option<i64>,
    pub carried_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReport {
    pub period: ReviewPeriod,
    pub anchor_date: String,
    pub period_start: String,
    pub period_end: String,
    pub current_task_day: String,
    pub totals: StatusCounts,
    pub days: Vec<ReviewDay>,
    pub categories: Vec<ReviewCategory>,
    pub tasks: Vec<ReviewTask>,
}

impl StatusCounts {
    fn record(&mut self, status: &str) -> Result<(), String> {
        self.planned += 1;
        match status {
            "completed" => self.completed += 1,
            "carried" => self.carried += 1,
            "cancelled" => self.cancelled += 1,
            "pending" => self.pending += 1,
            _ => return Err(format!("unsupported task status: {status}")),
        }
        Ok(())
    }
}

impl Database {
    pub fn review_report(
        &self,
        period: ReviewPeriod,
        anchor_date: NaiveDate,
        current_task_day: NaiveDate,
    ) -> Result<ReviewReport, String> {
        let (start, end) = period_bounds(period, anchor_date)?;
        let start_text = format_date(start);
        let end_text = format_date(end);
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let mut statement = connection
            .prepare(
                "SELECT t.id, t.title, c.name, t.status, t.scheduled_date,
                        t.completed_at, t.carried_from, t.category_id
                 FROM tasks t
                 LEFT JOIN categories c ON c.id = t.category_id
                 WHERE t.scheduled_date BETWEEN ?1 AND ?2
                 ORDER BY t.scheduled_date,
                   CASE t.status
                     WHEN 'pending' THEN 0
                     WHEN 'completed' THEN 1
                     WHEN 'carried' THEN 2
                     ELSE 3
                   END,
                   t.sort_order, t.created_at, t.id",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![start_text, end_text], |row| {
                Ok((
                    ReviewTask {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        category_name: row.get(2)?,
                        status: row.get(3)?,
                        scheduled_date: row.get(4)?,
                        completed_at: row.get(5)?,
                        carried_from: row.get(6)?,
                    },
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|error| error.to_string())?;

        let mut day_counts = BTreeMap::new();
        let mut day = start;
        while day <= end {
            day_counts.insert(format_date(day), StatusCounts::default());
            day = day
                .succ_opt()
                .ok_or_else(|| "review period exceeds supported date range".to_string())?;
        }

        let mut totals = StatusCounts::default();
        let mut category_counts: BTreeMap<String, (Option<String>, String, StatusCounts)> =
            BTreeMap::new();
        let mut tasks = Vec::new();
        for row in rows {
            let (task, category_id) = row.map_err(|error| error.to_string())?;
            totals.record(&task.status)?;
            day_counts
                .get_mut(&task.scheduled_date)
                .ok_or_else(|| "task date fell outside review period".to_string())?
                .record(&task.status)?;

            let category_key = category_id.clone().unwrap_or_default();
            let category_label = task
                .category_name
                .clone()
                .unwrap_or_else(|| "Uncategorized".to_string());
            category_counts
                .entry(category_key)
                .or_insert_with(|| (category_id, category_label, StatusCounts::default()))
                .2
                .record(&task.status)?;
            tasks.push(task);
        }

        let days = day_counts
            .into_iter()
            .map(|(date, counts)| ReviewDay { date, counts })
            .collect();
        let mut categories: Vec<_> = category_counts
            .into_values()
            .map(|(category_id, label, counts)| ReviewCategory {
                category_id,
                label,
                counts,
            })
            .collect();
        categories.sort_by(|left, right| {
            right
                .counts
                .planned
                .cmp(&left.counts.planned)
                .then_with(|| left.label.cmp(&right.label))
        });

        Ok(ReviewReport {
            period,
            anchor_date: format_date(anchor_date),
            period_start: format_date(start),
            period_end: format_date(end),
            current_task_day: format_date(current_task_day),
            totals,
            days,
            categories,
            tasks,
        })
    }
}

#[tauri::command]
pub fn review_report(
    period: ReviewPeriod,
    anchor_date: Option<String>,
    state: State<'_, AppState>,
) -> Result<ReviewReport, String> {
    let current_task_day = task_day::current_task_day(&state.snapshot()?.day_rollover)?;
    let anchor_date = anchor_date
        .as_deref()
        .map(parse_date)
        .transpose()?
        .unwrap_or(current_task_day);
    state
        .database
        .review_report(period, anchor_date, current_task_day)
}

fn period_bounds(
    period: ReviewPeriod,
    anchor_date: NaiveDate,
) -> Result<(NaiveDate, NaiveDate), String> {
    match period {
        ReviewPeriod::Daily => Ok((anchor_date, anchor_date)),
        ReviewPeriod::Weekly => {
            let start = anchor_date
                .checked_sub_signed(Duration::days(i64::from(
                    anchor_date.weekday().num_days_from_monday(),
                )))
                .ok_or_else(|| "review week is outside the supported date range".to_string())?;
            let end = start
                .checked_add_signed(Duration::days(6))
                .ok_or_else(|| "review week is outside the supported date range".to_string())?;
            Ok((start, end))
        }
        ReviewPeriod::Monthly => {
            let start = anchor_date
                .with_day(1)
                .ok_or_else(|| "invalid review month".to_string())?;
            let (next_year, next_month) = if start.month() == 12 {
                (start.year() + 1, 1)
            } else {
                (start.year(), start.month() + 1)
            };
            let next_start = NaiveDate::from_ymd_opt(next_year, next_month, 1)
                .ok_or_else(|| "review month is outside the supported date range".to_string())?;
            let end = next_start
                .pred_opt()
                .ok_or_else(|| "review month is outside the supported date range".to_string())?;
            Ok((start, end))
        }
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| "review date must use YYYY-MM-DD format".to_string())
}

fn format_date(value: NaiveDate) -> String {
    value.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::{period_bounds, ReviewPeriod, StatusCounts};
    use crate::database::Database;
    use chrono::NaiveDate;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    #[test]
    fn daily_report_counts_each_raw_status_once() {
        let database = Database::in_memory().expect("database");
        let task_day = date("2026-09-01");
        let completed = database.add_task("Done", None, task_day).expect("add");
        let carried = database.add_task("Carry", None, task_day).expect("add");
        let cancelled = database.add_task("Cancel", None, task_day).expect("add");
        database
            .add_task("Still pending", None, task_day)
            .expect("add");
        database
            .toggle_task_completed(&completed.id)
            .expect("complete");
        database.carry_task(&carried.id).expect("carry");
        database.cancel_task(&cancelled.id).expect("cancel");

        let report = database
            .review_report(ReviewPeriod::Daily, task_day, task_day)
            .expect("report");
        assert_eq!(
            report.totals,
            StatusCounts {
                planned: 4,
                completed: 1,
                carried: 1,
                cancelled: 1,
                pending: 1,
            }
        );
        assert!(report
            .tasks
            .iter()
            .any(|task| { task.status == "completed" && task.completed_at.is_some() }));
    }

    #[test]
    fn carry_source_and_successor_belong_to_their_own_task_days() {
        let database = Database::in_memory().expect("database");
        let source_day = date("2026-09-01");
        let source = database
            .add_task("Continue", None, source_day)
            .expect("add");
        let successor = database.carry_task(&source.id).expect("carry");
        database
            .toggle_task_completed(&successor.id)
            .expect("complete successor");

        let source_report = database
            .review_report(ReviewPeriod::Daily, source_day, source_day)
            .expect("source report");
        assert_eq!(source_report.totals.planned, 1);
        assert_eq!(source_report.totals.carried, 1);
        assert_eq!(source_report.totals.completed, 0);

        let successor_day = date("2026-09-02");
        let successor_report = database
            .review_report(ReviewPeriod::Daily, successor_day, successor_day)
            .expect("successor report");
        assert_eq!(successor_report.totals.planned, 1);
        assert_eq!(successor_report.totals.completed, 1);
        assert_eq!(
            successor_report.tasks[0].carried_from.as_deref(),
            Some(source.id.as_str())
        );
    }

    #[test]
    fn weekly_report_uses_monday_through_sunday_and_includes_empty_days() {
        let database = Database::in_memory().expect("database");
        database
            .add_task("Monday", None, date("2026-08-31"))
            .expect("add");
        database
            .add_task("Sunday", None, date("2026-09-06"))
            .expect("add");
        database
            .add_task("Outside", None, date("2026-09-07"))
            .expect("add");

        let report = database
            .review_report(ReviewPeriod::Weekly, date("2026-09-02"), date("2026-09-02"))
            .expect("report");
        assert_eq!(report.period_start, "2026-08-31");
        assert_eq!(report.period_end, "2026-09-06");
        assert_eq!(report.days.len(), 7);
        assert_eq!(report.totals.planned, 2);
    }

    #[test]
    fn monthly_bounds_cover_leap_day() {
        assert_eq!(
            period_bounds(ReviewPeriod::Monthly, date("2028-02-12")).expect("bounds"),
            (date("2028-02-01"), date("2028-02-29"))
        );
    }

    #[test]
    fn category_breakdown_is_factual_and_includes_uncategorized() {
        let database = Database::in_memory().expect("database");
        let task_day = date("2026-09-01");
        let category = database.create_category("Research").expect("category");
        database
            .add_task("Read", Some(&category.id), task_day)
            .expect("add");
        database
            .add_task("Loose note", None, task_day)
            .expect("add");

        let report = database
            .review_report(ReviewPeriod::Daily, task_day, task_day)
            .expect("report");
        assert_eq!(report.categories.len(), 2);
        assert!(report
            .categories
            .iter()
            .any(|item| item.label == "Research"));
        assert!(report
            .categories
            .iter()
            .any(|item| item.label == "Uncategorized"));
    }
}
