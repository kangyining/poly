use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastState {
    pub source: String,
    pub forecast_f: f64,
    pub lead_hours: f64,
    pub beta_f: f64,
    pub sigma_f: f64,
    pub mu_f: f64,
    pub gfs_f: Option<f64>,
    pub ecmwf_f: Option<f64>,
    pub model_spread_f: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketBin {
    pub bin_label: String,
    pub bin_index: i64,
    pub yes_price: f64,
    pub no_price: f64,
    pub effective_ask_no: f64,
    pub fee_rate: f64,
    pub hours_to_close: f64,
    pub token_id_yes: Option<String>,
    pub token_id_no: Option<String>,
    pub no_best_bid: Option<f64>,
    pub no_best_ask: Option<f64>,
    pub fill_source: String,
    pub opened_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StrategyContext {
    pub city: String,
    pub target_date: String,
    pub winning_bin: String,
    pub bins: Vec<MarketBin>,
    pub forecast: Option<ForecastState>,
    pub params: Value,
    pub replay_mode: String,
    pub evaluation_time: Option<DateTime<Utc>>,
    pub event_slug: Option<String>,
    pub settlement_temp_f: Option<f64>,
    pub metar_max_so_far_f: Option<f64>,
    pub climatology_temps_f: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyCandidate {
    pub strategy_name: String,
    pub side: String,
    pub bin_label: String,
    pub effective_ask: f64,
    pub fee_rate: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub p_model: Option<f64>,
    pub edge: f64,
    pub fill_source: String,
    pub market_id: String,
    pub token_id: String,
    pub event_slug: String,
    pub opened_at: Option<String>,
    pub settled_at: Option<String>,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDiagnostic {
    pub strategy_name: String,
    pub severity: String,
    pub reason: String,
    pub required_data: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StrategyOutput {
    pub candidates: Vec<StrategyCandidate>,
    pub diagnostics: Vec<StrategyDiagnostic>,
}

pub trait Strategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn required_data(&self) -> &'static [&'static str] {
        &[]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput;
}

pub fn missing(strategy_name: &str, reason: &str, required_data: &[&str]) -> StrategyOutput {
    StrategyOutput {
        candidates: Vec::new(),
        diagnostics: vec![StrategyDiagnostic {
            strategy_name: strategy_name.to_string(),
            severity: "missing_data".to_string(),
            reason: reason.to_string(),
            required_data: required_data.iter().map(|s| s.to_string()).collect(),
        }],
    }
}
