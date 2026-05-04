use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use clap::Args;
use serde_json::json;
use std::path::PathBuf;

use crate::data::settlement_tracker;
use crate::db::{self, Database};

#[derive(Debug, Clone, Args)]
pub struct SettleArgs {
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long)]
    pub backfill: Option<i64>,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = "open_positions.json")]
    pub positions: PathBuf,
}

pub fn run(args: SettleArgs) -> Result<()> {
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, false)?;
    let mut saved = 0;
    let mut closed = 0;
    let dates = if let Some(days) = args.backfill {
        let today = Utc::now().date_naive();
        (1..=days.max(0))
            .rev()
            .map(|offset| today - Duration::days(offset))
            .collect::<Vec<_>>()
    } else {
        vec![args
            .date
            .as_deref()
            .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
            .transpose()?
            .unwrap_or_else(|| Utc::now().date_naive() - Duration::days(1))]
    };

    for target in dates {
        let summary = settlement_tracker::check_dates(&db, &[target], None, false)?;
        saved += summary.settlements_saved;
        closed += summary.positions_closed;
    }
    db::export_open_positions_json(&db, &args.positions)?;
    println!(
        "{}",
        json!({"settlements_saved": saved, "positions_closed": closed, "positions": args.positions})
    );
    Ok(())
}

pub fn settle_positions_for_test(
    db: &Database,
    target_date: &str,
    winning_bin_label: &str,
    resolved_at: Option<&str>,
    event_slug: Option<&str>,
) -> Result<usize> {
    settlement_tracker::settle_positions(
        db,
        target_date,
        winning_bin_label,
        resolved_at,
        event_slug,
    )
}
