use tracing::{debug, info, instrument};

use crate::market::WeatherMarket;

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
            mu_f = input.mu_f,
            sigma_f = input.sigma_f,
            "starting strategy evaluation"
        );

        let mut candidates = Vec::new();
        let mut evaluated_markets = Vec::new();

        for (index, market) in markets.iter().enumerate() {
            let (candidate, evaluated_market) = self.evaluate_market(input, market, index);
            if let Some(candidate) = candidate {
                candidates.push(candidate);
            }
            evaluated_markets.push(evaluated_market);
        }

        info!(
            run_id = %input.run_id,
            approved = candidates.len(),
            rejected = evaluated_markets.len().saturating_sub(candidates.len()),
            "finished strategy evaluation"
        );

        StrategyEvaluation {
            candidates,
            evaluated_markets,
        }
    }

    #[instrument(
        name = "market_decision",
        skip(self, input, market),
        fields(
            run_id = %input.run_id,
            decision_id = %format!("{}:{}:{}", input.run_id, market.id, index),
            market_id = %market.id,
            question = %market.question
        )
    )]
    fn evaluate_market(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        index: usize,
    ) -> (Option<TradeCandidate>, super::types::EvaluatedMarket) {
        let decision_id = format!("{}:{}:{}", input.run_id, market.id, index);
        let mut trace = DecisionTrace::new(decision_id.clone(), market);
        let mut accepted = true;

        for rule in &self.rules {
            if !rule.evaluate(input, market, &mut trace) {
                accepted = false;
                debug!(rule = rule.name(), "market rejected");
                break;
            }
        }

        let candidate = if accepted {
            let p = market.range.bucket_probability(input.mu_f, input.sigma_f);
            let effective_ask = market
                .effective_ask_no
                .expect("accepted market has effective_ask");
            let fee_rate = market.fee_rate.expect("accepted market has fee_rate");
            let ev_no = (1.0 - p) - effective_ask * (1.0 + fee_rate * (1.0 - effective_ask));
            let no_token_id = market
                .no_token_id
                .clone()
                .expect("accepted market has no_token_id");

            Some(TradeCandidate {
                strategy_name: input.strategy_name.clone(),
                context_key: input.context_key.clone(),
                city_slug: input.city_slug.clone(),
                target_date: input.target_date,
                event_slug: input.event_slug.clone(),
                market_id: market.id.clone(),
                question: market.question.clone(),
                slug: market.slug.clone(),
                range: market.range,
                p_model: p,
                ev_no,
                effective_ask,
                fee_rate,
                no_token_id,
                shares: input.desired_shares,
                stake_usd: input.desired_shares
                    * effective_ask
                    * (1.0 + fee_rate * (1.0 - effective_ask)),
                decision_id,
            })
        } else {
            None
        };

        (candidate, trace.into_evaluated_market(accepted))
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use crate::market::{TemperatureRange, WeatherMarket};
    use crate::weather::Forecast;

    use super::super::rules::{
        BucketProbabilityRule, MarketTimingRule, NoEdgeRule, TokenAvailabilityRule,
    };
    use super::super::types::StrategyInput;
    use super::StrategyEngine;

    fn make_market(
        id: &str,
        question: &str,
        range: TemperatureRange,
        effective_ask: f64,
        fee_rate: f64,
    ) -> WeatherMarket {
        WeatherMarket {
            id: id.into(),
            question: question.into(),
            slug: None,
            end_date: None,
            range,
            yes_price: None,
            no_price: None,
            yes_token_id: None,
            no_token_id: Some(format!("token-{id}")),
            effective_ask_no: Some(effective_ask),
            fee_rate: Some(fee_rate),
        }
    }

    #[test]
    fn keeps_only_high_ev_no_bins() {
        // mu=50°F, sigma=2 → bins far from 50 have low P_model → high EV_NO
        let forecast = Forecast {
            context_key: "nyc:2026-04-20".into(),
            city_slug: "nyc".into(),
            target_date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            forecast_high_f: 50.0,
            lead_hours: 26.0,
            station_code: "KLGA".into(),
        };

        let input = StrategyInput {
            strategy_name: "test".into(),
            forecast,
            mu_f: 50.0,
            sigma_f: 2.0,
            min_ev_threshold: 0.02,
            desired_shares: 10.0,
            context_key: "nyc:2026-04-20".into(),
            city_slug: "nyc".into(),
            target_date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            run_id: "test".into(),
            event_slug: "test-slug".into(),
            // Set evaluation time 20h before close so timing rule passes
            evaluation_time_utc: Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(2026, 4, 19)
                    .unwrap()
                    .and_hms_opt(16, 0, 0)
                    .unwrap(),
                chrono::Utc,
            )),
        };

        let markets = vec![
            // Far below: P_model ≈ 0 → EV_NO ≈ 1 - ask → high EV
            make_market(
                "far-below",
                "38F or below",
                TemperatureRange::BelowOrEqual { upper_f: 38.0 },
                0.97, // effective_ask
                0.05, // fee_rate
            ),
            // At forecast: P_model ≈ 0.5 → EV_NO ≈ 0.5 - ask → negative
            make_market(
                "at-forecast",
                "49F to 51F",
                TemperatureRange::Between {
                    low_f: 49.0,
                    high_f: 51.0,
                },
                0.50,
                0.05,
            ),
            // Far above: P_model ≈ 0 → EV_NO high
            make_market(
                "far-above",
                "64F or above",
                TemperatureRange::AboveOrEqual { lower_f: 64.0 },
                0.97,
                0.05,
            ),
        ];

        let engine = StrategyEngine::new(vec![
            Box::<MarketTimingRule>::default(),
            Box::<TokenAvailabilityRule>::default(),
            Box::<BucketProbabilityRule>::default(),
            Box::<NoEdgeRule>::default(),
        ]);

        let result = engine.evaluate(&input, &markets);
        let ids: Vec<&str> = result
            .candidates
            .iter()
            .map(|c| c.market_id.as_str())
            .collect();
        assert!(ids.contains(&"far-below"), "far-below should qualify");
        assert!(ids.contains(&"far-above"), "far-above should qualify");
        assert!(
            !ids.contains(&"at-forecast"),
            "at-forecast should be rejected"
        );
    }
}
