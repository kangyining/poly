use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfigFile {
    #[serde(default)]
    pub defaults: Map<String, Value>,
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub strategies: BTreeMap<String, Map<String, Value>>,
}

pub fn load_config(path: &Path) -> Result<StrategyConfigFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read strategy config {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse strategy config {}", path.display()))
}

pub fn list_strategy_configs(path: &Path) -> Result<Vec<Value>> {
    let cfg = load_config(path)?;
    Ok(cfg
        .strategies
        .iter()
        .map(|(name, values)| {
            let mut obj = values.clone();
            obj.insert("name".to_string(), Value::String(name.clone()));
            Value::Object(obj)
        })
        .collect())
}

pub fn list_strategy_names(path: &Path) -> Result<Vec<String>> {
    let cfg = load_config(path)?;
    Ok(cfg.strategies.keys().cloned().collect())
}

pub fn group_members(path: &Path, group_name: &str) -> Result<Vec<String>> {
    let cfg = load_config(path)?;
    Ok(cfg.groups.get(group_name).cloned().unwrap_or_default())
}

pub fn all_members(path: &Path) -> Result<Vec<String>> {
    group_members(path, "all")
}

pub fn resolve_strategy_name(strategy_name: &str, params: &Value) -> String {
    params
        .get("strategy_id")
        .and_then(Value::as_str)
        .unwrap_or(match strategy_name {
            "base_ev" => "base_no_ev",
            other => other,
        })
        .to_string()
}

pub fn load_strategy_params(path: &Path, strategy_name: &str) -> Result<Value> {
    let cfg = load_config(path)?;
    let mut merged = cfg.defaults;
    let mut selected = if strategy_name == "base_ev" {
        cfg.strategies
            .get("base_no_ev")
            .cloned()
            .unwrap_or_default()
    } else {
        cfg.strategies
            .get(strategy_name)
            .cloned()
            .with_context(|| format!("unknown strategy: {strategy_name}"))?
    };
    let strategy_id = selected
        .remove("strategy_id")
        .and_then(|v| v.as_str().map(ToString::to_string))
        .unwrap_or_else(|| {
            if strategy_name == "base_ev" {
                "base_no_ev"
            } else {
                strategy_name
            }
            .to_string()
        });
    for (k, v) in selected {
        if k != "description" && k != "required_data" {
            merged.insert(k, v);
        }
    }
    merged.insert("strategy_id".to_string(), Value::String(strategy_id));
    Ok(Value::Object(merged))
}
