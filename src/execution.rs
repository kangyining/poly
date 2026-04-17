use std::str::FromStr as _;

use alloy::signers::Signer as _;
use alloy::signers::local::LocalSigner;
use anyhow::{Context, Result};
use async_trait::async_trait;
use polymarket_client_sdk::clob::types::{Amount, OrderType, Side};
use polymarket_client_sdk::clob::{Client, Config as ClobConfig};
use polymarket_client_sdk::types::Decimal;
use polymarket_client_sdk::{POLYGON, PRIVATE_KEY_VAR};

use crate::strategy::TradeCandidate;

const CLOB_URL: &str = "https://clob.polymarket.com";

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
    pub token_id: String,
    pub price: f64,
    pub size_usd: f64,
}

#[async_trait]
pub trait ExecutionClient {
    async fn submit(&self, plan: &ExecutionPlan) -> Result<ExecutionReport>;
}

#[derive(Debug, Clone, Default)]
pub struct ExecutionReport {
    pub attempted: usize,
    pub submitted: usize,
    pub skipped: usize,
}

#[derive(Debug, Default)]
pub struct DryRunExecutionClient;

#[async_trait]
impl ExecutionClient for DryRunExecutionClient {
    async fn submit(&self, plan: &ExecutionPlan) -> Result<ExecutionReport> {
        tracing::info!(
            trade_count = plan.trades.len(),
            total_stake_usd = plan.total_stake_usd,
            "dry-run execution client received execution plan"
        );
        Ok(ExecutionReport {
            attempted: plan.trades.len(),
            submitted: 0,
            skipped: 0,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PolymarketExecutionClient {
    pub private_key: Option<String>,
}

#[async_trait]
impl ExecutionClient for PolymarketExecutionClient {
    async fn submit(&self, plan: &ExecutionPlan) -> Result<ExecutionReport> {
        for trade in &plan.trades {
            tracing::info!(
                decision_id = %trade.decision_id,
                market_id = %trade.market_id,
                token_id = %trade.token_id,
                side = trade.side,
                price = trade.price,
                size_usd = trade.size_usd,
                "validated live order intent"
            );
        }

        let private_key = self.private_key.as_ref().with_context(|| {
            format!(
                "live order submission requires POLYMARKET_PRIVATE_KEY or {}",
                PRIVATE_KEY_VAR
            )
        })?;
        let signer = LocalSigner::from_str(private_key)
            .context("failed to parse POLYMARKET_PRIVATE_KEY")?
            .with_chain_id(Some(POLYGON));

        let client = Client::new(CLOB_URL, ClobConfig::default())?
            .authentication_builder(&signer)
            .authenticate()
            .await
            .context("failed to authenticate with Polymarket CLOB")?;

        let mut submitted = 0;
        for trade in &plan.trades {
            let amount = Decimal::from_str(&format!("{:.2}", trade.size_usd))
                .context("failed to convert trade size into Decimal")?;
            let order = client
                .market_order()
                .token_id(&trade.token_id)
                .amount(Amount::usdc(amount)?)
                .side(Side::Buy)
                .order_type(OrderType::FOK)
                .build()
                .await
                .with_context(|| {
                    format!("failed to build Polymarket market order for {}", trade.token_id)
                })?;
            let signed_order = client
                .sign(&signer, order)
                .await
                .with_context(|| format!("failed to sign order for {}", trade.token_id))?;
            let response = client
                .post_order(signed_order)
                .await
                .with_context(|| format!("failed to post order for {}", trade.token_id))?;

            tracing::info!(
                decision_id = %trade.decision_id,
                market_id = %trade.market_id,
                token_id = %trade.token_id,
                response = ?response,
                "submitted Polymarket market order"
            );
            submitted += 1;
        }

        Ok(ExecutionReport {
            attempted: plan.trades.len(),
            submitted,
            skipped: 0,
        })
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
                token_id: candidate
                    .no_token_id
                    .clone()
                    .expect("accepted candidate must have a NO token id"),
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
