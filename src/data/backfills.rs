use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::thread;
use std::time::Duration as StdDuration;

use crate::config;
use crate::data::forecast_recorder::f_to_c;
use crate::data::gamma::{self, value_to_f64};
use crate::data::models::{ForecastModelRun, ForecastRecord, MetarRecord, PriceHistoryRecord};
use crate::db::{self, Database, NewSettlement};

const ASOS_API: &str = "https://mesonet.agron.iastate.edu/cgi-bin/request/asos.py";
const HISTORICAL_FORECAST_URL: &str = "https://historical-forecast-api.open-meteo.com/v1/forecast";
const CLOB_PRICE_HISTORY_URL: &str = "https://clob.polymarket.com/prices-history";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillSummary {
    pub records_seen: usize,
    pub records_saved: usize,
    pub fetch_errors: usize,
}

pub fn backfill_settlement_temps(
    db: &Database,
    city: Option<&str>,
    days_back: i64,
) -> Result<BackfillSummary> {
    let contracts = contract_filter(city);
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    let today = Utc::now().date_naive();
    let start = today - Duration::days(days_back.max(1));
    for cfg in contracts {
        let mut day = start;
        while day < today {
            summary.records_seen += 1;
            let d = day.to_string();
            if let Some(temp_f) = fetch_daily_high_asos(cfg.station_id, &d)? {
                let settlement = NewSettlement {
                    city: cfg.display_city.to_string(),
                    target_date: d.clone(),
                    event_id: String::new(),
                    event_slug: config::build_daily_high_slug(cfg.slug, day),
                    winning_bin: temp_to_bin(temp_f),
                    settlement_temp_f: Some(temp_f),
                    wunderground_station: cfg.station_id.to_string(),
                    resolved_at: Some(Utc::now().to_rfc3339()),
                    raw_outcome_prices: json!({}).to_string(),
                    market_type: Some("daily_temperature".to_string()),
                    measure: Some("high".to_string()),
                    unit: Some("F".to_string()),
                    city_slug: Some(cfg.slug.to_string()),
                    station_id: Some(cfg.station_id.to_string()),
                    source_url: Some(cfg.source_url.to_string()),
                    settlement_value: Some(temp_f),
                    settlement_unit: Some("F".to_string()),
                    settlement_temp_source: Some("asos-iowa-mesonet".to_string()),
                    settlement_temp_station: Some(cfg.station_id.to_string()),
                    settlement_temp_raw_json: Some(
                        json!({
                            "source": "asos-iowa-mesonet",
                            "station": cfg.station_id,
                            "target_date": d,
                            "daily_high_f": temp_f,
                            "source_api": ASOS_API
                        })
                        .to_string(),
                    ),
                };
                db::insert_settlement(db, &settlement)?;
                summary.records_saved += 1;
            } else {
                summary.fetch_errors += 1;
            }
            day = day.succ_opt().context("date overflow in ASOS backfill")?;
        }
    }
    Ok(summary)
}

pub fn backfill_asos_metars(
    db: &Database,
    city: Option<&str>,
    days_back: i64,
) -> Result<BackfillSummary> {
    let contracts = contract_filter(city);
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    let today = Utc::now().date_naive();
    let start = today - Duration::days(days_back.max(1));
    for cfg in contracts {
        let csv = match fetch_asos_csv(cfg.station_id, &start.to_string(), &today.to_string()) {
            Ok(csv) => csv,
            Err(_) => {
                summary.fetch_errors += 1;
                continue;
            }
        };
        for row in parse_asos_rows(&csv) {
            summary.records_seen += 1;
            let Some(tmpf) = row.tmpf else { continue };
            let obs_time = row
                .valid
                .and_then(|raw| crate::db::parse_ts(&raw))
                .unwrap_or_else(Utc::now);
            let rec = MetarRecord {
                timestamp: Utc::now(),
                icao: cfg.station_id.to_string(),
                city: cfg.display_city.to_string(),
                temp_c: f_to_c(tmpf),
                temp_f: tmpf,
                dewpoint_c: row.dwpf.map(f_to_c),
                obs_time,
                raw_metar: row.metar.unwrap_or_default(),
            };
            db::insert_metar(db, &rec)?;
            summary.records_saved += 1;
        }
    }
    Ok(summary)
}

