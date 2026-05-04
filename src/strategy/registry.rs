use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::strategy::plugins;

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
    let mut names = plugins::plugin_names()
        .into_iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    names.extend(cfg.strategies.keys().cloned());
    Ok(names
        .into_iter()
        .map(|name| {
            let mut obj = cfg.strategies.get(&name).cloned().unwrap_or_default();
            obj.entry("name".to_string())
                .or_insert_with(|| Value::String(name.clone()));
            obj.entry("enabled".to_string())
                .or_insert(Value::Bool(true));
            if !obj.contains_key("required_data")
                && plugins::plugin_names().iter().any(|plugin| *plugin == name)
            {
                let strategy = plugins::strategy_by_name(&name);
                obj.insert(
                    "required_data".to_string(),
                    Value::Array(
                        strategy
                            .required_data()
                            .iter()
                            .map(|item| Value::String((*item).to_string()))
                            .collect(),
                    ),
                );
            }
            Value::Object(obj)
        })
        .collect())
}

pub fn list_strategy_names(path: &Path) -> Result<Vec<String>> {
    let cfg = load_config(path)?;
    let mut names = plugins::plugin_names()
        .into_iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    names.extend(cfg.strategies.keys().cloned());
    Ok(names.into_iter().collect())
}

pub fn group_members(path: &Path, group_name: &str) -> Result<Vec<String>> {
    let cfg = load_config(path)?;
    let Some(members) = cfg.groups.get(group_name) else {
        bail!("Unknown strategy group '{group_name}'");
    };
    Ok(members
        .iter()
        .map(|name| resolve_strategy_name(name, &Value::Null))
        .collect())
}

pub fn all_members(path: &Path) -> Result<Vec<String>> {
    group_members(path, "all")
}

pub fn resolve_strategy_name(strategy_name: &str, params: &Value) -> String {
    if let Some(explicit) = params
        .get("strategy_id")
        .and_then(Value::as_str)
        .or_else(|| params.get("plugin").and_then(Value::as_str))
    {
        return explicit.to_string();
    }
    if strategy_name.starts_with("sweep_base_ev") {
        return "base_no_ev".to_string();
    }
    match strategy_name {
        "base_ev" | "default" => "base_no_ev".to_string(),
        other => other.to_string(),
    }
}

pub fn load_strategy_params(path: &Path, strategy_name: &str) -> Result<Value> {
    let cfg = load_config(path)?;
    let resolved = resolve_strategy_name(strategy_name, &Value::Null);
    let mut merged = cfg.defaults;
    let mut selected = cfg
        .strategies
        .get(&resolved)
        .or_else(|| cfg.strategies.get(strategy_name))
        .cloned()
        .with_context(|| format!("unknown strategy: {strategy_name}"))?;
    let strategy_id = selected
        .remove("strategy_id")
        .and_then(|v| v.as_str().map(ToString::to_string))
        .unwrap_or_else(|| resolved.clone());
    for (k, v) in selected {
        if k != "description" && k != "required_data" {
            merged.insert(k, v);
        }
    }
    merged.insert("strategy_id".to_string(), Value::String(strategy_id));
    Ok(Value::Object(merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn config_file() -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            r#"{{
                "defaults": {{"min_ev_threshold": 0.05}},
                "groups": {{"core": ["default", "sweep_base_ev_1"]}},
                "strategies": {{"base_no_ev": {{"description": "base"}}}}
            }}"#
        )
        .unwrap();
        file
    }

    #[test]
    fn resolves_python_aliases() {
        assert_eq!(resolve_strategy_name("default", &Value::Null), "base_no_ev");
        assert_eq!(
            resolve_strategy_name("sweep_base_ev_abc", &Value::Null),
            "base_no_ev"
        );
    }

    #[test]
    fn unknown_group_errors_like_python() {
        let file = config_file();
        assert!(group_members(file.path(), "missing").is_err());
        assert_eq!(
            group_members(file.path(), "core").unwrap(),
            vec!["base_no_ev", "base_no_ev"]
        );
    }
}
