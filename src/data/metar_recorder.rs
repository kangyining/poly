use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::config;
use crate::data::gamma::{value_to_f64, value_to_string};
use crate::data::models::MetarRecord;
use crate::db::{self, Database};

const METAR_URL: &str = "https://aviationweather.gov/api/data/metar";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetarRunSummary {
    pub stations_requested: usize,
    pub records_received: usize,
    pub records_saved: usize,
}

pub fn c_to_f(c: f64) -> f64 {
    round1(c * 9.0 / 5.0 + 32.0)
}

pub fn fetch_metars(icao_list: &[String]) -> Result<Vec<Value>> {
    let ids = icao_list.join(",");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build METAR HTTP client")?;
    let payload: Value = client
        .get(METAR_URL)
        .query(&[("ids", ids), ("format", "json".to_string())])
        .send()
        .context("METAR fetch failed")?
        .error_for_status()?
        .json()
        .context("failed to decode METAR JSON")?;
    Ok(payload.as_array().cloned().unwrap_or_default())
}

pub fn parse_metar(raw: &Value) -> Option<MetarRecord> {
    let icao = value_to_string(raw.get("icaoId").or_else(|| raw.get("stationId")))?;
    let temp_c = value_to_f64(raw.get("temp"))?;
    let city = config::contract_for_station(&icao)
        .map(|cfg| cfg.display_city.to_string())
        .unwrap_or_else(|| icao.clone());
    let obs_time = parse_obs_time(raw.get("obsTime").or_else(|| raw.get("reportTime")))
        .unwrap_or_else(Utc::now);
    Some(MetarRecord {
        timestamp: Utc::now(),
        icao,
        city,
        temp_c,
        temp_f: c_to_f(temp_c),
        dewpoint_c: value_to_f64(raw.get("dewp")),
        obs_time,
        raw_metar: value_to_string(raw.get("rawOb")).unwrap_or_default(),
    })
}

pub fn record_metars(db: &Database) -> Result<MetarRunSummary> {
    let stations = config::enabled_station_ids();
    let raw_records = fetch_metars(&stations)?;
    let mut saved = 0;
    for raw in &raw_records {
        if let Some(record) = parse_metar(raw) {
            db::insert_metar(db, &record)?;
            saved += 1;
        }
    }
    Ok(MetarRunSummary {
        stations_requested: stations.len(),
        records_received: raw_records.len(),
        records_saved: saved,
    })
}

fn parse_obs_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
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

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_c_to_f_like_python() {
        assert_eq!(c_to_f(10.0), 50.0);
        assert_eq!(c_to_f(23.34), 74.0);
    }

    #[test]
    fn parses_metar_record() {
        let raw = json!({
            "icaoId": "KLGA",
            "temp": 12.0,
            "dewp": 4.0,
            "obsTime": 1770000000,
            "rawOb": "KLGA METAR"
        });
        let rec = parse_metar(&raw).unwrap();
        assert_eq!(rec.city, "NYC");
        assert_eq!(rec.temp_f, 53.6);
        assert_eq!(rec.raw_metar, "KLGA METAR");
    }
}
