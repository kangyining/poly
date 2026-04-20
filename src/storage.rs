use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;

use crate::execution::{ExecutionPlan, ExecutionReport};
use crate::strategy::{StrategyEvaluation, StrategyInput};
use crate::weather::Forecast;

#[derive(Debug, Clone)]
pub struct JsonlStore {
    path: PathBuf,
}

impl JsonlStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn persist_run_started(&self, input: &StrategyInput, forecast: &Forecast) -> Result<()> {
        self.append(json!({
            "record_type": "run_started",
            "recorded_at": Utc::now(),
            "run_id": input.run_id,
            "event_slug": input.event_slug,
            "target_date": input.forecast.target_date,
            "forecast_high_f": forecast.forecast_high_f,
            "forecast_source": forecast.source,
            "forecast_fetched_at": forecast.fetched_at,
            "distance_threshold_f": input.distance_threshold_f,
            "price_min": input.price_band.min,
            "price_max": input.price_band.max,
            "order_size_usd": input.order_size_usd,
        }))
    }

    pub fn persist_strategy_evaluation(&self, evaluation: &StrategyEvaluation) -> Result<()> {
        for market in &evaluation.evaluated_markets {
            self.append(json!({
                "record_type": "strategy_decision",
                "recorded_at": Utc::now(),
                "decision_id": market.decision_id,
                "market_id": market.market_id,
                "question": market.question,
                "accepted": market.accepted,
                "rules": market.traces.iter().map(|trace| {
                    json!({
                        "rule_name": trace.rule_name,
                        "passed": trace.passed,
                        "reason": trace.reason,
                    })
                }).collect::<Vec<_>>(),
            }))?;
        }

        Ok(())
    }

    pub fn persist_execution_plan(&self, plan: &ExecutionPlan) -> Result<()> {
        self.append(json!({
            "record_type": "execution_plan",
            "recorded_at": Utc::now(),
            "trade_count": plan.trades.len(),
            "total_stake_usd": plan.total_stake_usd,
            "trades": plan.trades.iter().map(|trade| {
                json!({
                    "decision_id": trade.decision_id,
                    "market_id": trade.market_id,
                    "question": trade.question,
                    "side": trade.side,
                    "token_id": trade.token_id,
                    "price": trade.price,
                    "size_usd": trade.size_usd,
                })
            }).collect::<Vec<_>>(),
        }))
    }

    pub fn persist_execution_report(&self, run_id: &str, report: &ExecutionReport) -> Result<()> {
        self.append(json!({
            "record_type": "execution_report",
            "recorded_at": Utc::now(),
            "run_id": run_id,
            "attempted": report.attempted,
            "submitted": report.submitted,
            "skipped": report.skipped,
        }))
    }

    pub fn persist_order_submission(
        &self,
        run_id: &str,
        decision_id: &str,
        token_id: &str,
        market_id: &str,
    ) -> Result<()> {
        self.append(json!({
            "record_type": "order_submission",
            "recorded_at": Utc::now(),
            "run_id": run_id,
            "decision_id": decision_id,
            "token_id": token_id,
            "market_id": market_id,
        }))
    }

    fn append(&self, value: serde_json::Value) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open storage file {}", self.path.display()))?;

        serde_json::to_writer(&mut file, &value)
            .with_context(|| format!("failed to write storage record {}", self.path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to terminate storage record {}", self.path.display()))
    }
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create storage directory {}", parent.display()))?;
    }
    Ok(())
}
