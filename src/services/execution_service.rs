use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::AppConfig;
use crate::market::TradingClient;
use crate::services::execution_plan::{ExecutionPlan, PlannedTrade};

use super::StrategyOutcome;

pub struct ExecutionService {
    config: AppConfig,
    trading_client: TradingClient,
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

    async fn execute_plan(&self, plan: &ExecutionPlan) -> Result<ExecutionReport> {
        info!(run_id = %plan.run_id, trade_count = plan.trades.len(), "executing plan");

        let dry_run = plan.dry_run;

        let mut executed_trades = Vec::with_capacity(plan.trades.len());
        for trade in &plan.trades {
            info!(
                decision_id = %trade.decision_id,
                run_id = %plan.run_id,
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
            executed_trades.push(self.build_executed_trade(trade, order_id, dry_run));
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