pub fn backfill_historical_forecasts(db: &Database, city: Option<&str>) -> Result<BackfillSummary> {
    backfill_open_meteo_history(db, city, "open-meteo-historical")
}

pub fn backfill_true_forecasts(db: &Database, city: Option<&str>) -> Result<BackfillSummary> {
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    for cfg in contract_filter(city) {
        let rows = settlement_rows_missing_source(db, cfg.display_city, "gfs-global-24h")?;
        summary.records_seen += rows.len();
        for (target_date, event_slug) in rows {
            let gfs_f = match fetch_true_gfs_forecast(cfg.lat, cfg.lon, target_date) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(_) => {
                    summary.fetch_errors += 1;
                    continue;
                }
            };
            let now = Utc::now();
            let d = target_date.to_string();
            let model_run = true_gfs_model_run(target_date);
            let raw_json = json!({
                "model": "gfs_global",
                "past_model_run": model_run,
                "target_date": d,
                "gfs_f": gfs_f,
            });
            let event_slug =
                event_slug.unwrap_or_else(|| config::build_daily_high_slug(cfg.slug, target_date));
            let forecast = ForecastRecord {
                timestamp: now,
                city: cfg.display_city.to_string(),
                target_date: d.clone(),
                source: "gfs-global-24h".to_string(),
                temp_max_ecmwf_c: None,
                temp_max_gfs_c: Some(f_to_c(gfs_f)),
                temp_max_ecmwf_f: None,
                temp_max_gfs_f: Some(gfs_f),
                model_spread_c: None,
                forecast_lead_hours: 26.0,
                raw_json: raw_json.to_string(),
                market_type: Some("daily_temperature".to_string()),
                measure: Some("high".to_string()),
                unit: Some("F".to_string()),
                city_slug: Some(cfg.slug.to_string()),
                station_id: Some(cfg.station_id.to_string()),
                event_slug: Some(event_slug.clone()),
                value_ecmwf: None,
                value_gfs: Some(gfs_f),
                value_unit: Some("F".to_string()),
            };
            db::insert_forecast(db, &forecast)?;
            let run = ForecastModelRun {
                timestamp: now,
                city: cfg.display_city.to_string(),
                target_date: d,
                source: "gfs-global-24h".to_string(),
                model_name: "gfs_global".to_string(),
                model_run_time: Some(model_run),
                forecast_lead_hours: Some(26.0),
                temp_max_f: Some(gfs_f),
                raw_json: Some(raw_json.to_string()),
                market_type: Some("daily_temperature".to_string()),
                measure: Some("high".to_string()),
                unit: Some("F".to_string()),
                city_slug: Some(cfg.slug.to_string()),
                station_id: Some(cfg.station_id.to_string()),
                event_slug: Some(event_slug),
                value: Some(gfs_f),
                value_unit: Some("F".to_string()),
            };
            db::insert_forecast_model_run(db, &run)?;
            summary.records_saved += 1;
            thread::sleep(StdDuration::from_millis(250));
        }
    }
    Ok(summary)
}

