use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::market::MarketService;
use crate::services::ExecutionPlan;
use crate::strategy::{
    BucketProbabilityRule, CalibrationTable, MarketTimingRule, NoEdgeRule, StrategyEngine,
    StrategyEvaluation, StrategyInput, TokenAvailabilityRule,
};
use crate::weather::Forecast;

use super::EventService;

#[derive(Debug, Clone)]
pub struct StrategyOutcome {
    pub run_id: String,
    pub event_slug: String,
    pub target_date: NaiveDate,
    pub evaluation: StrategyEvaluation,
    pub plan: ExecutionPlan,
}

pub struct StrategyService {
    config: AppConfig,
    calibration: Option<CalibrationTable>,
    market_service: Arc<MarketService>,
}

impl StrategyService {
    pub fn new(
        config: AppConfig,
        calibration: Option<CalibrationTable>,
        market_service: Arc<MarketService>,
    ) -> Self {
        Self {
            config,
            calibration,
            market_service,
        }
    }

    pub async fn evaluate(&self, forecast: Forecast) -> Result<StrategyOutcome> {
        let target_date = forecast.target_date;
        let event_slug = EventService::build_event_slug(&self.config.city, target_date);
        let run_id = format!("{}-{}", self.config.city, Utc::now().format("%Y%m%dT%H%M%SZ"));

        let (beta_f, sigma_f) = self
            .calibration
            .as_ref()
            .map(|t| t.lookup(&self.config.station_code, target_date, forecast.lead_hours))
            .unwrap_or((0.0, 2.0));

        let mu_f = forecast.forecast_high_f + beta_f;
        let sigma_f = sigma_f.clamp(0.8, 4.0);

        let open_token_ids = load_open_positions(&self.config.open_positions_path);
        let markets = self
            .market_service
            .fetch_event_markets(&event_slug, target_date, self.config.desired_shares)
            .await?;

        let markets: Vec<_> = markets
            .into_iter()
            .filter(|market| {
                market
                    .no_token_id
                    .as_ref()
                    .map(|id| !open_token_ids.contains(id))
                    .unwrap_or(true)
            })
            .collect();

        let strategy_input = StrategyInput {
            forecast: forecast.clone(),
            mu_f,
            sigma_f,
            min_ev_threshold: self.config.min_ev_threshold,
            desired_shares: self.config.desired_shares,
            order_size_usd: self.config.order_size_usd,
            run_id: run_id.clone(),
            event_slug: event_slug.clone(),
            evaluation_time_utc: Some(Utc::now()),
        };

        let engine = StrategyEngine::new(vec![
            Box::<MarketTimingRule>::default(),
            Box::<TokenAvailabilityRule>::default(),
            Box::<BucketProbabilityRule>::default(),
            Box::<NoEdgeRule>::default(),
        ]);

        let evaluation = engine.evaluate(&strategy_input, &markets);
        log_evaluation(&evaluation);

        let mut candidates = evaluation.candidates.clone();
        let total_stake: f64 = candidates.iter().map(|c| c.stake_usd).sum();
        if total_stake > self.config.max_total_stake_usd {
            let mut running = 0.0_f64;
            candidates.retain(|candidate| {
                if running + candidate.stake_usd <= self.config.max_total_stake_usd {
                    running += candidate.stake_usd;
                    true
                } else {
                    false
                }
            });
        }

        let plan = ExecutionPlan::from_candidates(
            &candidates,
            &run_id,
            &event_slug,
            &target_date.to_string(),
            self.config.dry_run,
            "default",
        );

        Ok(StrategyOutcome {
            run_id,
            event_slug,
            target_date,
            evaluation,
            plan,
        })
    }
}

#[derive(Deserialize)]
struct OpenPosition {
    token_id: String,
    #[allow(dead_code)]
    status: String,
}

fn load_open_positions(path: &Path) -> HashSet<String> {
    if !path.exists() {
        return HashSet::new();
    }

    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Vec<OpenPosition>>(&text) {
            Ok(positions) => positions
                .into_iter()
                .filter(|position| position.status == "open")
                .map(|position| position.token_id)
                .collect(),
            Err(e) => {
                warn!(error = %e, "failed to parse open_positions.json, treating as empty");
                HashSet::new()
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to read open_positions.json, treating as empty");
            HashSet::new()
        }
    }
}

fn log_evaluation(evaluation: &StrategyEvaluation) {
    for market in &evaluation.evaluated_markets {
        for trace in &market.traces {
            info!(
                decision_id = %market.decision_id,
                market_id = %market.market_id,
                question = %market.question,
                accepted = market.accepted,
                rule = trace.rule_name,
                passed = trace.passed,
                reason = %trace.reason,
                "rule trace"
            );
        }
    }

    for candidate in &evaluation.candidates {
        info!(
            decision_id = %candidate.decision_id,
            market_id = %candidate.market_id,
            bin = %candidate.range.label(),
            p_model = candidate.p_model,
            ev_no = candidate.ev_no,
            effective_ask = candidate.effective_ask,
            shares = candidate.shares,
            stake_usd = candidate.stake_usd,
            "qualified trade candidate"
        );
    }
}
