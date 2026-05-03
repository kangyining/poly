use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::{AppConfig, MarketContext};
use crate::market::MarketService;
use crate::strategy::{
    BucketProbabilityRule, CalibrationTable, MarketTimingRule, NoEdgeRule, StrategyEngine,
    StrategyEvaluation, StrategyInput, TokenAvailabilityRule,
};
use crate::weather::Forecast;

#[derive(Debug, Clone)]
pub struct StrategyOutcome {
    pub strategy_name: String,
    pub context: MarketContext,
    pub run_id: String,
    pub evaluation: StrategyEvaluation,
}

pub struct StrategyService {
    config: AppConfig,
    calibration: Option<CalibrationTable>,
    contexts: HashMap<String, MarketContext>,
    market_services: HashMap<String, Arc<MarketService>>,
}

impl StrategyService {
    pub fn new(
        config: AppConfig,
        calibration: Option<CalibrationTable>,
        contexts: Vec<MarketContext>,
        market_services: Vec<Arc<MarketService>>,
    ) -> Self {
        let contexts = contexts
            .into_iter()
            .map(|context| (context.key.clone(), context))
            .collect::<HashMap<_, _>>();
        let market_services = market_services
            .into_iter()
            .map(|service| (service.context_key().to_string(), service))
            .collect::<HashMap<_, _>>();
        Self {
            config,
            calibration,
            contexts,
            market_services,
        }
    }

    pub async fn evaluate(&self, forecast: Forecast) -> Result<Vec<StrategyOutcome>> {
        let context = self
            .contexts
            .get(&forecast.context_key)
            .cloned()
            .with_context(|| format!("unknown market context {}", forecast.context_key))?;
        let market_service = self
            .market_services
            .get(&forecast.context_key)
            .with_context(|| format!("missing market service for {}", forecast.context_key))?;

        let run_id = format!(
            "{}-{}",
            context.city_slug,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let (beta_f, sigma_f) = self
            .calibration
            .as_ref()
            .map(|t| {
                t.lookup(
                    &forecast.station_code,
                    forecast.target_date,
                    forecast.lead_hours,
                )
            })
            .unwrap_or((0.0, 2.0));

        let mu_f = forecast.forecast_high_f + beta_f;
        let sigma_f = sigma_f.clamp(0.8, 4.0);

        let open_token_ids = load_open_positions(&self.config.open_positions_path);
        let markets = market_service.fetch_event_markets().await?;

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

        let engine = StrategyEngine::new(vec![
            Box::<MarketTimingRule>::default(),
            Box::<TokenAvailabilityRule>::default(),
            Box::<BucketProbabilityRule>::default(),
            Box::<NoEdgeRule>::default(),
        ]);

        let mut outcomes = Vec::new();
        for strategy in self.config.active_strategies() {
            let strategy_input = StrategyInput {
                strategy_name: strategy.name.clone(),
                forecast: forecast.clone(),
                mu_f,
                sigma_f,
                min_ev_threshold: strategy.min_ev_threshold,
                desired_shares: strategy.desired_shares,
                context_key: context.key.clone(),
                city_slug: context.city_slug.clone(),
                target_date: context.target_date,
                run_id: format!("{run_id}-{}", strategy.name),
                event_slug: context.event_slug.clone(),
                evaluation_time_utc: Some(Utc::now()),
            };

            let mut evaluation = engine.evaluate(&strategy_input, &markets);
            log_evaluation(&context, strategy.name.as_str(), &evaluation);
            retain_by_total_stake(
                &mut evaluation,
                strategy
                    .max_stake_usd
                    .min(self.config.max_total_stake_usd * strategy.funding_pct),
            );

            outcomes.push(StrategyOutcome {
                strategy_name: strategy.name.clone(),
                context: context.clone(),
                run_id: strategy_input.run_id,
                evaluation,
            });
        }

        Ok(outcomes)
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
            Err(error) => {
                warn!(error = %error, "failed to parse open_positions.json, treating as empty");
                HashSet::new()
            }
        },
        Err(error) => {
            warn!(error = %error, "failed to read open_positions.json, treating as empty");
            HashSet::new()
        }
    }
}

fn log_evaluation(context: &MarketContext, strategy_name: &str, evaluation: &StrategyEvaluation) {
    for market in &evaluation.evaluated_markets {
        for trace in &market.traces {
            info!(
                context_key = %context.key,
                city = %context.city_slug,
                target_date = %context.target_date,
                strategy_name,
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
            context_key = %candidate.context_key,
            city = %candidate.city_slug,
            target_date = %candidate.target_date,
            strategy_name = %candidate.strategy_name,
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

fn retain_by_total_stake(evaluation: &mut StrategyEvaluation, max_stake_usd: f64) {
    let total_stake: f64 = evaluation.candidates.iter().map(|candidate| candidate.stake_usd).sum();
    if total_stake <= max_stake_usd {
        return;
    }

    let mut running = 0.0_f64;
    evaluation.candidates.retain(|candidate| {
        if running + candidate.stake_usd <= max_stake_usd {
            running += candidate.stake_usd;
            true
        } else {
            false
        }
    });
}
