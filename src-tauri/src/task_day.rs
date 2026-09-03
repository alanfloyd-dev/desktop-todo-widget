use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime};

pub fn parse_rollover(value: &str) -> Result<NaiveTime, String> {
    if value.len() != 5 || value.as_bytes().get(2) != Some(&b':') {
        return Err("day rollover must use 24-hour HH:MM format".to_string());
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| "day rollover must use 24-hour HH:MM format".to_string())
}

pub fn task_day_at(local_time: NaiveDateTime, rollover: NaiveTime) -> NaiveDate {
    // A task day changes at the configured rollover, not at midnight. Keeping
    // this calculation centralized prevents the header's calendar date from
    // leaking into persistence queries around the boundary.
    if local_time.time() < rollover {
        local_time
            .date()
            .pred_opt()
            .unwrap_or_else(|| local_time.date())
    } else {
        local_time.date()
    }
}

pub fn current_task_day(rollover: &str) -> Result<NaiveDate, String> {
    Ok(task_day_at(
        Local::now().naive_local(),
        parse_rollover(rollover)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{parse_rollover, task_day_at};
    use chrono::{NaiveDate, NaiveDateTime};

    fn at(value: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").expect("time")
    }

    #[test]
    fn midnight_rollover_starts_at_midnight() {
        let rollover = parse_rollover("00:00").expect("rollover");
        assert_eq!(
            task_day_at(at("2026-08-29 00:00:00"), rollover),
            NaiveDate::from_ymd_opt(2026, 8, 29).expect("date")
        );
    }

    #[test]
    fn four_am_boundary_is_exact() {
        let rollover = parse_rollover("04:00").expect("rollover");
        assert_eq!(
            task_day_at(at("2026-08-29 03:59:59"), rollover),
            NaiveDate::from_ymd_opt(2026, 8, 28).expect("date")
        );
        assert_eq!(
            task_day_at(at("2026-08-29 04:00:00"), rollover),
            NaiveDate::from_ymd_opt(2026, 8, 29).expect("date")
        );
    }

    #[test]
    fn rollover_crosses_month_and_year_boundaries() {
        let rollover = parse_rollover("04:00").expect("rollover");
        assert_eq!(
            task_day_at(at("2026-05-01 02:00:00"), rollover),
            NaiveDate::from_ymd_opt(2026, 4, 30).expect("date")
        );
        assert_eq!(
            task_day_at(at("2027-01-01 01:00:00"), rollover),
            NaiveDate::from_ymd_opt(2026, 12, 31).expect("date")
        );
    }

    #[test]
    fn rollover_format_is_validated() {
        assert!(parse_rollover("4:00").is_err());
        assert!(parse_rollover("25:00").is_err());
    }
}
