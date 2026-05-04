use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TemperatureRange {
    BelowOrEqual { high_f: f64 },
    AboveOrEqual { low_f: f64 },
    Between { low_f: f64, high_f: f64 },
}

pub fn parse_temperature_range(label: &str) -> Option<TemperatureRange> {
    let normalized = label.replace(['°', 'F', 'f'], "");
    let between_re = between_re();
    if let Some(caps) = between_re.captures(&normalized) {
        return Some(TemperatureRange::Between {
            low_f: caps.get(1)?.as_str().parse().ok()?,
            high_f: caps.get(2)?.as_str().parse().ok()?,
        });
    }

    let lower_label = label.to_lowercase();
    let lower_marked = ["below", "lower", "≤", "<="]
        .iter()
        .any(|token| lower_label.contains(token));
    let upper_marked = ["above", "higher", "≥", ">="]
        .iter()
        .any(|token| lower_label.contains(token));
    let number_re = number_re();
    let value = number_re
        .captures(&normalized)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok());
    match (lower_marked, upper_marked, value) {
        (true, _, Some(high_f)) => Some(TemperatureRange::BelowOrEqual { high_f }),
        (_, true, Some(low_f)) => Some(TemperatureRange::AboveOrEqual { low_f }),
        _ => None,
    }
}

fn between_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(?:to|-|through)\s*(\d+(?:\.\d+)?)")
            .expect("valid temperature range regex")
    })
}

fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+(?:\.\d+)?)").expect("valid temperature number regex"))
}

pub fn norm_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf_compat(x / 2.0_f64.sqrt()))
}

#[cfg(target_os = "linux")]
fn erf_compat(x: f64) -> f64 {
    #[link(name = "m")]
    extern "C" {
        fn erf(x: f64) -> f64;
    }
    unsafe { erf(x) }
}

#[cfg(not(target_os = "linux"))]
fn erf_compat(x: f64) -> f64 {
    libm::erf(x)
}

pub fn bucket_probability(label: &str, mu_f: f64, sigma_f: f64) -> Option<f64> {
    if sigma_f <= 0.0 {
        return None;
    }
    match parse_temperature_range(label)? {
        TemperatureRange::BelowOrEqual { high_f } => {
            Some(norm_cdf((high_f + 0.5 - mu_f) / sigma_f))
        }
        TemperatureRange::AboveOrEqual { low_f } => {
            Some(1.0 - norm_cdf((low_f - 0.5 - mu_f) / sigma_f))
        }
        TemperatureRange::Between { low_f, high_f } => {
            let hi = norm_cdf((high_f + 0.5 - mu_f) / sigma_f);
            let lo = norm_cdf((low_f - 0.5 - mu_f) / sigma_f);
            Some((hi - lo).max(0.0))
        }
    }
}

pub fn entry_cost(price: f64, fee_rate: f64) -> f64 {
    price * (1.0 + fee_rate * (1.0 - price))
}

pub fn settle_single(
    side: &str,
    bin_label: &str,
    winning_bin: &str,
    shares: f64,
    cost_per_share: f64,
) -> (f64, bool) {
    let side = side.to_ascii_uppercase();
    let won =
        (bin_label == winning_bin && side == "YES") || (bin_label != winning_bin && side == "NO");
    if won {
        (shares * (1.0 - cost_per_share), true)
    } else {
        (-(shares * cost_per_share), false)
    }
}

pub fn settle_union(
    winning_bin: &str,
    included_bins: &[String],
    shares: f64,
    total_cost_per_share: f64,
) -> (f64, bool) {
    let won = included_bins.iter().any(|bin| bin == winning_bin);
    if won {
        (shares * (1.0 - total_cost_per_share), true)
    } else {
        (-(shares * total_cost_per_share), false)
    }
}

pub fn param_f64(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or(default)
}

pub fn param_opt_f64(params: &Value, key: &str) -> Option<f64> {
    params.get(key).and_then(Value::as_f64)
}

pub fn param_bool(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn min_edge(params: &Value, default: f64) -> f64 {
    param_f64(
        params,
        "min_ev_threshold",
        param_f64(params, "min_edge", default),
    )
}

pub fn candidate_size(price: f64, fee_rate: f64, params: &Value, multiplier: f64) -> (f64, f64) {
    let cost = entry_cost(price, fee_rate);
    if let Some(fixed_stake) = param_opt_f64(params, "fixed_trade_stake_usd") {
        let stake = fixed_stake * multiplier;
        if cost <= 0.0 {
            return (0.0, 0.0);
        }
        return (stake / cost, stake);
    }
    let shares = param_f64(params, "desired_shares", 10.0)
        * param_f64(params, "funding_pct", 1.0)
        * multiplier;
    (shares, shares * cost)
}

pub fn range_contains(range: &TemperatureRange, temp: f64) -> bool {
    match range {
        TemperatureRange::BelowOrEqual { high_f } => temp <= *high_f,
        TemperatureRange::AboveOrEqual { low_f } => temp >= *low_f,
        TemperatureRange::Between { low_f, high_f } => temp >= *low_f && temp <= *high_f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_temperature_ranges() {
        assert_eq!(
            parse_temperature_range("38-39°F"),
            Some(TemperatureRange::Between {
                low_f: 38.0,
                high_f: 39.0
            })
        );
        assert_eq!(
            parse_temperature_range("≤27°F"),
            Some(TemperatureRange::BelowOrEqual { high_f: 27.0 })
        );
        assert_eq!(
            parse_temperature_range("46°F or above"),
            Some(TemperatureRange::AboveOrEqual { low_f: 46.0 })
        );
    }

    #[test]
    fn computes_fee_cost_and_settlement() {
        assert!((entry_cost(0.7, 0.05) - 0.7105).abs() < 1e-9);
        let (pnl, won) = settle_single("NO", "38-39°F", "40-41°F", 10.0, 0.91);
        assert!(won);
        assert!((pnl - 0.9).abs() < 1e-9);
    }
}
