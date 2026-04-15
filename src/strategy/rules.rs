use chrono::Utc;
use tracing::{debug, warn};

use crate::polymarket::WeatherMarket;

use super::types::{
    market_close_utc, within_24h_of_close, DecisionTrace, RuleTrace, StrategyInput,
};

pub trait StrategyRule: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool;
}

#[derive(Debug, Default)]
pub struct MarketTimingRule;

impl StrategyRule for MarketTimingRule {
    fn name(&self) -> &'static str {
        "MarketTimingRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let now = input.evaluation_time_utc.unwrap_or_else(Utc::now);
        let within_window = within_24h_of_close(now, input.forecast.target_date);
        let reason = format!(
            "now={} target_date={} market_close_utc={} within_24h={}",
            now.to_rfc3339(),
            input.forecast.target_date,
            market_close_utc(input.forecast.target_date).to_rfc3339(),
            within_window
        );

        debug!(
            run_id = %input.run_id,
            decision_id = %trace.decision_id,
            market_id = %market.id,
            rule = self.name(),
            question = %market.question,
            %reason,
            "evaluated market timing rule"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed: within_window,
            reason,
        });

        within_window
    }
}

#[derive(Debug, Default)]
pub struct TemperatureDistanceRule;

impl StrategyRule for TemperatureDistanceRule {
    fn name(&self) -> &'static str {
        "TemperatureDistanceRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let distance_f = market.range.distance_from(input.forecast.forecast_high_f);
        trace.set_distance_f(distance_f);
        let passed = distance_f >= input.distance_threshold_f;
        let reason = format!(
            "forecast_high_f={:.1} range={} distance_f={:.1} threshold_f={:.1}",
            input.forecast.forecast_high_f,
            market.range.label(),
            distance_f,
            input.distance_threshold_f
        );

        debug!(
            run_id = %input.run_id,
            decision_id = %trace.decision_id,
            market_id = %market.id,
            rule = self.name(),
            distance_f,
            threshold_f = input.distance_threshold_f,
            passed,
            "evaluated temperature distance rule"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed,
            reason,
        });

        passed
    }
}

#[derive(Debug, Default)]
pub struct NoPriceBandRule;

impl StrategyRule for NoPriceBandRule {
    fn name(&self) -> &'static str {
        "NoPriceBandRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let Some(no_price) = market.no_price else {
            let reason = "market is missing a NO price".to_string();
            warn!(
                run_id = %input.run_id,
                decision_id = %trace.decision_id,
                market_id = %market.id,
                rule = self.name(),
                "market rejected because no NO price is available"
            );
            trace.push(RuleTrace {
                rule_name: self.name(),
                passed: false,
                reason,
            });
            return false;
        };

        let passed = no_price >= input.price_band.min && no_price <= input.price_band.max;
        let reason = format!(
            "no_price={:.3} price_band={:.2}-{:.2}",
            no_price, input.price_band.min, input.price_band.max
        );

        debug!(
            run_id = %input.run_id,
            decision_id = %trace.decision_id,
            market_id = %market.id,
            rule = self.name(),
            no_price,
            min_price = input.price_band.min,
            max_price = input.price_band.max,
            passed,
            "evaluated NO price band rule"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed,
            reason,
        });

        passed
    }
}

#[derive(Debug, Default)]
pub struct TokenAvailabilityRule;

impl StrategyRule for TokenAvailabilityRule {
    fn name(&self) -> &'static str {
        "TokenAvailabilityRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let passed = market.no_token_id.is_some();
        let reason = if let Some(token_id) = &market.no_token_id {
            format!("no_token_id={token_id}")
        } else {
            "market is missing a NO token id".to_string()
        };

        debug!(
            run_id = %input.run_id,
            decision_id = %trace.decision_id,
            market_id = %market.id,
            rule = self.name(),
            has_token = passed,
            "evaluated token availability rule"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed,
            reason,
        });

        passed
    }
}
