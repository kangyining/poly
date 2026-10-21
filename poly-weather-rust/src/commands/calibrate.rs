use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;

use crate::analysis::calibration;
use crate::db::Database;

#[derive(Debug, Clone, Args)]
pub struct CalibrateArgs {
    #[arg(long)]
    pub show: bool,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = crate::DEFAULT_CALIBRATION_PATH)]
    pub out: PathBuf,
}

pub fn run(args: CalibrateArgs) -> Result<()> {
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, true)?;
    let table = calibration::build_table(&db)?;
    if args.show {
        calibration::print_table(&table);
        return Ok(());
    }
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&table)?)
        .with_context(|| format!("failed to write {}", args.out.display()))?;
    println!("{}", serde_json::to_string_pretty(&table)?);
    Ok(())
}
