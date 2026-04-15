mod engine;
mod rules;
mod types;

pub use engine::StrategyEngine;
pub use rules::{
    MarketTimingRule, NoPriceBandRule, StrategyRule, TemperatureDistanceRule, TokenAvailabilityRule,
};
pub use types::{
    DecisionTrace, EvaluatedMarket, RuleTrace, StrategyEvaluation, StrategyInput, TradeCandidate,
};