fn backfill_open_meteo_history(
    db: &Database,
    city: Option<&str>,
    source: &str,
) -> Result<BackfillSummary> {
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    for cfg in contract_filter(city) {
        let dates = settlement_dates(db, cfg.display_city, source)?;
        if dates.is_empty() {
            continue;
        }
        summary.records_seen += dates.len();
        let start = *dates.first().unwrap();
        let end = *dates.last().unwrap();
        let raw = match fetch_historical_forecasts(cfg.lat, cfg.lon, start, end) {
            Ok(raw) => raw,
            Err(_) => {
                summary.fetch_errors += 1;
                continue;
            }
        };
        let forecasts = historical_lookup(&raw);
        for date in dates {
            let d = date.to_string();
            let Some((ec_f, gf_f)) = forecasts.get(&d).cloned() else {
                continue;
            };
            let now = Utc::now();
            let model_run = if source == "gfs-global-24h" {
                Some(
                    date.and_hms_opt(12, 0, 0)
                        .unwrap()
                        .and_utc()
                        .checked_sub_signed(Duration::hours(24))
                        .unwrap()
                        .to_rfc3339(),
                )
            } else {
                None
            };
            let raw_json = json!({
                "source": source,
                "date": d,
                "ecmwf_f": ec_f,
                "gfs_f": gf_f,
                "past_model_run": model_run,
            });
            let forecast = ForecastRecord {
                timestamp: now,
                city: cfg.display_city.to_string(),
                target_date: d.clone(),
                source: source.to_string(),
                temp_max_ecmwf_c: ec_f.map(f_to_c),
                temp_max_gfs_c: gf_f.map(f_to_c),
                temp_max_ecmwf_f: ec_f,
                temp_max_gfs_f: gf_f,
                model_spread_c: match (ec_f, gf_f) {
                    (Some(a), Some(b)) => Some((f_to_c(a) - f_to_c(b)).abs()),
                    _ => None,
                },
                forecast_lead_hours: if source == "gfs-global-24h" {
                    24.0
                } else {
                    -1.0
                },
                raw_json: raw_json.to_string(),
                market_type: Some("daily_temperature".to_string()),
                measure: Some("high".to_string()),
                unit: Some("F".to_string()),
                city_slug: Some(cfg.slug.to_string()),
                station_id: Some(cfg.station_id.to_string()),
                event_slug: Some(config::build_daily_high_slug(cfg.slug, date)),
                value_ecmwf: ec_f,
                value_gfs: gf_f,
                value_unit: Some("F".to_string()),
            };
            db::insert_forecast(db, &forecast)?;
            for (model_name, temp_f) in [("ecmwf_ifs025", ec_f), ("gfs_seamless", gf_f)] {
                let Some(temp_f) = temp_f else { continue };
                let run = ForecastModelRun {
                    timestamp: now,
                    city: cfg.display_city.to_string(),
                    target_date: d.clone(),
                    source: source.to_string(),
                    model_name: model_name.to_string(),
                    model_run_time: model_run.clone(),
                    forecast_lead_hours: Some(forecast.forecast_lead_hours),
                    temp_max_f: Some(temp_f),
                    raw_json: Some(raw_json.to_string()),
                    market_type: Some("daily_temperature".to_string()),
                    measure: Some("high".to_string()),
                    unit: Some("F".to_string()),
                    city_slug: Some(cfg.slug.to_string()),
                    station_id: Some(cfg.station_id.to_string()),
                    event_slug: Some(config::build_daily_high_slug(cfg.slug, date)),
                    value: Some(temp_f),
                    value_unit: Some("F".to_string()),
                };
                db::insert_forecast_model_run(db, &run)?;
            }
            summary.records_saved += 1;
        }
    }
    Ok(summary)
}

