use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::config::PriceBand;
use crate::polymarket::{TemperatureRange, WeatherMarket};
use crate::weather::Forecast;

#[derive(Debug, Clone)]
pub struct StrategyInput {
    pub forecast: Forecast,
    pub price_band: PriceBand,
    pub distance_threshold_f: f64,
    pub order_size_usd: f64,
    pub run_id: String,
    pub event_slug: String,
    pub evaluation_time_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct TradeCandidate {
    pub market_id: String,
    pub question: String,
    pub slug: Option<String>,
    pub range: TemperatureRange,
    pub distance_f: f64,
    pub no_price: f64,
    pub yes_price: Option<f64>,
    pub no_token_id: Option<String>,
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
    pub no_price: Option<f64>,
    pub distance_f: Option<f64>,
    pub traces: Vec<RuleTrace>,
}

impl DecisionTrace {
    pub fn new(decision_id: String, market: &WeatherMarket) -> Self {
        Self {
            decision_id,
            market_id: market.id.clone(),
            market_question: market.question.clone(),
            no_price: market.no_price,
            distance_f: None,
            traces: Vec::new(),
        }
    }

    pub fn push(&mut self, trace: RuleTrace) {
        self.traces.push(trace);
    }

    pub fn set_distance_f(&mut self, distance_f: f64) {
        self.distance_f = Some(distance_f);
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
    let naive_close = target_date
        .and_hms_opt(12, 0, 0)
        .expect("12:00:00 is a valid UTC close time");
    DateTime::<Utc>::from_naive_utc_and_offset(naive_close, Utc)
}

pub fn within_24h_of_close(now: DateTime<Utc>, target_date: NaiveDate) -> bool {
    let close_time = market_close_utc(target_date);
    let earliest_entry = close_time - Duration::hours(24);
    now >= earliest_entry && now <= close_time
}
