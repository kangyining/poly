use tracing::{debug, info, info_span};

use crate::polymarket::WeatherMarket;

use super::rules::StrategyRule;
use super::types::{DecisionTrace, StrategyEvaluation, StrategyInput, TradeCandidate};

pub struct StrategyEngine {
    rules: Vec<Box<dyn StrategyRule>>,
}

impl StrategyEngine {
    pub fn new(rules: Vec<Box<dyn StrategyRule>>) -> Self {
        Self { rules }
    }

    pub fn evaluate(&self, input: &StrategyInput, markets: &[WeatherMarket]) -> StrategyEvaluation {
        info!(
            run_id = %input.run_id,
            event_slug = %input.event_slug,
            market_count = markets.len(),
            "starting strategy evaluation"
        );

        let mut candidates = Vec::new();
        let mut evaluated_markets = Vec::new();

        for (index, market) in markets.iter().enumerate() {
            let decision_id = format!("{}:{}:{}", input.run_id, market.id, index);
            let span = info_span!(
                "market_decision",
                run_id = %input.run_id,
                event_slug = %input.event_slug,
                decision_id = %decision_id,
                market_id = %market.id,
                question = %market.question
            );
            let _entered = span.enter();

            let mut trace = DecisionTrace::new(decision_id.clone(), market);
            let mut accepted = true;

            for rule in &self.rules {
                if !rule.evaluate(input, market, &mut trace) {
                    accepted = false;
                    debug!(rule = rule.name(), "market rejected after rule evaluation");
                    break;
                }
            }

            if accepted {
                let no_price = market
                    .no_price
                    .expect("accepted market must have a NO price");
                let distance_f = trace
                    .distance_f
                    .unwrap_or_else(|| market.range.distance_from(input.forecast.forecast_high_f));

                candidates.push(TradeCandidate {
                    market_id: market.id.clone(),
                    question: market.question.clone(),
                    slug: market.slug.clone(),
                    range: market.range,
                    distance_f,
                    no_price,
                    yes_price: market.yes_price,
                    no_token_id: market.no_token_id.clone(),
                    stake_usd: input.order_size_usd,
                    decision_id,
                });
            }

            evaluated_markets.push(trace.into_evaluated_market(accepted));
        }

        info!(
            run_id = %input.run_id,
            approved_count = candidates.len(),
            rejected_count = evaluated_markets.len().saturating_sub(candidates.len()),
            "finished strategy evaluation"
        );

        StrategyEvaluation {
            candidates,
            evaluated_markets,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use crate::config::PriceBand;
    use crate::polymarket::{TemperatureRange, WeatherMarket};
    use crate::weather::Forecast;

    use super::super::rules::{
        MarketTimingRule, NoPriceBandRule, TemperatureDistanceRule, TokenAvailabilityRule,
    };
    use super::super::types::StrategyInput;
    use super::StrategyEngine;

    #[test]
    fn keeps_only_far_and_expensive_no_bins() {
        let forecast = Forecast {
            target_date: NaiveDate::from_ymd_opt(2026, 4, 13).expect("valid date"),
            forecast_high_f: 50.0,
            fetched_at: chrono::Utc::now(),
            source: "test".to_string(),
        };
        let input = StrategyInput {
            forecast,
            price_band: PriceBand {
                min: 0.90,
                max: 0.99,
            },
            distance_threshold_f: 4.0,
            order_size_usd: 5.0,
            run_id: "test-run".into(),
            event_slug: "nyc-apr-13".into(),
            evaluation_time_utc: Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(2026, 4, 12)
                    .expect("valid date")
                    .and_hms_opt(12, 30, 0)
                    .expect("valid time"),
                chrono::Utc,
            )),
        };

        let markets = vec![
            WeatherMarket {
                id: "far-left".into(),
                question: "44F or below".into(),
                slug: None,
                end_date: None,
                range: TemperatureRange::BelowOrEqual { upper_f: 44.0 },
                yes_price: Some(0.08),
                no_price: Some(0.92),
                yes_token_id: None,
                no_token_id: Some("1".into()),
            },
            WeatherMarket {
                id: "middle".into(),
                question: "48F to 50F".into(),
                slug: None,
                end_date: None,
                range: TemperatureRange::Between {
                    low_f: 48.0,
                    high_f: 50.0,
                },
                yes_price: Some(0.60),
                no_price: Some(0.40),
                yes_token_id: None,
                no_token_id: Some("2".into()),
            },
            WeatherMarket {
                id: "far-right-cheap".into(),
                question: "56F or above".into(),
                slug: None,
                end_date: None,
                range: TemperatureRange::AboveOrEqual { lower_f: 56.0 },
                yes_price: Some(0.15),
                no_price: Some(0.85),
                yes_token_id: None,
                no_token_id: Some("3".into()),
            },
        ];

        let engine = StrategyEngine::new(vec![
            Box::<MarketTimingRule>::default(),
            Box::<TemperatureDistanceRule>::default(),
            Box::<NoPriceBandRule>::default(),
            Box::<TokenAvailabilityRule>::default(),
        ]);
        let evaluation = engine.evaluate(&input, &markets);

        assert_eq!(evaluation.candidates.len(), 1);
        assert_eq!(evaluation.candidates[0].market_id, "far-left");
        assert_eq!(evaluation.candidates[0].stake_usd, 5.0);
    }
}
