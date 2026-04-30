use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

use crate::config::AppConfig;
use crate::market::TradingClient;
use crate::strategy::TradeCandidate;

use super::StrategyOutcome;

pub struct ExecutionService {
    config: AppConfig,
    trading_client: TradingClient,
}

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
    pub strategy_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub run_id: String,
    pub event_slug: String,
    pub target_date: String,
    pub dry_run: bool,
    pub executed_at: String,
    pub total_stake_usd: f64,
    pub trades: Vec<ExecutedTrade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedTrade {
    pub decision_id: String,
    pub market_id: String,
    pub question: String,
    pub bin_label: String,
    pub side: String,
    pub token_id: String,
    pub effective_ask: f64,
    pub shares: f64,
    pub stake_usd: f64,
    pub p_model: f64,
    pub ev_no: f64,
    pub fee_rate: f64,
    pub strategy_name: String,
    pub order_id: Option<String>,
    pub executed_at: String,
    pub is_paper: bool,
}

impl ExecutionPlan {
    pub fn from_candidates(
        candidates: &[TradeCandidate],
        run_id: &str,
        event_slug: &str,
        target_date: &str,
        dry_run: bool,
        strategy_name: &str,
    ) -> Self {
        let trades = candidates
            .iter()
            .map(|candidate| PlannedTrade {
                decision_id: candidate.decision_id.clone(),
                market_id: candidate.market_id.clone(),
                question: candidate.question.clone(),
                bin_label: candidate.range.label(),
                side: "NO",
                no_token_id: candidate.no_token_id.clone(),
                effective_ask: candidate.effective_ask,
                shares: candidate.shares,
                stake_usd: candidate.stake_usd,
                p_model: candidate.p_model,
                ev_no: candidate.ev_no,
                fee_rate: candidate.fee_rate,
                strategy_name: strategy_name.to_string(),
            })
            .collect::<Vec<_>>();

        let total_stake_usd = trades.iter().map(|trade| trade.stake_usd).sum();

        Self {
            run_id: run_id.to_string(),
            event_slug: event_slug.to_string(),
            target_date: target_date.to_string(),
            total_stake_usd,
            dry_run,
            trades,
        }
    }

    pub fn write_to_dir(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("execution_plan.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, &json)?;
        info!(
            run_id = %self.run_id,
            path = %path.display(),
            trade_count = self.trades.len(),
            total_stake_usd = self.total_stake_usd,
            dry_run = self.dry_run,
            "execution plan written"
        );
        Ok(())
    }
}

impl ExecutionService {
    pub fn new(config: AppConfig) -> Self {
        let trading_client = TradingClient::new(&config.clob_url);
        Self {
            config,
            trading_client,
        }
    }

    pub async fn execute(&self, outcome: &StrategyOutcome) -> Result<ExecutionReport> {
        outcome.plan.write_to_dir(&self.config.execution_plan_dir)?;
        self.execute_plan(&outcome.plan).await
    }

    #[instrument(
        name = "execute_plan",
        skip(self, plan),
        fields(
            run_id = %plan.run_id,
            event_slug = %plan.event_slug,
            target_date = %plan.target_date
        )
    )]
    async fn execute_plan(&self, plan: &ExecutionPlan) -> Result<ExecutionReport> {
        info!(run_id = %plan.run_id, trade_count = plan.trades.len(), "executing plan");

        let dry_run = plan.dry_run;

        let mut executed_trades = Vec::with_capacity(plan.trades.len());
        for trade in &plan.trades {
            executed_trades.push(self.execute_trade(&plan.run_id, trade, dry_run).await?);
        }

        Ok(ExecutionReport {
            run_id: plan.run_id.clone(),
            event_slug: plan.event_slug.clone(),
            target_date: plan.target_date.clone(),
            dry_run,
            executed_at: Utc::now().to_rfc3339(),
            total_stake_usd: plan.total_stake_usd,
            trades: executed_trades,
        })
    }

    fn build_executed_trade(
        &self,
        trade: &PlannedTrade,
        order_id: Option<String>,
        is_paper: bool,
    ) -> ExecutedTrade {
        ExecutedTrade {
            decision_id: trade.decision_id.clone(),
            market_id: trade.market_id.clone(),
            question: trade.question.clone(),
            bin_label: trade.bin_label.clone(),
            side: trade.side.to_string(),
            token_id: trade.no_token_id.clone(),
            effective_ask: trade.effective_ask,
            shares: trade.shares,
            stake_usd: trade.stake_usd,
            p_model: trade.p_model,
            ev_no: trade.ev_no,
            fee_rate: trade.fee_rate,
            strategy_name: trade.strategy_name.clone(),
            order_id,
            executed_at: Utc::now().to_rfc3339(),
            is_paper,
        }
    }

    #[instrument(
        name = "execute_trade",
        skip(self, trade),
        fields(
            run_id = %run_id,
            decision_id = %trade.decision_id,
            market_id = %trade.market_id,
            token_id = %trade.no_token_id
        )
    )]
    async fn execute_trade(
        &self,
        run_id: &str,
        trade: &PlannedTrade,
        dry_run: bool,
    ) -> Result<ExecutedTrade> {
        info!(
            decision_id = %trade.decision_id,
            run_id = %run_id,
            mode = if dry_run { "paper" } else { "live" },
            bin = %trade.bin_label,
            effective_ask = trade.effective_ask,
            shares = trade.shares,
            stake_usd = trade.stake_usd,
            "processing planned trade"
        );

        let order_id = if dry_run {
            None
        } else {
            Some(self.submit_live_trade(trade).await?)
        };

        Ok(self.build_executed_trade(trade, order_id, dry_run))
    }

    #[instrument(
        name = "submit_live_trade",
        skip(self, trade),
        fields(
            decision_id = %trade.decision_id,
            market_id = %trade.market_id,
            token_id = %trade.no_token_id
        )
    )]
    async fn submit_live_trade(&self, trade: &PlannedTrade) -> Result<String> {
        let order_id = self
            .trading_client
            .submit_no_buy_order(&trade.no_token_id, trade.effective_ask, trade.shares)
            .await?;

        info!(
            decision_id = %trade.decision_id,
            bin = %trade.bin_label,
            order_id = %order_id,
            "live order placed"
        );
        Ok(order_id)
    }
}
