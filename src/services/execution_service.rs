use std::env;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy::signers::Signer as _;
use alloy::signers::local::LocalSigner;
use anyhow::{Context, Result};
use chrono::Utc;
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::POLYGON;
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk::types::{Address as PolyAddress, Decimal};
use rusqlite::{Connection, params};
use serde::Serialize;
use tracing::info;

use crate::config::AppConfig;
use crate::services::execution_plan::{ExecutionPlan, PlannedTrade};

use super::StrategyOutcome;

pub struct ExecutionService {
    config: AppConfig,
}

impl ExecutionService {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub async fn execute(&self, outcome: &StrategyOutcome) -> Result<()> {
        outcome.plan.write_to_dir(&self.config.execution_plan_dir)?;
        self.run_native_bridge(&outcome.plan).await?;
        Ok(())
    }

    async fn run_native_bridge(&self, plan: &ExecutionPlan) -> Result<()> {
        info!("executing plan with native Rust bridge");

        if plan.trades.is_empty() {
            self.export_open_positions()?;
            info!("no trades in plan, exiting");
            return Ok(());
        }

        let dry_run = plan.dry_run;
        let live_client = if dry_run {
            None
        } else {
            Some(self.authenticate_live_client().await?)
        };

        for trade in &plan.trades {
            info!(
                mode = if dry_run { "paper" } else { "live" },
                bin = %trade.bin_label,
                p_model = trade.p_model,
                ev_no = trade.ev_no,
                effective_ask = trade.effective_ask,
                shares = trade.shares,
                stake_usd = trade.stake_usd,
                "processing planned trade"
            );

            let order_id = match &live_client {
                Some(client) => Some(self.submit_live_trade(client, trade).await?),
                None => None,
            };
            self.record_position(plan, trade, order_id.as_deref(), dry_run)?;
        }

        self.export_open_positions()?;
        Ok(())
    }

    async fn authenticate_live_client(
        &self,
    ) -> Result<ClobClient<polymarket_client_sdk::auth::state::Authenticated<Normal>>> {
        let private_key = env::var("POLYMARKET_PRIVATE_KEY")
            .context("POLYMARKET_PRIVATE_KEY must be set for live trading")?;
        let signer = LocalSigner::from_str(&private_key)
            .context("failed to parse POLYMARKET_PRIVATE_KEY")?
            .with_chain_id(Some(POLYGON));

        let config = ClobConfig::builder().use_server_time(true).build();
        let base = ClobClient::new(&self.config.clob_url, config)
            .context("failed to build Polymarket CLOB client")?;

        let mut auth = base.authentication_builder(&signer);
        if let Ok(funder_raw) = env::var("POLYMARKET_FUNDER") {
            let funder = PolyAddress::from_str(&funder_raw)
                .context("failed to parse POLYMARKET_FUNDER")?;
            if funder == signer.address() {
                auth = auth.signature_type(SignatureType::Eoa);
            } else {
                auth = auth.funder(funder).signature_type(SignatureType::Proxy);
            }
        } else {
            auth = auth.signature_type(SignatureType::Eoa);
        }

        let client = auth
            .authenticate()
            .await
            .context("failed to authenticate Polymarket CLOB client")?;
        info!(address = %signer.address(), "Polymarket CLOB client authenticated");
        Ok(client)
    }

    async fn submit_live_trade(
        &self,
        client: &ClobClient<polymarket_client_sdk::auth::state::Authenticated<Normal>>,
        trade: &PlannedTrade,
    ) -> Result<String> {
        let private_key = env::var("POLYMARKET_PRIVATE_KEY")
            .context("POLYMARKET_PRIVATE_KEY must be set for live trading")?;
        let signer = LocalSigner::from_str(&private_key)
            .context("failed to parse POLYMARKET_PRIVATE_KEY")?
            .with_chain_id(Some(POLYGON));

        let order = client
            .limit_order()
            .token_id(&trade.no_token_id)
            .order_type(OrderType::GTC)
            .price(decimal_from_f64(trade.effective_ask)?)
            .size(decimal_from_f64(trade.shares)?)
            .side(Side::Buy)
            .build()
            .await
            .with_context(|| format!("failed to build live order for {}", trade.bin_label))?;

        let signed = client
            .sign(&signer, order)
            .await
            .with_context(|| format!("failed to sign live order for {}", trade.bin_label))?;

        let response = client
            .post_order(signed)
            .await
            .with_context(|| format!("failed to submit live order for {}", trade.bin_label))?;

        if !response.success {
            let message = response
                .error_msg
                .unwrap_or_else(|| "unknown exchange error".to_string());
            anyhow::bail!("live order rejected for {}: {}", trade.bin_label, message);
        }

        info!(
            bin = %trade.bin_label,
            order_id = %response.order_id,
            "live order placed"
        );
        Ok(response.order_id)
    }

