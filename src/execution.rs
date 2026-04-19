use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::strategy::TradeCandidate;

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPlan {
    pub run_id: String,
    pub event_slug: String,
    pub target_date: String,
    pub total_stake_usd: f64,
    pub dry_run: bool,
    pub trades: Vec<PlannedTrade>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTrade {
    pub decision_id: String,
    pub market_id: String,
    pub question: String,
    pub bin_label: String,
    pub side: &'static str,
    pub no_token_id: String,
    pub effective_ask: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub p_model: f64,
    pub ev_no: f64,
    pub fee_rate: f64,
}

impl ExecutionPlan {
    pub fn from_candidates(
        candidates: &[TradeCandidate],
        run_id: &str,
        event_slug: &str,
        target_date: &str,
        dry_run: bool,
    ) -> Self {
        let trades: Vec<PlannedTrade> = candidates
            .iter()
            .map(|c| PlannedTrade {
                decision_id: c.decision_id.clone(),
                market_id: c.market_id.clone(),
                question: c.question.clone(),
                bin_label: c.range.label(),
                side: "NO",
                no_token_id: c.no_token_id.clone(),
                effective_ask: c.effective_ask,
                shares: c.shares,
                stake_usd: c.stake_usd,
                p_model: c.p_model,
                ev_no: c.ev_no,
                fee_rate: c.fee_rate,
            })
            .collect();

        let total_stake_usd = trades.iter().map(|t| t.stake_usd).sum();

        Self {
            run_id: run_id.to_string(),
            event_slug: event_slug.to_string(),
            target_date: target_date.to_string(),
            total_stake_usd,
            dry_run,
            trades,
        }
    }

    /// Write the plan to `{dir}/execution_plan.json`.
    /// If dry_run is true, write with `"dry_run": true` — the Python bridge
    /// will read this flag and skip order submission.
    pub fn write_to_dir(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("execution_plan.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, &json)?;
        info!(
            path = %path.display(),
            trade_count = self.trades.len(),
            total_stake_usd = self.total_stake_usd,
            dry_run = self.dry_run,
            "execution plan written"
        );
        Ok(())
    }
}
