use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config;
use crate::data::models::WeatherContract;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherContractDiscovery {
    pub contract: WeatherContract,
    pub event: Option<Value>,
}

impl WeatherContractDiscovery {
    pub fn is_valid(&self) -> bool {
        self.contract.validation_status == "valid"
    }
}

pub fn parse_daily_high_slug(slug: &str) -> Option<(String, NaiveDate)> {
    let re = Regex::new(
        r"(?i)^highest-temperature-in-(?P<city>[a-z0-9-]+)-on-(?P<month>[a-z]+)-(?P<day>\d{1,2})-(?P<year>\d{4})$",
    )
    .ok()?;
    let caps = re.captures(slug)?;
    let month = month_number(&caps["month"].to_lowercase())?;
    let target =
        NaiveDate::from_ymd_opt(caps["year"].parse().ok()?, month, caps["day"].parse().ok()?)?;
    Some((caps["city"].to_lowercase(), target))
}

pub fn configured_weather_contracts(target_dates: &[NaiveDate]) -> Vec<WeatherContract> {
    let mut out = Vec::new();
    for cfg in config::enabled_contracts() {
        for target in target_dates {
            out.push(contract_from_config(
                cfg,
                *target,
                &config::build_daily_high_slug(cfg.slug, *target),
                None,
                "configured",
                None,
                None,
            ));
        }
    }
    out
}

pub fn discover_configured_weather_contracts(
    target_dates: &[NaiveDate],
    include_skipped: bool,
) -> Vec<WeatherContractDiscovery> {
    let mut out = Vec::new();
    for cfg in config::enabled_contracts() {
        for target in target_dates {
            let slug = config::build_daily_high_slug(cfg.slug, *target);
            match crate::data::gamma::fetch_event_by_slug(&slug) {
                Ok(Some(event)) => {
                    let discovery = validate_weather_event(&event);
                    if discovery.is_valid() || include_skipped {
                        out.push(discovery);
                    }
                }
                Ok(None) if include_skipped => out.push(WeatherContractDiscovery {
                    contract: contract_from_config(
                        cfg,
                        *target,
                        &slug,
                        None,
                        "skipped",
                        Some("event_not_found".to_string()),
                        None,
                    ),
                    event: None,
                }),
                Ok(None) => {}
                Err(err) if include_skipped => out.push(WeatherContractDiscovery {
                    contract: contract_from_config(
                        cfg,
                        *target,
                        &slug,
                        None,
                        "skipped",
                        Some(format!("fetch_error:{err}")),
                        None,
                    ),
                    event: None,
                }),
                Err(_) => {}
            }
        }
    }
    out
}

pub fn validate_weather_event(event: &Value) -> WeatherContractDiscovery {
    let slug = event
        .get("slug")
        .or_else(|| event.get("_slug"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some((city_slug, target)) = parse_daily_high_slug(slug) else {
        return skipped(slug, event, "unsupported_slug", "", None, "", "", "F");
    };
    let Some(cfg) = config::contract_for_city(&city_slug) else {
        return skipped(
            slug,
            event,
            "city_not_allowlisted",
            &city_slug,
            Some(target),
            "",
            "",
            "F",
        );
    };

    let text = event_text(event);
    let unit = detect_unit(&text);
    if unit.as_deref() != Some("F") {
        return skipped(
            slug,
            event,
            "unsupported_unit",
            &city_slug,
            Some(target),
            "",
            "",
            unit.as_deref().unwrap_or(""),
        );
    }

    let source_url = source_url(event, &text);
    if !source_url
        .to_lowercase()
        .contains("wunderground.com/history/daily/")
    {
        return skipped(
            slug,
            event,
            "unsupported_resolution_source",
            &city_slug,
            Some(target),
            "",
            &source_url,
            "F",
        );
    }
    let station = station_from_source_url(&source_url);
    if station.is_empty() {
        return skipped(
            slug,
            event,
            "missing_station",
            &city_slug,
            Some(target),
            "",
            &source_url,
            "F",
        );
    }
    if station.to_uppercase() != cfg.station_id.to_uppercase() {
        return skipped(
            slug,
            event,
            &format!("station_mismatch:{}!={}", station, cfg.station_id),
            &city_slug,
            Some(target),
            &station,
            &source_url,
            "F",
        );
    }

    WeatherContractDiscovery {
        contract: contract_from_config(
            cfg,
            target,
            slug,
            crate::data::gamma::value_to_string(event.get("id")),
            "valid",
            None,
            Some(source_url),
        ),
        event: Some(event.clone()),
    }
}

fn skipped(
    slug: &str,
    event: &Value,
    reason: &str,
    city_slug: &str,
    target: Option<NaiveDate>,
    station_id: &str,
    source_url: &str,
    unit: &str,
) -> WeatherContractDiscovery {
    let target = target
        .or_else(|| fallback_target_date(event))
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1, 1, 1).unwrap());
    let contract = if let Some(cfg) = config::contract_for_city(city_slug) {
        contract_from_config(
            cfg,
            target,
            slug,
            crate::data::gamma::value_to_string(event.get("id")),
            "skipped",
            Some(reason.to_string()),
            (!source_url.is_empty()).then(|| source_url.to_string()),
        )
    } else {
        WeatherContract {
            event_slug: slug.to_string(),
            target_date: target.to_string(),
            market_type: "daily_temperature".to_string(),
            measure: "high".to_string(),
            unit: if unit.is_empty() { "F" } else { unit }.to_string(),
            city_slug: city_slug.to_string(),
            display_city: title_from_slug(city_slug),
            resolution_source_kind: if source_url.to_lowercase().contains("wunderground.com") {
                "wunderground".to_string()
            } else {
                String::new()
            },
            station_id: station_id.to_string(),
            source_url: source_url.to_string(),
            timezone: String::new(),
            lat: 0.0,
            lon: 0.0,
            validation_status: "skipped".to_string(),
            skip_reason: Some(reason.to_string()),
            event_id: crate::data::gamma::value_to_string(event.get("id")),
            raw_json: Some(raw_event_json(event)),
        }
    };
    WeatherContractDiscovery {
        contract,
        event: Some(event.clone()),
    }
}

