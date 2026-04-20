use chrono::{Datelike, Duration, NaiveDate, Utc};

use crate::weather::market_close_utc;

pub struct EventService;

impl EventService {
    pub fn resolve_target_date(override_date: Option<NaiveDate>) -> NaiveDate {
        if let Some(date) = override_date {
            return date;
        }

        let now = Utc::now();
        let today = now.date_naive();
        if now < market_close_utc(today) {
            today
        } else {
            today + Duration::days(1)
        }
    }

    pub fn build_event_slug(city: &str, date: NaiveDate) -> String {
        let month = match date.month() {
            1 => "january",
            2 => "february",
            3 => "march",
            4 => "april",
            5 => "may",
            6 => "june",
            7 => "july",
            8 => "august",
            9 => "september",
            10 => "october",
            11 => "november",
            12 => "december",
            _ => "unknown",
        };

        format!(
            "highest-temperature-in-{}-on-{}-{}-{}",
            city,
            month,
            date.day(),
            date.year()
        )
    }
}
