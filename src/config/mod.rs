use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeatherContractConfig {
    pub slug: &'static str,
    pub display_city: &'static str,
    pub station_id: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub tz: &'static str,
    pub source_url: &'static str,
    pub enabled: bool,
}

pub const DAILY_HIGH_F_CONTRACTS: &[WeatherContractConfig] = &[
    WeatherContractConfig {
        slug: "atlanta",
        display_city: "Atlanta",
        station_id: "KATL",
        lat: 33.6407,
        lon: -84.4277,
        tz: "America/New_York",
        source_url: "https://www.wunderground.com/history/daily/us/ga/atlanta/KATL",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "austin",
        display_city: "Austin",
        station_id: "KAUS",
        lat: 30.1975,
        lon: -97.6664,
        tz: "America/Chicago",
        source_url: "https://www.wunderground.com/history/daily/us/tx/austin/KAUS",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "chicago",
        display_city: "Chicago",
        station_id: "KORD",
        lat: 41.9742,
        lon: -87.9073,
        tz: "America/Chicago",
        source_url: "https://www.wunderground.com/history/daily/us/il/chicago/KORD",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "dallas",
        display_city: "Dallas",
        station_id: "KDAL",
        lat: 32.8471,
        lon: -96.8518,
        tz: "America/Chicago",
        source_url: "https://www.wunderground.com/history/daily/us/tx/dallas/KDAL",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "denver",
        display_city: "Denver",
        station_id: "KBKF",
        lat: 39.7017,
        lon: -104.7517,
        tz: "America/Denver",
        source_url: "https://www.wunderground.com/history/daily/us/co/aurora/KBKF",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "houston",
        display_city: "Houston",
        station_id: "KHOU",
        lat: 29.6454,
        lon: -95.2789,
        tz: "America/Chicago",
        source_url: "https://www.wunderground.com/history/daily/us/tx/houston/KHOU",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "los-angeles",
        display_city: "Los Angeles",
        station_id: "KLAX",
        lat: 33.9425,
        lon: -118.4081,
        tz: "America/Los_Angeles",
        source_url: "https://www.wunderground.com/history/daily/us/ca/los-angeles/KLAX",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "miami",
        display_city: "Miami",
        station_id: "KMIA",
        lat: 25.7959,
        lon: -80.2870,
        tz: "America/New_York",
        source_url: "https://www.wunderground.com/history/daily/us/fl/miami/KMIA",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "nyc",
        display_city: "NYC",
        station_id: "KLGA",
        lat: 40.7769,
        lon: -73.8740,
        tz: "America/New_York",
        source_url: "https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "san-francisco",
        display_city: "San Francisco",
        station_id: "KSFO",
        lat: 37.6213,
        lon: -122.3790,
        tz: "America/Los_Angeles",
        source_url: "https://www.wunderground.com/history/daily/us/ca/san-francisco/KSFO",
        enabled: true,
    },
    WeatherContractConfig {
        slug: "seattle",
        display_city: "Seattle",
        station_id: "KSEA",
        lat: 47.4502,
        lon: -122.3088,
        tz: "America/Los_Angeles",
        source_url: "https://www.wunderground.com/history/daily/us/wa/seatac/KSEA",
        enabled: true,
    },
];

pub fn enabled_contracts() -> impl Iterator<Item = &'static WeatherContractConfig> {
    DAILY_HIGH_F_CONTRACTS.iter().filter(|c| c.enabled)
}

pub fn normalize_key(value: &str) -> String {
    let re = Regex::new(r"[^a-z0-9]+").expect("valid regex");
    re.replace_all(&value.trim().to_lowercase(), "-")
        .trim_matches('-')
        .to_string()
}

pub fn contract_for_city(value: &str) -> Option<&'static WeatherContractConfig> {
    if value.trim().is_empty() {
        return None;
    }
    let key = normalize_key(value);
    enabled_contracts().find(|cfg| key == cfg.slug || key == normalize_key(cfg.display_city))
}

pub fn contract_for_station(station_id: &str) -> Option<&'static WeatherContractConfig> {
    let station = station_id.trim().to_uppercase();
    enabled_contracts().find(|cfg| cfg.station_id.eq_ignore_ascii_case(&station))
}

pub fn contract_for_display_city(display_city: &str) -> Option<&'static WeatherContractConfig> {
    let key = normalize_key(display_city);
    enabled_contracts().find(|cfg| key == normalize_key(cfg.display_city))
}

pub fn normalize_city(value: Option<&str>) -> Option<String> {
    value.and_then(|raw| {
        contract_for_city(raw)
            .map(|cfg| cfg.display_city.to_string())
            .or_else(|| Some(raw.to_string()).filter(|s| !s.trim().is_empty()))
    })
}

pub fn station_for_city(city: &str) -> String {
    contract_for_city(city)
        .map(|cfg| cfg.station_id.to_string())
        .unwrap_or_else(|| city.to_string())
}

pub fn known_city_names() -> Vec<String> {
    let mut names = enabled_contracts()
        .map(|cfg| cfg.display_city.to_string())
        .collect::<Vec<_>>();
    names.sort();
    names
}

pub fn enabled_station_ids() -> Vec<String> {
    let mut stations = enabled_contracts()
        .map(|cfg| cfg.station_id.to_string())
        .collect::<Vec<_>>();
    stations.sort();
    stations
}

pub fn build_daily_high_slug(city_slug: &str, target: NaiveDate) -> String {
    format!(
        "highest-temperature-in-{}-on-{}-{}-{}",
        city_slug,
        month_name(target.month()),
        target.day(),
        target.year()
    )
}

pub fn default_event_slug(city: &str, target_date: &str) -> String {
    let target = NaiveDate::parse_from_str(target_date, "%Y-%m-%d").expect("valid target date");
    if let Some(cfg) = contract_for_city(city) {
        build_daily_high_slug(cfg.slug, target)
    } else {
        build_daily_high_slug(&normalize_key(city), target)
    }
}

fn month_name(month: u32) -> &'static str {
    match month {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_city_aliases() {
        assert_eq!(normalize_city(Some("nyc")).as_deref(), Some("NYC"));
        assert_eq!(
            normalize_city(Some("Los Angeles")).as_deref(),
            Some("Los Angeles")
        );
    }

    #[test]
    fn builds_slug_like_python() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 19).unwrap();
        assert_eq!(
            build_daily_high_slug("nyc", d),
            "highest-temperature-in-nyc-on-april-19-2026"
        );
    }

    #[test]
    fn maps_station_to_contract() {
        assert_eq!(contract_for_station("klga").unwrap().display_city, "NYC");
        assert!(enabled_station_ids().contains(&"KLGA".to_string()));
    }

    #[test]
    fn maps_display_city_to_contract() {
        assert_eq!(
            contract_for_display_city("San Francisco").unwrap().slug,
            "san-francisco"
        );
    }
}
