use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use crate::db::{load_positions, Database};

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub open: bool,
    #[arg(long)]
    pub closed: bool,
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long)]
    pub db: Option<PathBuf>,
}

pub fn run(args: StatusArgs) -> Result<()> {
    let status = match (args.open, args.closed) {
        (true, false) => Some("open"),
        (false, true) => Some("closed"),
        _ => None,
    };
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, true)?;
    let rows = load_positions(&db, status, args.date.as_deref())?;
    if rows.is_empty() {
        println!("No positions found.");
        return Ok(());
    }
    println!();
    println!(
        "  {:<18} {:<12} {:>6} {:>6} {:>7} {:>9} {:<8} Type",
        "Bin", "Date", "Ask", "Shares", "Stake", "PnL", "Status"
    );
    println!("  {}", "-".repeat(80));
    let mut total_stake = 0.0;
    let mut total_pnl = 0.0;
    for row in &rows {
        total_stake += row.stake_usd;
        total_pnl += row.pnl.unwrap_or(0.0);
        println!(
            "  {:<18} {:<12} {:>6.4} {:>6.1} ${:>6.2} {:>9} {:<8} {}",
            row.bin_label,
            row.target_date,
            row.entry_price,
            row.shares,
            row.stake_usd,
            row.pnl
                .map(|v| format!("${v:+.4}"))
                .unwrap_or_else(|| "     -  ".to_string()),
            row.status,
            if row.is_paper { "paper" } else { "live" }
        );
    }
    let roi = if total_stake > 0.0 {
        total_pnl / total_stake * 100.0
    } else {
        0.0
    };
    println!("  {}", "-".repeat(80));
    println!(
        "  {:<18} {:12} {:>6} {:>6} ${:>6.2} ${:>+8.4}   ROI: {:+.1}%",
        "TOTAL", "", "", "", total_stake, total_pnl, roi
    );
    println!();
    Ok(())
}
