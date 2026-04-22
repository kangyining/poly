use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    pub name: String,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub funding_pct: f64,
    pub min_ev_threshold: f64,
    pub desired_shares: f64,
    pub max_stake_usd: f64,
}

fn bool_true() -> bool { true }
fn one() -> f64 { 1.0 }

#[derive(Deserialize)]
struct StrategiesFile {
    strategy: Vec<StrategyConfig>,
}

pub struct StrategyRegistry {
    pub strategies: Vec<StrategyConfig>,
}

impl StrategyRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let file: StrategiesFile = toml::from_str(&text)?;
        Ok(Self { strategies: file.strategy })
    }

    pub fn active(&self) -> impl Iterator<Item = &StrategyConfig> {
        self.strategies.iter().filter(|s| s.enabled)
    }
}