pub fn backfill_price_history(db: &Database, city: Option<&str>) -> Result<BackfillSummary> {
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    for (city_name, target_date, event_slug) in settlement_event_rows(db, city)? {
        summary.records_seen += 1;
        let target = NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?;
        let slug =
            event_slug.unwrap_or_else(|| config::default_event_slug(&city_name, &target_date));
        let Some(event) = gamma::fetch_event_by_slug(&slug)? else {
            summary.fetch_errors += 1;
            continue;
        };
        let close = target.and_hms_opt(12, 0, 0).unwrap().and_utc();
        let start_ts = (close - Duration::hours(48)).timestamp();
        let end_ts = close.timestamp();
        for market in gamma::parse_markets(&event) {
            if market.token_id_yes.is_empty() {
                continue;
            }
            let history = fetch_clob_price_history(&market.token_id_yes, start_ts, end_ts)?;
            let mut hourly = BTreeMap::<DateTime<Utc>, f64>::new();
            for point in history {
                let Some(ts) = point.t.and_then(|t| Utc.timestamp_opt(t, 0).single()) else {
                    continue;
                };
                let hours_to_close = (close - ts).num_seconds() as f64 / 3600.0;
                let minute = PriceHistoryRecord {
                    city: city_name.clone(),
                    target_date: target_date.clone(),
                    bin_label: market.bin_label.clone(),
                    bin_index: Some(market.bin_index),
                    token_id_yes: Some(market.token_id_yes.clone()),
                    timestamp: ts,
                    yes_price: point.p,
                    hours_to_close: Some(hours_to_close),
                    source: "clob-backfill".to_string(),
                    minute: true,
                };
                db::insert_price_history(db, &minute)?;
                hourly.insert(truncate_hour(ts), point.p.unwrap_or(0.0));
                summary.records_saved += 1;
            }
            for (ts, price) in hourly {
                let hourly = PriceHistoryRecord {
                    city: city_name.clone(),
                    target_date: target_date.clone(),
                    bin_label: market.bin_label.clone(),
                    bin_index: Some(market.bin_index),
                    token_id_yes: Some(market.token_id_yes.clone()),
                    timestamp: ts,
                    yes_price: Some(price),
                    hours_to_close: Some((close - ts).num_seconds() as f64 / 3600.0),
                    source: "clob-backfill".to_string(),
                    minute: false,
                };
                db::insert_price_history(db, &hourly)?;
                summary.records_saved += 1;
            }
        }
    }
    Ok(summary)
}

pub fn backfill_missing_bins(db: &Database, city: Option<&str>) -> Result<BackfillSummary> {
    let mut summary = BackfillSummary {
        records_seen: 0,
        records_saved: 0,
        fetch_errors: 0,
    };
    for (city_name, target_date, event_slug) in settlement_event_rows(db, city)? {
        let current = existing_hourly_bin_indexes(db, &city_name, &target_date)?;
        let missing = missing_bin_indexes(&current);
        if missing.is_empty() {
            continue;
        }
        summary.records_seen += missing.len();
        let target = NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?;
        let slug =
            event_slug.unwrap_or_else(|| config::default_event_slug(&city_name, &target_date));
        let Some(event) = gamma::fetch_event_by_slug(&slug)? else {
            summary.fetch_errors += 1;
            continue;
        };
        let close = target.and_hms_opt(12, 0, 0).unwrap().and_utc();
        let start_ts = (close - Duration::hours(48)).timestamp();
        let end_ts = close.timestamp();
        for market in gamma::parse_markets(&event) {
            if !missing.contains(&market.bin_index) || market.token_id_yes.is_empty() {
                continue;
            }
            let history = match fetch_clob_price_history(&market.token_id_yes, start_ts, end_ts) {
                Ok(history) => history,
                Err(_) => {
                    summary.fetch_errors += 1;
                    continue;
                }
            };
            let mut hourly = BTreeMap::<DateTime<Utc>, f64>::new();
            for point in history {
                let Some(ts) = point.t.and_then(|t| Utc.timestamp_opt(t, 0).single()) else {
                    continue;
                };
                let hours_to_close = (close - ts).num_seconds() as f64 / 3600.0;
                let minute = PriceHistoryRecord {
                    city: city_name.clone(),
                    target_date: target_date.clone(),
                    bin_label: market.bin_label.clone(),
                    bin_index: Some(market.bin_index),
                    token_id_yes: Some(market.token_id_yes.clone()),
                    timestamp: ts,
                    yes_price: point.p,
                    hours_to_close: Some(hours_to_close),
                    source: "clob-backfill".to_string(),
                    minute: true,
                };
                db::insert_price_history(db, &minute)?;
                hourly.insert(truncate_hour(ts), point.p.unwrap_or(0.0));
                summary.records_saved += 1;
            }
            for (ts, price) in hourly {
                let hourly = PriceHistoryRecord {
                    city: city_name.clone(),
                    target_date: target_date.clone(),
                    bin_label: market.bin_label.clone(),
                    bin_index: Some(market.bin_index),
                    token_id_yes: Some(market.token_id_yes.clone()),
                    timestamp: ts,
                    yes_price: Some(price),
                    hours_to_close: Some((close - ts).num_seconds() as f64 / 3600.0),
                    source: "clob-backfill".to_string(),
                    minute: false,
                };
                db::insert_price_history(db, &hourly)?;
                summary.records_saved += 1;
            }
        }
    }
    Ok(summary)
}