fn contract_from_config(
    cfg: &config::WeatherContractConfig,
    target: NaiveDate,
    slug: &str,
    event_id: Option<String>,
    status: &str,
    skip_reason: Option<String>,
    source_url: Option<String>,
) -> WeatherContract {
    WeatherContract {
        event_slug: slug.to_string(),
        event_id,
        target_date: target.to_string(),
        market_type: "daily_temperature".to_string(),
        measure: "high".to_string(),
        unit: "F".to_string(),
        city_slug: cfg.slug.to_string(),
        display_city: cfg.display_city.to_string(),
        resolution_source_kind: "wunderground".to_string(),
        station_id: cfg.station_id.to_string(),
        source_url: source_url.unwrap_or_else(|| cfg.source_url.to_string()),
        timezone: cfg.tz.to_string(),
        lat: cfg.lat,
        lon: cfg.lon,
        validation_status: status.to_string(),
        skip_reason,
        raw_json: None,
    }
}

pub fn event_text(event: &Value) -> String {
    let mut chunks = vec![
        event.get("title").and_then(Value::as_str).unwrap_or(""),
        event
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(""),
        event
            .get("resolutionSource")
            .and_then(Value::as_str)
            .unwrap_or(""),
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    if let Some(markets) = event.get("markets").and_then(Value::as_array) {
        for market in markets {
            for key in [
                "question",
                "groupItemTitle",
                "description",
                "resolutionSource",
            ] {
                chunks.push(
                    market
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
        }
    }
    chunks.join("\n")
}

pub fn detect_unit(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if text.contains("°F") || lower.contains("degrees fahrenheit") {
        Some("F".to_string())
    } else if text.contains("°C") || lower.contains("degrees celsius") {
        Some("C".to_string())
    } else {
        None
    }
}

pub fn source_url(event: &Value, text: &str) -> String {
    if let Some(source) = event.get("resolutionSource").and_then(Value::as_str) {
        if !source.is_empty() {
            return source.to_string();
        }
    }
    let re = Regex::new(r"https://www\.wunderground\.com/history/daily/[^\s)]+").unwrap();
    re.find(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',']).to_string())
        .unwrap_or_default()
}

pub fn station_from_source_url(source_url: &str) -> String {
    source_url
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_uppercase()
}

fn fallback_target_date(event: &Value) -> Option<NaiveDate> {
    event
        .get("endDate")
        .or_else(|| event.get("end_date"))
        .and_then(Value::as_str)
        .and_then(|raw| raw.get(..10))
        .and_then(|raw| NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok())
        .or_else(|| {
            event
                .get("slug")
                .or_else(|| event.get("_slug"))
                .and_then(Value::as_str)
                .and_then(parse_daily_high_slug)
                .map(|(_, target)| target)
        })
}

fn raw_event_json(event: &Value) -> String {
    json!({
        "title": event.get("title"),
        "slug": event.get("slug"),
        "resolutionSource": event.get("resolutionSource"),
    })
    .to_string()
}

fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn month_number(month: &str) -> Option<u32> {
    Some(match month {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_daily_high_slug() {
        let (city, target) =
            parse_daily_high_slug("highest-temperature-in-nyc-on-april-19-2026").unwrap();
        assert_eq!(city, "nyc");
        assert_eq!(target.to_string(), "2026-04-19");
    }

    #[test]
    fn validates_configured_wunderground_event() {
        let event = json!({
            "id": "1",
            "slug": "highest-temperature-in-nyc-on-april-19-2026",
            "title": "Highest temperature in NYC on April 19, 2026?",
            "description": "Resolves in °F.",
            "resolutionSource": "https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA",
            "markets": []
        });
        let discovery = validate_weather_event(&event);
        assert!(discovery.is_valid());
        assert_eq!(discovery.contract.station_id, "KLGA");
    }

    #[test]
    fn detects_station_mismatch() {
        let event = json!({
            "id": "1",
            "slug": "highest-temperature-in-nyc-on-april-19-2026",
            "description": "Resolves in °F.",
            "resolutionSource": "https://www.wunderground.com/history/daily/us/ny/new-york-city/KJFK",
            "markets": []
        });
        let discovery = validate_weather_event(&event);
        assert!(!discovery.is_valid());
        assert!(discovery
            .contract
            .skip_reason
            .unwrap()
            .starts_with("station_mismatch"));
    }
}
