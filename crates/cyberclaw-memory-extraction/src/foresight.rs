//! Foresight extraction trait.
//! Borrows from EverOS ForesightExtractor: up to 10 predictive associations per MemCell.

use crate::memcell::MemCell;
use chrono::NaiveDate;
use cyberclaw_core::ids::ActorId;
use cyberclaw_core::memory_context::ForesightProjection;

/// Trait for extracting forward-looking predictions from a MemCell.
#[async_trait::async_trait]
pub trait ForesightExtractor: Send + Sync {
    /// Extract up to 10 forward-looking predictions from a MemCell.
    async fn extract(
        &self,
        memcell: &MemCell,
        target_actor: &ActorId,
    ) -> anyhow::Result<Vec<ForesightProjection>>;
}

/// Calculate end_date from start_date + duration_days.
pub fn end_date_from_duration(start: NaiveDate, duration_days: u32) -> NaiveDate {
    start + chrono::Duration::days(i64::from(duration_days))
}

/// Calculate duration_days from start_date and end_date.
pub fn duration_from_dates(start: NaiveDate, end: NaiveDate) -> Option<u32> {
    let days = (end - start).num_days();
    if days >= 0 {
        u32::try_from(days).ok()
    } else {
        None
    }
}

/// Validate and clean a date string (YYYY-MM-DD format).
pub fn clean_date_string(date_str: &str) -> Option<NaiveDate> {
    let cleaned: String = date_str
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    NaiveDate::parse_from_str(&cleaned, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_end_date_from_duration() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 15).unwrap();
        let end = end_date_from_duration(start, 5);
        assert_eq!(end, NaiveDate::from_ymd_opt(2026, 4, 20).unwrap());
    }

    #[test]
    fn test_duration_from_dates() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 15).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        assert_eq!(duration_from_dates(start, end), Some(5));
    }

    #[test]
    fn test_duration_from_dates_negative() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 4, 15).unwrap();
        assert_eq!(duration_from_dates(start, end), None);
    }

    #[test]
    fn test_clean_date_string_valid() {
        assert_eq!(
            clean_date_string("2026-04-15"),
            Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap())
        );
    }

    #[test]
    fn test_clean_date_string_with_junk() {
        assert_eq!(
            clean_date_string("2026-04-15 "),
            Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap())
        );
    }

    #[test]
    fn test_clean_date_string_invalid() {
        assert_eq!(clean_date_string("not-a-date"), None);
        assert_eq!(clean_date_string("2026-13-45"), None);
    }
}