pub fn true_gfs_model_run(target_date: NaiveDate) -> String {
    (target_date - Duration::days(1))
        .format("%Y-%m-%dT06:00")
        .to_string()
}

pub fn missing_bin_indexes(current: &BTreeSet<i64>) -> Vec<i64> {
    (0..=10)
        .filter(|idx| !current.contains(idx))
        .collect::<Vec<_>>()
}

fn contract_filter(city: Option<&str>) -> Vec<&'static config::WeatherContractConfig> {
    if let Some(city) = city {
        config::contract_for_city(city).into_iter().collect()
    } else {
        config::enabled_contracts().collect()
    }
}

fn fetch_asos_csv(station: &str, start: &str, end: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(30))
        .build()?;
    let text = client
        .get(ASOS_API)
        .query(&[
            ("station", station.to_string()),
            ("data", "tmpf,dwpf,metar".to_string()),
            ("sts", format!("{start}T00:00:00Z")),
            ("ets", format!("{end}T23:59:59Z")),
            ("format", "csv".to_string()),
        ])
        .send()?
        .error_for_status()?
        .text()?;
    Ok(text)
}

fn fetch_daily_high_asos(station: &str, target_date: &str) -> Result<Option<f64>> {
    let csv = fetch_asos_csv(station, target_date, target_date)?;
    Ok(parse_asos_rows(&csv)
        .into_iter()
        .filter_map(|row| row.tmpf)
        .reduce(f64::max))
}

#[derive(Debug, Deserialize)]
struct AsosRow {
    #[serde(default)]
    valid: Option<String>,
    #[serde(default)]
    tmpf: Option<f64>,
    #[serde(default)]
    dwpf: Option<f64>,
    #[serde(default)]
    metar: Option<String>,
}

fn parse_asos_rows(text: &str) -> Vec<AsosRow> {
    let clean = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let mut reader = csv::Reader::from_reader(clean.as_bytes());
    reader.deserialize().filter_map(Result::ok).collect()
}

fn fetch_historical_forecasts(
    lat: f64,
    lon: f64,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(30))
        .build()?;
    Ok(client
        .get(HISTORICAL_FORECAST_URL)
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("start_date", start.to_string()),
            ("end_date", end.to_string()),
            ("daily", "temperature_2m_max".to_string()),
            ("models", "ecmwf_ifs025,gfs_seamless".to_string()),
            ("temperature_unit", "fahrenheit".to_string()),
            ("timezone", "UTC".to_string()),
        ])
        .send()?
        .error_for_status()?
        .json()?)
}

fn fetch_true_gfs_forecast(lat: f64, lon: f64, target_date: NaiveDate) -> Result<Option<f64>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let raw: Value = client
        .get(HISTORICAL_FORECAST_URL)
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("start_date", target_date.to_string()),
            ("end_date", target_date.to_string()),
            ("daily", "temperature_2m_max".to_string()),
            ("models", "gfs_global".to_string()),
            ("past_model_run", true_gfs_model_run(target_date)),
            ("temperature_unit", "fahrenheit".to_string()),
            ("timezone", "UTC".to_string()),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(raw
        .get("daily")
        .and_then(|daily| daily.get("temperature_2m_max"))
        .and_then(Value::as_array)
        .and_then(|values| value_to_f64(values.first())))
}

fn historical_lookup(raw: &Value) -> BTreeMap<String, (Option<f64>, Option<f64>)> {
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
    dates
        .iter()
        .enumerate()
        .filter_map(|(idx, day)| {
            Some((
                day.as_str()?.to_string(),
                (value_to_f64(ecmwf.get(idx)), value_to_f64(gfs.get(idx))),
            ))
        })
        .collect()
}

