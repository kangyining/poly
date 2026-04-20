use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_BETA_F: f64 = 0.0;
const DEFAULT_SIGMA_F: f64 = 2.0;
const SIGMA_MIN: f64 = 0.8;
const SIGMA_MAX: f64 = 4.0;
const MIN_SAMPLES: u64 = 10;

#[derive(Debug, Clone, Deserialize)]
pub struct CalibCell {
    pub beta_f: f64,
    pub sigma_f: f64,
    pub n: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CalibrationTable {
    // cells[station][month_str][lead_bin]
    cells: HashMap<String, HashMap<String, HashMap<String, CalibCell>>>,
    #[serde(rename = "_defaults")]
    defaults: HashMap<String, CalibCell>,
}

impl CalibrationTable {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read calibration file: {}", path.display()))?;
        let table: Self = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse calibration JSON: {}", path.display()))?;
        Ok(table)
    }

    /// Returns (beta_f, sigma_f) with fallback chain:
    ///   cell (station, month, lead_bin) → station default → global default
    pub fn lookup(&self, station: &str, date: NaiveDate, lead_hours: f64) -> (f64, f64) {
        let month = date.format("%m").to_string().trim_start_matches('0').to_string();
        let lb = lead_bin(lead_hours);

        // Try cell lookup
        if let Some(cell) = self
            .cells
            .get(station)
            .and_then(|m| m.get(&month))
            .and_then(|m| m.get(lb))
        {
            if cell.n >= MIN_SAMPLES {
                return (cell.beta_f, clamp(cell.sigma_f, SIGMA_MIN, SIGMA_MAX));
            }
        }

        // Station default
        if let Some(def) = self.defaults.get(station) {
            return (def.beta_f, clamp(def.sigma_f, SIGMA_MIN, SIGMA_MAX));
        }

        // Global default
        if let Some(global) = self.defaults.get("_global") {
            return (global.beta_f, clamp(global.sigma_f, SIGMA_MIN, SIGMA_MAX));
        }

        (DEFAULT_BETA_F, DEFAULT_SIGMA_F)
    }
}

fn lead_bin(hours: f64) -> &'static str {
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

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}
