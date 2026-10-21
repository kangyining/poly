use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::Args;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

use crate::db::{self, Database, NewPosition};

#[derive(Debug, Clone, Args)]
pub struct ExecuteArgs {
    #[arg(long, default_value = "execution_plan.json")]
    pub plan: PathBuf,
    #[arg(long, default_value = "open_positions.json")]
    pub positions: PathBuf,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct ExecutionPlan {
    event_slug: String,
    target_date: String,
    #[serde(default = "default_true")]
    dry_run: bool,
    #[serde(default)]
    trades: Vec<PlannedTrade>,
}

#[derive(Debug, Deserialize)]
struct PlannedTrade {
    no_token_id: String,
    market_id: String,
    bin_label: String,
    effective_ask: f64,
    #[serde(default)]
    fee_rate: Option<f64>,
    shares: f64,
    stake_usd: f64,
    #[serde(default)]
    strategy_name: Option<String>,
    #[serde(default)]
    ev_no: Option<f64>,
    #[serde(default)]
    p_model: Option<f64>,
}

fn default_true() -> bool {
    true
}

pub fn run(args: ExecuteArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.plan)
        .with_context(|| format!("execution plan not found at {}", args.plan.display()))?;
    let plan: ExecutionPlan = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", args.plan.display()))?;
    let dry_run = args.dry_run || plan.dry_run;
    if !dry_run {
        bail!("unsupported: live order submission is intentionally not ported");
    }
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, false)?;
    for trade in &plan.trades {
        let strategy_name = trade
            .strategy_name
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let rationale = json!({
            "ev_no": trade.ev_no,
            "p_model": trade.p_model,
            "fee_rate": trade.fee_rate,
            "effective_ask": trade.effective_ask,
        });
        let pos = NewPosition {
            token_id: trade.no_token_id.clone(),
            market_id: trade.market_id.clone(),
            bin_label: trade.bin_label.clone(),
            event_slug: plan.event_slug.clone(),
            target_date: plan.target_date.clone(),
            entry_price: trade.effective_ask,
            fee_rate: trade.fee_rate,
            shares: trade.shares,
            stake_usd: trade.stake_usd,
            order_id: None,
            status: "open".to_string(),
            pnl: None,
            opened_at: Utc::now().to_rfc3339(),
            settled_at: None,
            is_paper: true,
            strategy_name,
            backtest_id: None,
            trade_rationale: Some(rationale.to_string()),
        };
        db::insert_position(&db, &pos)?;
    }
    db::export_open_positions_json(&db, &args.positions)?;
    println!(
        "{}",
        json!({
            "event_slug": plan.event_slug,
            "target_date": plan.target_date,
            "trades_recorded": plan.trades.len(),
            "dry_run": true,
            "positions": args.positions,
        })
    );
    Ok(())
}
