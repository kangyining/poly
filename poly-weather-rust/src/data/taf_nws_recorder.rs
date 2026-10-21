use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration as StdDuration;

use crate::config;
use crate::data::gamma::{value_to_f64, value_to_string};
use crate::data::models::{NwsForecast, TafRecord};
use crate::db::{self, Database};

const TAF_URL: &str = "https://aviationweather.gov/api/data/taf";
const NWS_USER_AGENT: &str = "poly-weather-rust/0.1 (contact: admin@localhost)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TafRunSummary {
    pub stations_requested: usize,
    pub records_received: usize,
    pub records_saved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwsRunSummary {
    pub cities_requested: usize,
    pub records_saved: usize,
    pub fetch_errors: usize,
}

pub fn fetch_tafs(icao_list: &[String]) -> Result<Vec<Value>> {
    let payload = crate::data::http::get_json(
        TAF_URL,
        &[("ids", icao_list.join(",")), ("format", "json".to_string())],
        15,
    )
    .context("TAF fetch failed")?;
    Ok(payload.as_array().cloned().unwrap_or_default())
}

pub fn parse_taf(raw: &Value) -> Option<TafRecord> {
    let icao = value_to_string(raw.get("icaoId").or_else(|| raw.get("stationId")))?;
    let city = config::contract_for_station(&icao)
        .map(|cfg| cfg.display_city.to_string())
        .unwrap_or_else(|| icao.clone());
    Some(TafRecord {
        timestamp: Utc::now(),
        icao,
        city,
        issue_time: parse_time(raw.get("issueTime").or_else(|| raw.get("bulletinTime")))
            .map(|dt| dt.to_rfc3339()),
        valid_from: parse_time(raw.get("validTimeFrom")).map(|dt| dt.to_rfc3339()),
        valid_to: parse_time(raw.get("validTimeTo")).map(|dt| dt.to_rfc3339()),
        raw_taf: value_to_string(raw.get("rawTAF").or_else(|| raw.get("rawOb")))
            .unwrap_or_default(),
    })
}

pub fn record_tafs(db: &Database) -> Result<TafRunSummary> {
    let stations = config::enabled_station_ids();
    let raw_records = fetch_tafs(&stations)?;
    let mut saved = 0;
    for raw in &raw_records {
        if let Some(record) = parse_taf(raw) {
            db::insert_taf(db, &record)?;
            saved += 1;
        }
    }
    Ok(TafRunSummary {
        stations_requested: stations.len(),
        records_received: raw_records.len(),
        records_saved: saved,
    })
}

pub fn record_nws_forecasts(db: &Database) -> Result<NwsRunSummary> {
    let now = Utc::now();
    let today = now.date_naive();
    let mut summary = NwsRunSummary {
        cities_requested: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    let mut forecast_url_cache = BTreeMap::<String, String>::new();
    for cfg in config::enabled_contracts() {
        summary.cities_requested += 1;
        let forecast_url = match resolve_nws_forecast_url(cfg, &mut forecast_url_cache) {
            Ok(Some(url)) => url,
            Ok(None) => continue,
            Err(_) => {
                summary.fetch_errors += 1;
                thread::sleep(StdDuration::from_secs(1));
                continue;
            }
        };
        let policy = crate::data::http::RetryPolicy::new(30).user_agent(NWS_USER_AGENT);
        let payload = match crate::data::http::get_json_with_policy(&forecast_url, &[], &policy) {
            Ok(payload) => payload,
            Err(_) => {
                summary.fetch_errors += 1;
                thread::sleep(StdDuration::from_secs(1));
                continue;
            }
        };
        let periods = payload
            .get("properties")
            .and_then(|p| p.get("periods"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for period in periods {
            if !period
                .get("isDaytime")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(start) = period
                .get("startTime")
                .and_then(Value::as_str)
                .and_then(crate::db::parse_ts)
            else {
                continue;
            };
            let target = start.date_naive();
            if target < today || target > today + Duration::days(7) {
                continue;
            }
            let rec = NwsForecast {
                timestamp: now,
                city: cfg.display_city.to_string(),
                target_date: target.to_string(),
                period_name: period
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                is_daytime: Some(true),
                temp_f: value_to_f64(period.get("temperature")),
                short_forecast: period
                    .get("shortForecast")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                forecast_lead_hours: Some(lead_hours_to_noon(target, now)),
            };
            db::insert_nws_forecast(db, &rec)?;
            summary.records_saved += 1;
        }
        thread::sleep(StdDuration::from_millis(500));
    }
    Ok(summary)
}

fn resolve_nws_forecast_url(
    cfg: &config::WeatherContractConfig,
    cache: &mut BTreeMap<String, String>,
) -> Result<Option<String>> {
    if let Some(url) = cache.get(cfg.station_id) {
        return Ok(Some(url.clone()));
    }
    let policy = crate::data::http::RetryPolicy::new(30).user_agent(NWS_USER_AGENT);
    let payload = crate::data::http::get_json_with_policy(
        &format!(
            "https://api.weather.gov/points/{:.4},{:.4}",
            cfg.lat, cfg.lon
        ),
        &[],
        &policy,
    )
    .with_context(|| format!("NWS points fetch failed for {}", cfg.display_city))?;
    let url = payload
        .get("properties")
        .and_then(|p| p.get("forecast"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(url) = &url {
        cache.insert(cfg.station_id.to_string(), url.clone());
    }
    Ok(url)
}

fn lead_hours_to_noon(target: NaiveDate, now: DateTime<Utc>) -> f64 {
    let close = target.and_hms_opt(12, 0, 0).unwrap().and_utc();
    ((close - now).num_seconds() as f64 / 3600.0).max(0.0)
}

fn parse_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    if let Some(epoch) = value.and_then(Value::as_i64) {
        return DateTime::from_timestamp(epoch, 0);
    }
    if let Some(raw) = value.and_then(Value::as_str) {
        if let Ok(epoch) = raw.parse::<i64>() {
            return DateTime::from_timestamp(epoch, 0);
        }
        return crate::db::parse_ts(raw);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_taf_record() {
        let raw = json!({
            "icaoId": "KLGA",
            "issueTime": 1770000000,
            "validTimeFrom": 1770000100,
            "validTimeTo": 1770000200,
            "rawTAF": "TAF KLGA"
        });
        let rec = parse_taf(&raw).unwrap();
        assert_eq!(rec.city, "NYC");
        assert_eq!(rec.raw_taf, "TAF KLGA");
        assert!(rec.issue_time.unwrap().starts_with("2026-02-02"));
    }

    #[test]
    fn computes_nws_lead_to_noon_utc() {
        let now = "2026-04-18T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let target = NaiveDate::from_ymd_opt(2026, 4, 19).unwrap();
        assert_eq!(lead_hours_to_noon(target, now), 24.0);
    }
}
