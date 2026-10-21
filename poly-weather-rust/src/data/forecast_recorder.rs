use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::data::gamma::value_to_f64;
use crate::data::models::{ForecastModelRun, ForecastRecord};
use crate::data::weather_contracts;
use crate::db::{self, Database};

const OPEN_METEO_URL: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastRunSummary {
    pub contracts_seen: usize,
    pub forecasts_saved: usize,
    pub model_runs_saved: usize,
    pub fetch_errors: usize,
}

pub fn c_to_f(c: f64) -> f64 {
    round1(c * 9.0 / 5.0 + 32.0)
}

pub fn f_to_c(f: f64) -> f64 {
    round1((f - 32.0) * 5.0 / 9.0)
}

pub fn lead_hours(target: NaiveDate, tz: &str, now: DateTime<Utc>) -> f64 {
    let tz = tz.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    let Some(local_midnight) = tz
        .with_ymd_and_hms(target.year(), target.month(), target.day(), 0, 0, 0)
        .single()
    else {
        return 0.0;
    };
    let delta = local_midnight.with_timezone(&Utc) - now;
    ((delta.num_seconds() as f64) / 3600.0).max(0.0)
}

pub fn fetch_forecast(lat: f64, lon: f64, tz: &str) -> Result<Value> {
    let payload = crate::data::http::get_json(
        OPEN_METEO_URL,
        &[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("daily", "temperature_2m_max".to_string()),
            ("models", "ecmwf_ifs025,gfs_seamless".to_string()),
            ("temperature_unit", "fahrenheit".to_string()),
            ("forecast_days", "3".to_string()),
            ("timezone", tz.to_string()),
        ],
        15,
    )
    .context("Open-Meteo forecast fetch failed")?;
    Ok(payload)
}

pub fn record_forecasts(db: &Database) -> Result<ForecastRunSummary> {
    let now = Utc::now();
    let target_dates = (0..3)
        .map(|offset| now.date_naive() + Duration::days(offset))
        .collect::<Vec<_>>();
    let contracts = weather_contracts::configured_weather_contracts(&target_dates);
    let mut summary = ForecastRunSummary {
        contracts_seen: contracts.len(),
        forecasts_saved: 0,
        model_runs_saved: 0,
        fetch_errors: 0,
    };

    for contract in contracts {
        db::upsert_weather_contract(db, &contract)?;
        let raw = match fetch_forecast(contract.lat, contract.lon, &contract.timezone) {
            Ok(raw) => raw,
            Err(_) => {
                summary.fetch_errors += 1;
                continue;
            }
        };
        let daily = raw.get("daily").unwrap_or(&Value::Null);
        let dates = daily
            .get("time")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let ecmwf = daily
            .get("temperature_2m_max_ecmwf_ifs025")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let gfs = daily
            .get("temperature_2m_max_gfs_seamless")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for (idx, day_value) in dates.iter().enumerate() {
            let Some(date_str) = day_value.as_str() else {
                continue;
            };
            if date_str != contract.target_date {
                continue;
            }
            let Ok(target) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                continue;
            };
            let ec_f = value_to_f64(ecmwf.get(idx));
            let gf_f = value_to_f64(gfs.get(idx));
            let ec_c = ec_f.map(f_to_c);
            let gf_c = gf_f.map(f_to_c);
            let spread_c = match (ec_c, gf_c) {
                (Some(a), Some(b)) => Some((a - b).abs()),
                _ => None,
            };
            let forecast = ForecastRecord {
                timestamp: now,
                city: contract.display_city.clone(),
                target_date: date_str.to_string(),
                source: "open-meteo".to_string(),
                temp_max_ecmwf_c: ec_c,
                temp_max_gfs_c: gf_c,
                temp_max_ecmwf_f: ec_f,
                temp_max_gfs_f: gf_f,
                model_spread_c: spread_c,
                forecast_lead_hours: lead_hours(target, &contract.timezone, now),
                raw_json: raw.to_string(),
                market_type: Some(contract.market_type.clone()),
                measure: Some(contract.measure.clone()),
                unit: Some(contract.unit.clone()),
                city_slug: Some(contract.city_slug.clone()),
                station_id: Some(contract.station_id.clone()),
                event_slug: Some(contract.event_slug.clone()),
                value_ecmwf: ec_f,
                value_gfs: gf_f,
                value_unit: Some(contract.unit.clone()),
            };
            db::insert_forecast(db, &forecast)?;
            summary.forecasts_saved += 1;

            for (model_name, temp_f) in [("ecmwf_ifs025", ec_f), ("gfs_seamless", gf_f)] {
                let Some(temp_f) = temp_f else {
                    continue;
                };
                let run = ForecastModelRun {
                    timestamp: now,
                    city: contract.display_city.clone(),
                    target_date: date_str.to_string(),
                    source: "open-meteo".to_string(),
                    model_name: model_name.to_string(),
                    model_run_time: None,
                    forecast_lead_hours: Some(forecast.forecast_lead_hours),
                    temp_max_f: Some(temp_f),
                    raw_json: Some(
                        json!({
                            "source": "open-meteo",
                            "model": model_name,
                            "target_date": date_str,
                            "temp_max_f": temp_f
                        })
                        .to_string(),
                    ),
                    market_type: Some(contract.market_type.clone()),
                    measure: Some(contract.measure.clone()),
                    unit: Some(contract.unit.clone()),
                    city_slug: Some(contract.city_slug.clone()),
                    station_id: Some(contract.station_id.clone()),
                    event_slug: Some(contract.event_slug.clone()),
                    value: Some(temp_f),
                    value_unit: Some(contract.unit.clone()),
                };
                db::insert_forecast_model_run(db, &run)?;
                summary.model_runs_saved += 1;
            }
        }
    }
    Ok(summary)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_temperature_units_like_python() {
        assert_eq!(c_to_f(0.0), 32.0);
        assert_eq!(f_to_c(50.0), 10.0);
    }

    #[test]
    fn lead_hours_uses_local_midnight() {
        let now = "2026-04-18T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let target = NaiveDate::from_ymd_opt(2026, 4, 19).unwrap();
        assert_eq!(lead_hours(target, "America/New_York", now), 16.0);
    }
}
