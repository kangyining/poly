use tracing::{debug, info, warn};

use crate::polymarket::WeatherMarket;

use super::types::{DecisionTrace, RuleTrace, StrategyInput, within_trading_window};

pub trait StrategyRule: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool;
}

// ── Rule 1: Market must be within the 36h trading window ─────────────────────

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
        let now = input.evaluation_time_utc.unwrap_or_else(chrono::Utc::now);
        let in_window = within_trading_window(now, input.forecast.target_date);
        let reason = format!(
            "now={} target_date={} in_36h_window={}",
            now.to_rfc3339(),
            input.forecast.target_date,
            in_window
        );

        debug!(
            decision_id = %trace.decision_id,
            market_id = %market.id,
            rule = self.name(),
            in_window,
            "evaluated MarketTimingRule"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed: in_window,
            reason,
        });

        in_window
    }
}

// ── Rule 2: Market must have a NO token ID ────────────────────────────────────

#[derive(Debug, Default)]
pub struct TokenAvailabilityRule;

impl StrategyRule for TokenAvailabilityRule {
    fn name(&self) -> &'static str {
        "TokenAvailabilityRule"
    }

    fn evaluate(
        &self,
        _input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let passed = market.no_token_id.is_some();
        let reason = match &market.no_token_id {
            Some(id) => format!("no_token_id={id}"),
            None => "missing NO token id".to_string(),
        };

        if !passed {
            warn!(
                decision_id = %trace.decision_id,
                market_id = %market.id,
                "market rejected: missing NO token id"
            );
        }

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed,
            reason,
        });

        passed
    }
}

// ── Rule 3: Compute bucket probability via Gaussian CDF ───────────────────────

#[derive(Debug, Default)]
pub struct BucketProbabilityRule;

impl StrategyRule for BucketProbabilityRule {
    fn name(&self) -> &'static str {
        "BucketProbabilityRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let p = market.range.bucket_probability(input.mu_f, input.sigma_f);
        let reason = format!(
            "mu_f={:.2} sigma_f={:.2} range={} p_model={:.4}",
            input.mu_f,
            input.sigma_f,
            market.range.label(),
            p,
        );

        debug!(
            decision_id = %trace.decision_id,
            market_id = %market.id,
            p_model = p,
            "evaluated BucketProbabilityRule"
        );

        // This rule always passes — it annotates p_model into the trace for
        // the NoEdgeRule which needs it. We store p in the reason string.
        // The NoEdgeRule reads market data directly, so both rules see the market.
        trace.push(RuleTrace {
            rule_name: self.name(),
            passed: true,
            reason,
        });

        true
    }
}

// ── Rule 4: Fee-adjusted NO EV must clear threshold ──────────────────────────
//
// EV_NO = (1 - P_model) × (1 - fee_rate) - effective_ask
//
// fee_rate is the taker fee collected from the winning payout (from CLOB /fee-rate).
// This requires effective_ask_no and fee_rate from the CLOB (set during market fetch).

#[derive(Debug, Default)]
pub struct NoEdgeRule;

impl StrategyRule for NoEdgeRule {
    fn name(&self) -> &'static str {
        "NoEdgeRule"
    }

    fn evaluate(
        &self,
        input: &StrategyInput,
        market: &WeatherMarket,
        trace: &mut DecisionTrace,
    ) -> bool {
        let (Some(effective_ask), Some(fee_rate)) =
            (market.effective_ask_no, market.fee_rate)
        else {
            let reason = "no CLOB data available (effective_ask or fee_rate missing)".to_string();
            warn!(
                decision_id = %trace.decision_id,
                market_id = %market.id,
                "market rejected: missing CLOB data"
            );
            trace.push(RuleTrace {
                rule_name: self.name(),
                passed: false,
                reason,
            });
            return false;
        };

        let p = market.range.bucket_probability(input.mu_f, input.sigma_f);
        // Polymarket charges fee_rate on the winning payout (taker fee on profits).
        // If NO wins: net receipt per share = 1 - fee_rate
        // EV_NO = P(NO) × (1 - fee_rate) - effective_ask
        let ev_no = (1.0 - p) * (1.0 - fee_rate) - effective_ask;
        let passed = ev_no >= input.min_ev_threshold;

        let reason = format!(
            "p_model={:.4} effective_ask={:.4} fee_rate={:.4} ev_no={:.4} threshold={:.4} passed={}",
            p, effective_ask, fee_rate, ev_no, input.min_ev_threshold, passed
        );

        info!(
            bin         = %market.range.label(),
            p_model     = format_args!("{:.4}", p),
            effective_ask = format_args!("{:.4}", effective_ask),
            fee_rate    = format_args!("{:.4}", fee_rate),
            ev_no       = format_args!("{:.4}", ev_no),
            threshold   = format_args!("{:.4}", input.min_ev_threshold),
            passed,
            strategy    = %input.run_id.split('-').next().unwrap_or("?"),
            "bin EV evaluated"
        );

        trace.push(RuleTrace {
            rule_name: self.name(),
            passed,
            reason,
        });

        passed
    }
}
