use std::process::Command;

use anyhow::Result;
use tracing::{info, warn};

use crate::config::AppConfig;

use super::StrategyOutcome;

pub struct ExecutionService {
    config: AppConfig,
}

impl ExecutionService {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub fn execute(&self, outcome: &StrategyOutcome) -> Result<()> {
        outcome.plan.write_to_dir(&self.config.execution_plan_dir)?;
        self.run_execute_bridge();
        Ok(())
    }

    fn run_execute_bridge(&self) {
        let script = &self.config.execute_script;
        if script.as_os_str().is_empty() {
            return;
        }
        if !script.exists() {
            warn!(path = %script.display(), "execute.py not found, skipping bridge");
            return;
        }

        let plan_path = self.config.execution_plan_dir.join("execution_plan.json");
        let positions_path = &self.config.open_positions_path;

        info!(script = %script.display(), "invoking execute.py");

        #[cfg(target_os = "windows")]
        let python_bin = "python";
        #[cfg(not(target_os = "windows"))]
        let python_bin = "python3";

        let status = Command::new(python_bin)
            .arg(script)
            .arg("--plan")
            .arg(&plan_path)
            .arg("--positions")
            .arg(positions_path)
            .status();

        match status {
            Ok(s) if s.success() => info!("execute.py finished successfully"),
            Ok(s) => warn!(exit_code = ?s.code(), "execute.py exited with non-zero status"),
            Err(e) => warn!(error = %e, "failed to launch execute.py"),
        }
    }
}
