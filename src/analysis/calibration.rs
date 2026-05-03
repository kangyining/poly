use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::db::Database;

const MIN_SAMPLES: i64 = 10;
const DEFAULT_BETA_F: f64 = 0.0;
const DEFAULT_SIGMA_F: f64 = 2.0;
const SIGMA_MIN: f64 = 0.8;
const SIGMA_MAX: f64 = 4.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CalibrationCell {
    pub beta_f: f64,
    pub sigma_f: f64,
    #[serde(default)]
    pub n: i64,
}

pub fn load(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read calibration file {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse calibration file {}", path.display()))
}

pub fn lead_bin(hours: f64) -> &'static str {
    if hours < 6.0 {
        "lt6"
    } else if hours < 12.0 {
        "6_12"
    } else if hours < 24.0 {
        "12_24"
    } else if hours < 36.0 {
        "24_36"
    } else if hours < 48.0 {
        "36_48"
    } else if hours < 72.0 {
        "48_72"
    } else {
        "72_plus"
    }
}

pub fn lookup(table: &Value, station: &str, month: u32, lead_hours: f64) -> (f64, f64) {
    let bin = lead_bin(lead_hours);
    let month = month.to_string();
    if let Some(cell) = table
        .get("cells")
        .and_then(|v| v.get(station))
        .and_then(|v| v.get(&month))
        .and_then(|v| v.get(bin))
    {
        if cell.get("n").and_then(Value::as_i64).unwrap_or(0) >= MIN_SAMPLES {
            return (
                cell.get("beta_f")
                    .and_then(Value::as_f64)
                    .unwrap_or(DEFAULT_BETA_F),
                cell.get("sigma_f")
                    .and_then(Value::as_f64)
                    .unwrap_or(DEFAULT_SIGMA_F),
            );
        }
    }
    if let Some(defaults) = table.get("_defaults").and_then(|v| v.get(station)) {
        return (
            defaults
                .get("beta_f")
                .and_then(Value::as_f64)
                .unwrap_or(DEFAULT_BETA_F),
            defaults
                .get("sigma_f")
                .and_then(Value::as_f64)
                .unwrap_or(DEFAULT_SIGMA_F),
        );
    }
    if let Some(defaults) = table.get("_defaults").and_then(|v| v.get("_global")) {
        return (
            defaults
                .get("beta_f")
                .and_then(Value::as_f64)
                .unwrap_or(DEFAULT_BETA_F),
            defaults
                .get("sigma_f")
                .and_then(Value::as_f64)
                .unwrap_or(DEFAULT_SIGMA_F),
        );
    }
    (DEFAULT_BETA_F, DEFAULT_SIGMA_F)
}

#[derive(Debug, Clone)]
struct ErrorRow {
    station: String,
    month: String,
    lead_bin: String,
    error: f64,
}

