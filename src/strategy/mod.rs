mod calibration;
mod engine;
mod registry;
mod rules;
mod types;

pub use calibration::CalibrationTable;
pub use engine::StrategyEngine;
pub use registry::{StrategyConfig, StrategyRegistry};
pub use rules::{
    BucketProbabilityRule, MarketTimingRule, NoEdgeRule, StrategyRule, TokenAvailabilityRule,
};
pub use types::{
    DecisionTrace, EvaluatedMarket, RuleTrace, StrategyEvaluation, StrategyInput, TradeCandidate,
};
