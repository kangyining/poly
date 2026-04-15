use crate::strategy::TradeCandidate;

#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub total_stake_usd: f64,
    pub trades: Vec<PlannedTrade>,
}

#[derive(Debug, Clone)]
pub struct PlannedTrade {
    pub decision_id: String,
    pub market_id: String,
    pub question: String,
    pub side: &'static str,
    pub price: f64,
    pub size_usd: f64,
}

pub trait ExecutionClient {
    fn submit(&self, plan: &ExecutionPlan) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct DryRunExecutionClient;

impl ExecutionClient for DryRunExecutionClient {
    fn submit(&self, plan: &ExecutionPlan) -> anyhow::Result<()> {
        tracing::info!(
            trade_count = plan.trades.len(),
            total_stake_usd = plan.total_stake_usd,
            "dry-run execution client received execution plan"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PolymarketExecutionClient {
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub api_passphrase: Option<String>,
}

impl ExecutionClient for PolymarketExecutionClient {
    fn submit(&self, _plan: &ExecutionPlan) -> anyhow::Result<()> {
        anyhow::bail!(
            "live order submission is not implemented yet; configure Polymarket auth and wire the CLOB client"
        );
    }
}

impl ExecutionPlan {
    pub fn from_candidates(candidates: &[TradeCandidate]) -> Self {
        let trades: Vec<PlannedTrade> = candidates
            .iter()
            .map(|candidate| PlannedTrade {
                decision_id: candidate.decision_id.clone(),
                market_id: candidate.market_id.clone(),
                question: candidate.question.clone(),
                side: "NO",
                price: candidate.no_price,
                size_usd: candidate.stake_usd,
            })
            .collect();

        let total_stake_usd = trades.iter().map(|trade| trade.size_usd).sum();

        Self {
            total_stake_usd,
            trades,
        }
    }
}
