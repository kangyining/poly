use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
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