fn settlement_rows_missing_source(
    db: &Database,
    city: &str,
    source: &str,
) -> Result<Vec<(NaiveDate, Option<String>)>> {
    let mut stmt = db.conn().prepare(
        "SELECT target_date, event_slug FROM settlement_records
         WHERE city = ?1
           AND target_date NOT IN (
               SELECT target_date FROM forecast_records WHERE city = ?1 AND source = ?2
           )
         ORDER BY target_date",
    )?;
    let rows = stmt
        .query_map((city, source), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .filter_map(Result::ok)
        .filter_map(|(d, slug)| {
            NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                .ok()
                .map(|date| (date, slug))
        })
        .collect();
    Ok(rows)
}

fn settlement_dates(db: &Database, city: &str, source: &str) -> Result<Vec<NaiveDate>> {
    let mut stmt = db.conn().prepare(
        "SELECT target_date FROM settlement_records
         WHERE city = ?1
           AND target_date NOT IN (
               SELECT target_date FROM forecast_records WHERE city = ?1 AND source = ?2
           )
         ORDER BY target_date",
    )?;
    let dates = stmt
        .query_map((city, source), |row| row.get::<_, String>(0))?
        .filter_map(Result::ok)
        .filter_map(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok())
        .collect();
    Ok(dates)
}

fn settlement_event_rows(
    db: &Database,
    city: Option<&str>,
) -> Result<Vec<(String, String, Option<String>)>> {
    let city = config::normalize_city(city);
    let (sql, param): (&str, Option<String>) = if city.is_some() {
        (
            "SELECT city, target_date, event_slug FROM settlement_records WHERE city = ?1 ORDER BY city, target_date",
            city,
        )
    } else {
        (
            "SELECT city, target_date, event_slug FROM settlement_records ORDER BY city, target_date",
            None,
        )
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = param {
        stmt.query_map([city], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

fn existing_hourly_bin_indexes(
    db: &Database,
    city: &str,
    target_date: &str,
) -> Result<BTreeSet<i64>> {
    let mut stmt = db.conn().prepare(
        "SELECT DISTINCT bin_index FROM price_history
         WHERE city = ?1 AND target_date = ?2 AND bin_index IS NOT NULL",
    )?;
    let rows = stmt
        .query_map((city, target_date), |row| row.get::<_, i64>(0))?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[derive(Debug, Deserialize)]
struct ClobPoint {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    p: Option<f64>,
}

fn fetch_clob_price_history(token_id: &str, start_ts: i64, end_ts: i64) -> Result<Vec<ClobPoint>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let payload: Value = client
        .get(CLOB_PRICE_HISTORY_URL)
        .query(&[
            ("market", token_id.to_string()),
            ("startTs", start_ts.to_string()),
            ("endTs", end_ts.to_string()),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(serde_json::from_value(
        payload
            .get("history")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    )?)
}

fn truncate_hour(ts: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::Timelike;
    ts.with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

fn temp_to_bin(temp_f: f64) -> String {
    let lo = temp_f.floor() as i64;
    format!("{lo}-{}°F", lo + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_asos_csv_rows() {
        let rows = parse_asos_rows(
            "station,valid,tmpf,dwpf,metar\nKLGA,2026-04-01T01:00:00Z,55,40,METAR\n",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tmpf, Some(55.0));
    }

    #[test]
    fn builds_temp_bin_from_settlement_temp() {
        assert_eq!(temp_to_bin(71.8), "71-72°F");
    }

    #[test]
    fn builds_python_true_gfs_model_run() {
        let target = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(true_gfs_model_run(target), "2026-03-31T06:00");
    }

    #[test]
    fn detects_missing_price_bins() {
        let current = [0, 1, 3, 10].into_iter().collect::<BTreeSet<_>>();
        assert_eq!(missing_bin_indexes(&current), vec![2, 4, 5, 6, 7, 8, 9]);
    }
}
