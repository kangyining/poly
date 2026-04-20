use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};

use crate::polymarket::WeatherMarket;
use crate::services::StrategyOutcome;
use crate::weather::Forecast;

#[derive(Debug, Clone)]
pub enum Event {
    Market(MarketEvent),
    Weather(WeatherEvent),
    Strategy(StrategyEvent),
    Trading(TradingEvent),
    System(SystemEvent),
}

#[derive(Debug, Clone)]
pub enum EventSource {
    App,
    MarketWs,
    WeatherPoller,
    StrategyEngine,
    ExecutionEngine,
    StateListener,
    TriggerListener,
}

#[derive(Debug, Clone)]
pub enum MarketEvent {
    MetadataRefreshed(MarketMetadataRefreshed),
    BestBidAskChanged(BestBidAskChanged),
    ConnectionStatusChanged(ConnectionStatusChanged),
}

#[derive(Debug, Clone)]
pub struct MarketMetadataRefreshed {
    pub event_slug: String,
    pub target_date: NaiveDate,
    pub markets: Arc<Vec<WeatherMarket>>,
}

#[derive(Debug, Clone)]
pub struct BestBidAskChanged {
    pub token_id: String,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ConnectionStatusChanged {
    pub connected: bool,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum WeatherEvent {
    ForecastUpdated(ForecastUpdated),
}

#[derive(Debug, Clone)]
pub struct ForecastUpdated {
    pub previous: Option<Forecast>,
    pub current: Forecast,
}

#[derive(Debug, Clone)]
pub enum StrategyEvent {
    EvaluationRequested(EvaluationRequested),
    EvaluationCompleted(Arc<StrategyOutcome>),
}

#[derive(Debug, Clone)]
pub struct EvaluationRequested {
    pub reason: EvaluationReason,
}

#[derive(Debug, Clone)]
pub enum EvaluationReason {
    Startup,
    WeatherChanged,
    MarketMetadataChanged,
    MarketPriceChanged { token_id: String },
}

#[derive(Debug, Clone)]
pub enum TradingEvent {
    ExecutionPlanCreated(ExecutionPlanCreated),
    OrderSubmitted(OrderSubmitted),
    OrderSkipped(OrderSkipped),
}

#[derive(Debug, Clone)]
pub struct ExecutionPlanCreated {
    pub run_id: String,
    pub trade_count: usize,
    pub total_stake_usd: f64,
}

#[derive(Debug, Clone)]
pub struct OrderSubmitted {
    pub run_id: String,
    pub token_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OrderSkipped {
    pub run_id: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum SystemEvent {
    Started,
    Stopping,
    SourceLagged {
        source: EventSource,
        dropped_messages: u64,
    },
    ListenerFailed {
        listener: &'static str,
        error: String,
    },
    Healthy {
        source: EventSource,
        at: DateTime<Utc>,
    },
}
