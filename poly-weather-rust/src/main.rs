use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use poly_weather_rust::commands;

#[derive(Debug, Parser)]
#[command(author, version, about = "Rust migration of poly-weather", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run strategy backtests against SQLite history.
    Backtest(commands::backtest::BacktestArgs),
    /// Run initial-deposit bankroll replay.
    BankrollBacktest(commands::bankroll::BankrollArgs),
    /// Grid-search backtest parameters.
    ParamSweep(commands::sweep::SweepArgs),
    /// Fit forecast calibration table.
    Calibrate(commands::calibrate::CalibrateArgs),
    /// Recorder/backfill command surface.
    Recorder(commands::recorder::RecorderArgs),
    /// Execute an execution_plan.json.
    Execute(commands::execute::ExecuteArgs),
    /// Settle open positions.
    Settle(commands::settle::SettleArgs),
    /// Show position status/PnL.
    Status(commands::status::StatusArgs),
    /// Print where bundled/default assets live.
    Doctor {
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "poly_weather_rust=info,warn".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Backtest(args) => commands::backtest::run(args),
        Command::BankrollBacktest(args) => commands::bankroll::run(args),
        Command::ParamSweep(args) => commands::sweep::run(args),
        Command::Calibrate(args) => commands::calibrate::run(args),
        Command::Recorder(args) => commands::recorder::run(args),
        Command::Execute(args) => commands::execute::run(args),
        Command::Settle(args) => commands::settle::run(args),
        Command::Status(args) => commands::status::run(args),
        Command::Doctor { db } => {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "crate": env!("CARGO_PKG_NAME"),
                    "version": env!("CARGO_PKG_VERSION"),
                    "default_db": db.unwrap_or_else(|| PathBuf::from(poly_weather_rust::DEFAULT_DB_PATH)),
                    "strategies": poly_weather_rust::DEFAULT_STRATEGIES_PATH,
                    "calibration": poly_weather_rust::DEFAULT_CALIBRATION_PATH,
                })
            );
            Ok(())
        }
    }
}
