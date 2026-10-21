use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepResult {
    pub strategy: String,
    pub params: serde_json::Value,
    pub metric: f64,
}