pub fn build_table(db: &Database) -> Result<Value> {
    let settlement_temp_station = optional_column_expr(
        db,
        "settlement_records",
        "settlement_temp_station",
        "settlement_temp_station",
    )?;
    let sql = format!(
        "SELECT
            f.city,
            f.target_date,
            f.temp_max_gfs_f AS forecast_f,
            f.forecast_lead_hours,
            f.station_id AS forecast_station,
            s.settlement_temp_f,
            s.station_id AS settlement_station,
            {settlement_temp_station},
            s.wunderground_station
         FROM forecast_records f
         JOIN settlement_records s
           ON f.city = s.city AND f.target_date = s.target_date
         WHERE f.source = 'gfs-global-24h'
           AND f.temp_max_gfs_f IS NOT NULL
           AND s.settlement_temp_f IS NOT NULL
         ORDER BY f.city, f.target_date"
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let mut rows = Vec::new();
    for row in stmt.query_map(params![], |row| {
        Ok((
            row.get::<_, String>("city")?,
            row.get::<_, String>("target_date")?,
            row.get::<_, f64>("forecast_f")?,
            row.get::<_, Option<f64>>("forecast_lead_hours")?,
            row.get::<_, Option<String>>("forecast_station")?,
            row.get::<_, f64>("settlement_temp_f")?,
            row.get::<_, Option<String>>("settlement_station")?,
            row.get::<_, Option<String>>("settlement_temp_station")?,
            row.get::<_, Option<String>>("wunderground_station")?,
        ))
    })? {
        let (
            city,
            target_date,
            forecast_f,
            forecast_lead_hours,
            forecast_station,
            settlement_temp_f,
            settlement_station,
            settlement_temp_station,
            wunderground_station,
        ) = row?;
        let station = settlement_station
            .or(settlement_temp_station)
            .or(wunderground_station)
            .or(forecast_station)
            .unwrap_or(city);
        let target = NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?;
        rows.push(ErrorRow {
            station,
            month: target.month().to_string(),
            lead_bin: lead_bin(forecast_lead_hours.unwrap_or(26.0)).to_string(),
            error: settlement_temp_f - forecast_f,
        });
    }

    let mut cells: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
    let mut station_errors: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut all_errors = Vec::new();
    for row in rows {
        cells
            .entry((row.station.clone(), row.month, row.lead_bin))
            .or_default()
            .push(row.error);
        station_errors
            .entry(row.station)
            .or_default()
            .push(row.error);
        all_errors.push(row.error);
    }

    let mut table = serde_json::Map::new();
    for ((station, month, lb), errors) in cells {
        let (beta, sigma) = fit(&errors);
        table
            .entry(station)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap()
            .entry(month)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap()
            .insert(
                lb,
                json!({"beta_f": round4(beta), "sigma_f": round4(sigma), "n": errors.len()}),
            );
    }
    let mut defaults = serde_json::Map::new();
    for (station, errors) in station_errors {
        let (beta, sigma) = fit(&errors);
        defaults.insert(
            station,
            json!({"beta_f": round4(beta), "sigma_f": round4(sigma), "n": errors.len()}),
        );
    }
    let (global_beta, global_sigma) = fit(&all_errors);
    defaults.insert(
        "_global".to_string(),
        json!({"beta_f": round4(global_beta), "sigma_f": round4(global_sigma), "n": all_errors.len()}),
    );
    Ok(json!({"cells": table, "_defaults": defaults}))
}

pub fn print_table(table: &Value) {
    println!("station,month,lead_bin,beta_f,sigma_f,n");
    if let Some(cells) = table.get("cells").and_then(Value::as_object) {
        for (station, months) in cells {
            let Some(months) = months.as_object() else {
                continue;
            };
            for (month, bins) in months {
                let Some(bins) = bins.as_object() else {
                    continue;
                };
                for (lead_bin, cell) in bins {
                    println!(
                        "{station},{month},{lead_bin},{},{},{}",
                        cell.get("beta_f").and_then(Value::as_f64).unwrap_or(0.0),
                        cell.get("sigma_f").and_then(Value::as_f64).unwrap_or(0.0),
                        cell.get("n").and_then(Value::as_i64).unwrap_or(0),
                    );
                }
            }
        }
    }
    if let Some(defaults) = table.get("_defaults").and_then(Value::as_object) {
        for (station, cell) in defaults {
            println!(
                "{station},default,default,{},{},{}",
                cell.get("beta_f").and_then(Value::as_f64).unwrap_or(0.0),
                cell.get("sigma_f").and_then(Value::as_f64).unwrap_or(0.0),
                cell.get("n").and_then(Value::as_i64).unwrap_or(0),
            );
        }
    }
}

fn optional_column_expr(db: &Database, table: &str, column: &str, alias: &str) -> Result<String> {
    let mut stmt = db.conn().prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if names.iter().any(|name| name == column) {
        Ok(format!("s.{column} AS {alias}"))
    } else {
        Ok(format!("NULL AS {alias}"))
    }
}

fn fit(errors: &[f64]) -> (f64, f64) {
    if errors.is_empty() {
        return (DEFAULT_BETA_F, DEFAULT_SIGMA_F);
    }
    let beta = errors.iter().sum::<f64>() / errors.len() as f64;
    let sigma = if errors.len() > 1 {
        let variance =
            errors.iter().map(|v| (v - beta).powi(2)).sum::<f64>() / (errors.len() - 1) as f64;
        variance.sqrt().clamp(SIGMA_MIN, SIGMA_MAX)
    } else {
        DEFAULT_SIGMA_F
    };
    (beta, sigma)
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_lead_hours() {
        assert_eq!(lead_bin(4.0), "lt6");
        assert_eq!(lead_bin(24.0), "24_36");
        assert_eq!(lead_bin(80.0), "72_plus");
    }

    #[test]
    fn lookup_uses_cell_only_after_minimum_samples_then_defaults() {
        let table = json!({
            "cells": {
                "KLGA": {
                    "4": {
                        "12_24": {"beta_f": 9.0, "sigma_f": 1.0, "n": 2},
                        "24_36": {"beta_f": 1.5, "sigma_f": 2.5, "n": 10}
                    }
                }
            },
            "_defaults": {
                "KLGA": {"beta_f": 0.25, "sigma_f": 1.75, "n": 20},
                "_global": {"beta_f": -0.5, "sigma_f": 2.0, "n": 100}
            }
        });
        assert_eq!(lookup(&table, "KLGA", 4, 26.0), (1.5, 2.5));
        assert_eq!(lookup(&table, "KLGA", 4, 18.0), (0.25, 1.75));
        assert_eq!(lookup(&table, "KXXX", 4, 18.0), (-0.5, 2.0));
    }
}
