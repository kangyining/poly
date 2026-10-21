use chrono::NaiveDate;

use crate::config;

pub struct EventService;

impl EventService {
    pub fn resolve_start_date(override_date: Option<NaiveDate>) -> NaiveDate {
        config::resolve_start_date(override_date)
    }

    pub fn build_event_slug(city: &str, date: NaiveDate) -> String {
        config::build_event_slug(city, date)
    }
}
