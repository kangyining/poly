use chrono::{DateTime, NaiveDate, Utc};

use crate::market::{TemperatureRange, WeatherMarket};
use crate::weather::Forecast;

#[derive(Debug, Clone)]
pub struct StrategyInput {
    pub strategy_name: String,
    pub forecast: Forecast,
    /// Calibrated mean: forecast_high_f + beta_f
    pub mu_f: f64,
    /// Calibrated error std dev (clamped to [0.8, 4.0])
    pub sigma_f: f64,
    /// Minimum fee-adjusted EV per share to qualify a bin
    pub min_ev_threshold: f64,
    pub context_key: String,
    pub city_slug: String,
    pub target_date: NaiveDate,
    pub run_id: String,
    pub event_slug: String,
    pub evaluation_time_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct TradeCandidate {
    pub strategy_name: String,
    pub context_key: String,
    pub city_slug: String,
    pub target_date: NaiveDate,
    pub event_slug: String,
    pub market_id: String,
    pub question: String,
    pub slug: Option<String>,
    pub range: TemperatureRange,
    pub p_model: f64,
    pub ev_no: f64,
    pub effective_ask: f64,
    pub min_order_size: Option<f64>,
    pub fee_rate: f64,
    pub no_token_id: String,
    pub shares: f64,
    pub stake_usd: f64,
    pub decision_id: String,
}

#[derive(Debug, Clone)]
pub struct StrategyEvaluation {
    pub candidates: Vec<TradeCandidate>,
    pub evaluated_markets: Vec<EvaluatedMarket>,
}

#[derive(Debug, Clone)]
pub struct EvaluatedMarket {
    pub decision_id: String,
    pub market_id: String,
    pub question: String,
    pub accepted: bool,
    pub traces: Vec<RuleTrace>,
}

#[derive(Debug, Clone)]
pub struct RuleTrace {
    pub rule_name: &'static str,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct DecisionTrace {
    pub decision_id: String,
    pub market_id: String,
    pub market_question: String,
    pub traces: Vec<RuleTrace>,
}

impl DecisionTrace {
    pub fn new(decision_id: String, market: &WeatherMarket) -> Self {
        Self {
            decision_id,
            market_id: market.id.clone(),
            market_question: market.question.clone(),
            traces: Vec::new(),
        }
    }

    pub fn push(&mut self, trace: RuleTrace) {
        self.traces.push(trace);
    }

    pub fn into_evaluated_market(self, accepted: bool) -> EvaluatedMarket {
        EvaluatedMarket {
            decision_id: self.decision_id,
            market_id: self.market_id,
            question: self.market_question,
            accepted,
            traces: self.traces,
        }
    }
}

pub fn market_close_utc(target_date: NaiveDate) -> DateTime<Utc> {
    let naive = target_date
        .and_hms_opt(12, 0, 0)
        .expect("12:00:00 is always valid");
    DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
}

pub fn within_trading_window(now: DateTime<Utc>, target_date: NaiveDate) -> bool {
    let close = market_close_utc(target_date);
    let open = close - chrono::Duration::hours(36);
    now >= open && now < close
}
