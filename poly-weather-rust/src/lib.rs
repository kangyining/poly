pub mod analysis;
pub mod commands;
pub mod config;
pub mod data;
pub mod db;
pub mod execution;
pub mod strategy;

pub const DEFAULT_DB_PATH: &str = "db/data.db";
pub const DEFAULT_STRATEGIES_PATH: &str = "analysis/strategies/strategies.json";
pub const DEFAULT_CALIBRATION_PATH: &str = "calibration.json";
