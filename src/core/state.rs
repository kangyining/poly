use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::market::WeatherMarket;
use crate::weather::Forecast;

#[derive(Debug, Clone, Copy, Default)]
pub enum LifecycleState {
    #[default]
    Starting,
    WarmingUp,
    Ready,
    Evaluating,
    Executing,
    Degraded,
}

#[derive(Debug, Default)]
pub struct BotState {
    pub lifecycle: LifecycleState,
    pub market: MarketState,
    pub weather: WeatherState,
    pub execution: ExecutionState,
    pub performance: PerformanceState,
}

#[derive(Debug, Default)]
pub struct MarketState {
    pub event_slug: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub markets: Vec<WeatherMarket>,
    pub best_asks: HashMap<String, f64>,
    pub best_bids: HashMap<String, f64>,
    pub last_metadata_at: Option<DateTime<Utc>>,
    pub last_price_event_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct WeatherState {
    pub forecast: Option<Forecast>,
    pub last_update_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct ExecutionState {
    pub submitted_tokens: HashSet<String>,
    pub last_run_id: Option<String>,
    pub last_evaluation_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct PerformanceState {
    pub starting_balance_usd: f64,
    pub cash_usd: f64,
    pub open_exposure_usd: f64,
    pub realized_pnl_usd: f64,
    pub equity_usd: f64,
    pub open_position_count: usize,
    pub closed_position_count: usize,
    pub action_count: u64,
    pub last_action: Option<PerformanceActionState>,
    pub action_history: Vec<PerformanceActionState>,
    pub last_updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceActionState {
    pub sequence: u64,
    pub logged_at: Option<DateTime<Utc>>,
    pub action: String,
    pub run_id: Option<String>,
    pub decision_id: Option<String>,
    pub event_slug: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub market_id: Option<String>,
    pub bin_label: Option<String>,
    pub token_id: Option<String>,
    pub order_id: Option<String>,
    pub resolution_bin_label: Option<String>,
    pub is_paper: bool,
    pub status: Option<String>,
    pub shares: f64,
    pub entry_price: f64,
    pub stake_usd: f64,
    pub payout_usd: Option<f64>,
    pub pnl_usd: Option<f64>,
    pub cash_before_usd: Option<f64>,
    pub cash_after_usd: Option<f64>,
    pub open_exposure_after_usd: Option<f64>,
    pub realized_pnl_after_usd: Option<f64>,
    pub equity_after_usd: Option<f64>,
    pub starting_balance_usd: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRationale {
    pub ev_no: f64,
    pub p_model: f64,
    pub fee_rate: f64,
    pub effective_ask: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPosition {
    pub token_id: String,
    #[serde(default)]
    pub market_id: String,
    pub bin_label: String,
    pub event_slug: String,
    pub target_date: String,
    pub entry_price: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub order_id: Option<String>,
    pub status: String,
    pub pnl: Option<f64>,
    pub opened_at: String,
    pub settled_at: Option<String>,
    pub is_paper: bool,
    #[serde(default = "default_strategy_name")]
    pub strategy_name: String,
    pub backtest_id: Option<String>,
    pub trade_rationale: Option<TradeRationale>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub decision_id: Option<String>,
    #[serde(default)]
    pub payout_usd: Option<f64>,
    #[serde(default)]
    pub resolution_bin_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioState {
    pub starting_balance_usd: f64,
    pub cash_usd: f64,
    pub realized_pnl_usd: f64,
    pub equity_usd: f64,
    pub last_updated_at: String,
    #[serde(default = "default_next_sequence")]
    pub next_sequence: u64,
    #[serde(default)]
    pub positions: Vec<PortfolioPosition>,
    #[serde(default)]
    pub action_history: Vec<PerformanceActionState>,
}

impl PortfolioState {
    pub fn refresh_summary(&mut self) {
        self.realized_pnl_usd = self.positions.iter().filter_map(|position| position.pnl).sum();
        self.equity_usd = self.cash_usd + self.open_exposure_usd();
    }

    pub fn open_exposure_usd(&self) -> f64 {
        self.positions
            .iter()
            .filter(|position| position.status == "open" && position.is_paper)
            .map(|position| position.stake_usd)
            .sum()
    }
}

impl BotState {
    pub fn is_ready(&self) -> bool {
        self.weather.forecast.is_some() && !self.market.markets.is_empty()
    }
}

pub type SharedState = Arc<RwLock<BotState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(RwLock::new(BotState {
        lifecycle: LifecycleState::Starting,
        ..BotState::default()
    }))
}

fn default_strategy_name() -> String {
    "default".to_string()
}

fn default_next_sequence() -> u64 {
    1
}