    fn record_position(
        &self,
        plan: &ExecutionPlan,
        trade: &PlannedTrade,
        order_id: Option<&str>,
        is_paper: bool,
    ) -> Result<()> {
        let db_path = positions_db_path(&self.config.open_positions_path);
        let conn = open_positions_db(&db_path)?;
        ensure_positions_schema(&conn)?;

        let trade_rationale = serde_json::to_string(&TradeRationale {
            ev_no: trade.ev_no,
            p_model: trade.p_model,
            fee_rate: trade.fee_rate,
            effective_ask: trade.effective_ask,
        })?;

        conn.execute(
            "INSERT OR IGNORE INTO positions (
                token_id, market_id, bin_label, event_slug, target_date,
                entry_price, shares, stake_usd, order_id,
                status, pnl, opened_at, settled_at, is_paper,
                strategy_name, backtest_id, trade_rationale
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'open', NULL, ?10, NULL, ?11, ?12, NULL, ?13)",
            params![
                trade.no_token_id,
                trade.market_id,
                trade.bin_label,
                plan.event_slug,
                plan.target_date,
                trade.effective_ask,
                trade.shares,
                trade.stake_usd,
                order_id,
                Utc::now().to_rfc3339(),
                if is_paper { 1 } else { 0 },
                trade.strategy_name,
                trade_rationale,
            ],
        )?;

        info!(
            bin = %trade.bin_label,
            mode = if is_paper { "paper" } else { "live" },
            order_id = order_id.unwrap_or("paper"),
            "position recorded"
        );
        Ok(())
    }

    fn export_open_positions(&self) -> Result<()> {
        let db_path = positions_db_path(&self.config.open_positions_path);
        let conn = open_positions_db(&db_path)?;
        ensure_positions_schema(&conn)?;

        let mut stmt = conn.prepare(
            "SELECT token_id, market_id, bin_label, event_slug, target_date, entry_price,
                    shares, stake_usd, order_id, status, pnl, opened_at, settled_at, is_paper
             FROM positions
             WHERE status = 'open'
             ORDER BY opened_at",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(OpenPositionExport {
                token_id: row.get(0)?,
                market_id: row.get(1)?,
                bin_label: row.get(2)?,
                event_slug: row.get(3)?,
                target_date: row.get(4)?,
                entry_price: row.get(5)?,
                shares: row.get(6)?,
                stake_usd: row.get(7)?,
                order_id: row.get(8)?,
                status: row.get(9)?,
                pnl: row.get(10)?,
                opened_at: row.get(11)?,
                settled_at: row.get(12)?,
                is_paper: row.get::<_, i64>(13)? != 0,
            })
        })?;

        let open_positions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(parent) = self.config.open_positions_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&open_positions)?;
        std::fs::write(&self.config.open_positions_path, json)?;
        info!(
            path = %self.config.open_positions_path.display(),
            count = open_positions.len(),
            "open positions exported"
        );
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct TradeRationale {
    ev_no: f64,
    p_model: f64,
    fee_rate: f64,
    effective_ask: f64,
}

#[derive(Debug, Serialize)]
struct OpenPositionExport {
    token_id: String,
    market_id: String,
    bin_label: String,
    event_slug: String,
    target_date: String,
    entry_price: f64,
    shares: f64,
    stake_usd: f64,
    order_id: Option<String>,
    status: String,
    pnl: Option<f64>,
    opened_at: String,
    settled_at: Option<String>,
    is_paper: bool,
}

fn positions_db_path(open_positions_path: &Path) -> PathBuf {
    open_positions_path
        .parent()
        .map(|dir| dir.join("db").join("data.db"))
        .unwrap_or_else(|| PathBuf::from("db/data.db"))
}

fn open_positions_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(Connection::open(path)?)
}

fn ensure_positions_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS positions (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            token_id        TEXT NOT NULL,
            market_id       TEXT NOT NULL,
            bin_label       TEXT NOT NULL,
            event_slug      TEXT NOT NULL,
            target_date     TEXT NOT NULL,
            entry_price     REAL NOT NULL,
            shares          REAL NOT NULL,
            stake_usd       REAL NOT NULL,
            order_id        TEXT,
            status          TEXT NOT NULL DEFAULT 'open',
            pnl             REAL,
            opened_at       TEXT NOT NULL,
            settled_at      TEXT,
            is_paper        INTEGER NOT NULL DEFAULT 1,
            strategy_name   TEXT NOT NULL DEFAULT 'default',
            backtest_id     TEXT,
            trade_rationale TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_positions_token_order
            ON positions(token_id, order_id);
        CREATE INDEX IF NOT EXISTS idx_positions_status
            ON positions(status);
        CREATE INDEX IF NOT EXISTS idx_positions_target_date
            ON positions(target_date);",
    )?;
    Ok(())
}

fn decimal_from_f64(value: f64) -> Result<Decimal> {
    Decimal::from_str(&value.to_string())
        .with_context(|| format!("failed to convert {value} into Decimal"))
}
