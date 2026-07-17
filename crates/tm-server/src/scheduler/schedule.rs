use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};

use crate::{Result, ServerError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

impl CronSchedule {
    pub fn parse(value: &str) -> Result<Self> {
        let parts = value.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(ServerError::InvalidRequest(format!(
                "cron schedule must have five fields: {value}"
            )));
        }
        Ok(Self {
            minute: CronField::parse(parts[0], 0, 59)?,
            hour: CronField::parse(parts[1], 0, 23)?,
            day_of_month: CronField::parse(parts[2], 1, 31)?,
            month: CronField::parse(parts[3], 1, 12)?,
            day_of_week: CronField::parse(parts[4], 0, 7)?,
        })
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut candidate = after + Duration::minutes(1);
        candidate = candidate
            .with_second(0)
            .and_then(|time| time.with_nanosecond(0))?;
        for _ in 0..(366 * 24 * 60) {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate += Duration::minutes(1);
        }
        None
    }

    pub fn matches(&self, time: DateTime<Utc>) -> bool {
        self.minute.matches(time.minute())
            && self.hour.matches(time.hour())
            && self.day_of_month.matches(time.day())
            && self.month.matches(time.month())
            && self.day_of_week.matches(day_of_week_value(time))
    }
}

pub fn missed_fire_times(
    schedule: &str,
    after: DateTime<Utc>,
    now: DateTime<Utc>,
    max_catch_up: usize,
) -> Result<Vec<DateTime<Utc>>> {
    if max_catch_up == 0 || now <= after {
        return Ok(Vec::new());
    }
    let schedule = CronSchedule::parse(schedule)?;
    let mut cursor = after;
    let mut fires = Vec::new();
    while fires.len() < max_catch_up {
        let Some(next) = schedule.next_after(cursor) else {
            break;
        };
        if next > now {
            break;
        }
        fires.push(next);
        cursor = next;
    }
    Ok(fires)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CronField {
    Any,
    Values(BTreeSet<u32>),
}

impl CronField {
    fn parse(value: &str, min: u32, max: u32) -> Result<Self> {
        if value == "*" {
            return Ok(Self::Any);
        }
        let mut values = BTreeSet::new();
        for part in value.split(',') {
            let parsed = part.parse::<u32>().map_err(|_| {
                ServerError::InvalidRequest(format!("invalid cron field value {part}"))
            })?;
            if parsed < min || parsed > max {
                return Err(ServerError::InvalidRequest(format!(
                    "cron field value {parsed} out of range {min}-{max}"
                )));
            }
            values.insert(if max == 7 && parsed == 7 { 0 } else { parsed });
        }
        Ok(Self::Values(values))
    }

    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Values(values) => values.contains(&value),
        }
    }
}
fn day_of_week_value(time: DateTime<Utc>) -> u32 {
    time.weekday().num_days_from_sunday()
}
