use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherContract {
    pub event_slug: String,
    pub target_date: String,
    pub market_type: String,
    pub measure: String,
    pub unit: String,
    pub city_slug: String,
    pub display_city: String,
    pub resolution_source_kind: String,
    pub station_id: String,
    pub source_url: String,
    pub timezone: String,
    pub lat: f64,
    pub lon: f64,
    pub validation_status: String,
    pub skip_reason: Option<String>,
    pub event_id: Option<String>,
    pub raw_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub timestamp: DateTime<Utc>,
    pub city: String,
    pub target_date: String,
    pub hours_to_close: f64,
    pub event_id: String,
    pub event_slug: String,
    pub bin_label: String,
    pub bin_index: i64,
    pub token_id_yes: String,
    pub token_id_no: String,
    pub yes_price: f64,
    pub no_price: f64,
    pub yes_best_bid: f64,
    pub yes_best_ask: f64,
    pub no_best_bid: f64,
    pub no_best_ask: f64,
    pub book_depth_bid: f64,
    pub book_depth_ask: f64,
    pub no_asks_json: Option<String>,
    pub fee_rate: Option<f64>,
    pub no_vwap_10: Option<f64>,
    pub volume_24h: f64,
    pub liquidity: f64,
    pub market_type: Option<String>,
    pub measure: Option<String>,
    pub unit: Option<String>,
    pub city_slug: Option<String>,
    pub station_id: Option<String>,
    pub source_url: Option<String>,
    pub validation_status: Option<String>,
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetarRecord {
    pub timestamp: DateTime<Utc>,
    pub icao: String,
    pub city: String,
    pub temp_c: f64,
    pub temp_f: f64,
    pub dewpoint_c: Option<f64>,
    pub obs_time: DateTime<Utc>,
    pub raw_metar: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastRecord {
    pub timestamp: DateTime<Utc>,
    pub city: String,
    pub target_date: String,
    pub source: String,
    pub temp_max_ecmwf_c: Option<f64>,
    pub temp_max_gfs_c: Option<f64>,
    pub temp_max_ecmwf_f: Option<f64>,
    pub temp_max_gfs_f: Option<f64>,
    pub model_spread_c: Option<f64>,
    pub forecast_lead_hours: f64,
    pub raw_json: String,
    pub market_type: Option<String>,
    pub measure: Option<String>,
    pub unit: Option<String>,
    pub city_slug: Option<String>,
    pub station_id: Option<String>,
    pub event_slug: Option<String>,
    pub value_ecmwf: Option<f64>,
    pub value_gfs: Option<f64>,
    pub value_unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastModelRun {
    pub timestamp: DateTime<Utc>,
    pub city: String,
    pub target_date: String,
    pub source: String,
    pub model_name: String,
    pub model_run_time: Option<String>,
    pub forecast_lead_hours: Option<f64>,
    pub temp_max_f: Option<f64>,
    pub raw_json: Option<String>,
    pub market_type: Option<String>,
    pub measure: Option<String>,
    pub unit: Option<String>,
    pub city_slug: Option<String>,
    pub station_id: Option<String>,
    pub event_slug: Option<String>,
    pub value: Option<f64>,
    pub value_unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TafRecord {
    pub timestamp: DateTime<Utc>,
    pub icao: String,
    pub city: String,
    pub issue_time: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub raw_taf: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwsForecast {
    pub timestamp: DateTime<Utc>,
    pub city: String,
    pub target_date: String,
    pub period_name: Option<String>,
    pub is_daytime: Option<bool>,
    pub temp_f: Option<f64>,
    pub short_forecast: Option<String>,
    pub forecast_lead_hours: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceHistoryRecord {
    pub city: String,
    pub target_date: String,
    pub bin_label: String,
    pub bin_index: Option<i64>,
    pub token_id_yes: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub yes_price: Option<f64>,
    pub hours_to_close: Option<f64>,
    pub source: String,
    pub minute: bool,
}
