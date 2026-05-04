use serde_json::{json, Map, Value};

use crate::config;
use crate::strategy::core::*;
use crate::strategy::utils::*;

fn event_slug(context: &StrategyContext) -> String {
    context
        .event_slug
        .clone()
        .unwrap_or_else(|| config::default_event_slug(&context.city, &context.target_date))
}

fn market_id(context: &StrategyContext, label: &str) -> String {
    format!(
        "backtest-{}-{}-{}",
        context.city, context.target_date, label
    )
}

fn token(context: &StrategyContext, side: &str, label: &str) -> String {
    let safe = label.replace([' ', '/'], "_");
    format!(
        "backtest-{}-{}-{}-{}",
        context.city,
        context.target_date,
        side.to_lowercase(),
        safe
    )
}

fn single_candidate(
    strategy: &str,
    context: &StrategyContext,
    market: &MarketBin,
    side: &str,
    p_model: f64,
    edge: f64,
    price: f64,
    fee_rate: f64,
    metadata: Map<String, Value>,
) -> StrategyCandidate {
    let (shares, stake_usd) = candidate_size(price, fee_rate, &context.params, 1.0);
    let token_id = if side == "NO" {
        market.token_id_no.clone()
    } else {
        market.token_id_yes.clone()
    }
    .unwrap_or_else(|| token(context, side, &market.bin_label));
    StrategyCandidate {
        strategy_name: strategy.to_string(),
        side: side.to_string(),
        bin_label: market.bin_label.clone(),
        effective_ask: price,
        fee_rate,
        shares,
        stake_usd,
        p_model: Some(p_model),
        edge,
        fill_source: market.fill_source.clone(),
        market_id: market_id(context, &market.bin_label),
        token_id,
        event_slug: event_slug(context),
        opened_at: market.opened_at.clone(),
        settled_at: None,
        metadata,
    }
}

fn metadata(items: &[(&str, Value)]) -> Map<String, Value> {
    items
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn python_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let next = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - next) + value;
        } else {
            compensation += (value - next) + total;
        }
        total = next;
    }
    total + compensation
}

pub struct BaseNoEvStrategy;
impl Strategy for BaseNoEvStrategy {
    fn name(&self) -> &'static str {
        "base_no_ev"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &[
            "forecast_records",
            "price_history_or_market_snapshots",
            "settlement_records",
        ]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let mut out = StrategyOutput::default();
        let threshold = min_edge(&context.params, 0.05);
        for market in &context.bins {
            let Some(p_yes) =
                bucket_probability(&market.bin_label, forecast.mu_f, forecast.sigma_f)
            else {
                out.diagnostics.push(StrategyDiagnostic {
                    strategy_name: self.name().to_string(),
                    severity: "skip".to_string(),
                    reason: format!("unparseable bin {}", market.bin_label),
                    required_data: vec![],
                });
                continue;
            };
            let cost = entry_cost(market.effective_ask_no, market.fee_rate);
            let edge = (1.0 - p_yes) - cost;
            if edge >= threshold {
                out.candidates.push(single_candidate(
                    self.name(),
                    context,
                    market,
                    "NO",
                    p_yes,
                    edge,
                    market.effective_ask_no,
                    market.fee_rate,
                    metadata(&[
                        ("mu_f", json!(forecast.mu_f)),
                        ("sigma_f", json!(forecast.sigma_f)),
                        ("forecast_source", json!(forecast.source)),
                    ]),
                ));
            }
        }
        out
    }
}

pub struct BaseYesNoEvStrategy;
impl Strategy for BaseYesNoEvStrategy {
    fn name(&self) -> &'static str {
        "base_yes_no_ev"
    }
    fn required_data(&self) -> &'static [&'static str] {
        BaseNoEvStrategy.required_data()
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        generate_yes_no(self.name(), context)
    }
}

fn generate_yes_no(strategy_name: &str, context: &StrategyContext) -> StrategyOutput {
    let Some(forecast) = context.forecast.as_ref() else {
        return missing(strategy_name, "forecast unavailable", &["forecast_records"]);
    };
    let mut out = StrategyOutput::default();
    let threshold = min_edge(&context.params, 0.05);
    for market in &context.bins {
        let Some(p_yes) = bucket_probability(&market.bin_label, forecast.mu_f, forecast.sigma_f)
        else {
            continue;
        };
        let yes_cost = entry_cost(market.yes_price, market.fee_rate);
        let yes_edge = p_yes - yes_cost;
        if yes_edge >= threshold {
            out.candidates.push(single_candidate(
                strategy_name,
                context,
                market,
                "YES",
                p_yes,
                yes_edge,
                market.yes_price,
                market.fee_rate,
                metadata(&[
                    ("mu_f", json!(forecast.mu_f)),
                    ("sigma_f", json!(forecast.sigma_f)),
                ]),
            ));
        }
        let no_cost = entry_cost(market.effective_ask_no, market.fee_rate);
        let no_edge = (1.0 - p_yes) - no_cost;
        if no_edge >= threshold {
            out.candidates.push(single_candidate(
                strategy_name,
                context,
                market,
                "NO",
                p_yes,
                no_edge,
                market.effective_ask_no,
                market.fee_rate,
                metadata(&[
                    ("mu_f", json!(forecast.mu_f)),
                    ("sigma_f", json!(forecast.sigma_f)),
                ]),
            ));
        }
    }
    out
}

pub struct EnsembleBucketEvStrategy;
impl Strategy for EnsembleBucketEvStrategy {
    fn name(&self) -> &'static str {
        "ensemble_bucket_ev"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &[
            "forecast_records.gfs_f",
            "forecast_records.ecmwf_f",
            "price_history_or_market_snapshots",
        ]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let members = [forecast.gfs_f, forecast.ecmwf_f]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if members.len() < 2 {
            return missing(
                self.name(),
                "requires at least two model forecasts",
                self.required_data(),
            );
        }
        let mut out = StrategyOutput::default();
        let threshold = min_edge(&context.params, 0.05);
        let sigma = forecast
            .sigma_f
            .max(param_f64(&context.params, "ensemble_sigma_floor", 1.2));
        for market in &context.bins {
            let probs = members
                .iter()
                .filter_map(|member| {
                    bucket_probability(&market.bin_label, member + forecast.beta_f, sigma)
                })
                .collect::<Vec<_>>();
            if probs.is_empty() {
                continue;
            }
            let p_yes = python_sum(probs.iter().copied()) / probs.len() as f64;
            for (side, price, payoff_prob) in [
                ("YES", market.yes_price, p_yes),
                ("NO", market.effective_ask_no, 1.0 - p_yes),
            ] {
                let cost = entry_cost(price, market.fee_rate);
                let edge = payoff_prob - cost;
                if edge >= threshold {
                    out.candidates.push(single_candidate(
                        self.name(),
                        context,
                        market,
                        side,
                        p_yes,
                        edge,
                        price,
                        market.fee_rate,
                        metadata(&[
                            ("members_f", json!(members)),
                            ("member_count", json!(members.len())),
                            ("sigma_f", json!(sigma)),
                        ]),
                    ));
                }
            }
        }
        out
    }
}

pub struct DynamicSigmaEvStrategy;
impl Strategy for DynamicSigmaEvStrategy {
    fn name(&self) -> &'static str {
        "dynamic_sigma_ev"
    }
    fn required_data(&self) -> &'static [&'static str] {
        BaseNoEvStrategy.required_data()
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let spread = forecast.model_spread_f.unwrap_or(0.0);
        let lead_component = (forecast.lead_hours - 6.0).max(0.0) / 36.0;
        let dynamic_sigma = forecast
            .sigma_f
            .max(param_f64(&context.params, "dynamic_sigma_min", 0.8) + lead_component)
            .max(spread * param_f64(&context.params, "spread_sigma_multiplier", 0.5))
            .min(param_f64(&context.params, "dynamic_sigma_max", 5.5));
        let mut adjusted = context.clone();
        let mut f = forecast.clone();
        f.sigma_f = dynamic_sigma;
        adjusted.forecast = Some(f);
        generate_yes_no(self.name(), &adjusted)
    }
}

pub struct BayesianClimatologyEvStrategy;
impl Strategy for BayesianClimatologyEvStrategy {
    fn name(&self) -> &'static str {
        "bayesian_climatology_ev"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &[
            "settlement_records.settlement_temp_f",
            "forecast_records",
            "price_history_or_market_snapshots",
            "settlement_records",
        ]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        if context.climatology_temps_f.is_empty() {
            return missing(
                self.name(),
                "no climatology settlement temperatures available",
                &["settlement_records"],
            );
        }
        let mut out = StrategyOutput::default();
        let threshold = min_edge(&context.params, 0.05);
        let model_weight = param_f64(&context.params, "model_weight", 0.7);
        for market in &context.bins {
            let Some(model_p) =
                bucket_probability(&market.bin_label, forecast.mu_f, forecast.sigma_f)
            else {
                continue;
            };
            let Some(range) = parse_temperature_range(&market.bin_label) else {
                continue;
            };
            let hits = context
                .climatology_temps_f
                .iter()
                .filter(|temp| range_contains(&range, **temp))
                .count();
            let clim_p = hits as f64 / context.climatology_temps_f.len() as f64;
            let p_yes = model_weight * model_p + (1.0 - model_weight) * clim_p;
            for (side, price, payoff_prob) in [
                ("YES", market.yes_price, p_yes),
                ("NO", market.effective_ask_no, 1.0 - p_yes),
            ] {
                let cost = entry_cost(price, market.fee_rate);
                let edge = payoff_prob - cost;
                if edge >= threshold {
                    out.candidates.push(single_candidate(
                        self.name(),
                        context,
                        market,
                        side,
                        p_yes,
                        edge,
                        price,
                        market.fee_rate,
                        metadata(&[
                            ("model_p", json!(model_p)),
                            ("climatology_p", json!(clim_p)),
                            ("model_weight", json!(model_weight)),
                        ]),
                    ));
                }
            }
        }
        out
    }
}

pub struct CorridorLadderStrategy;
impl Strategy for CorridorLadderStrategy {
    fn name(&self) -> &'static str {
        "corridor_ladder"
    }
    fn required_data(&self) -> &'static [&'static str] {
        BaseNoEvStrategy.required_data()
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let mut bins = context.bins.clone();
        bins.sort_by_key(|b| b.bin_index);
        if bins.len() < 2 {
            return missing(
                self.name(),
                "needs multiple bucket markets",
                &["price_history"],
            );
        }
        let mut out = StrategyOutput::default();
        let threshold = param_f64(
            &context.params,
            "corridor_min_edge",
            min_edge(&context.params, 0.05),
        );
        let max_width = param_f64(&context.params, "max_corridor_width", 3.0) as usize;
        for width in 2..=max_width {
            if width > bins.len() {
                continue;
            }
            for start in 0..=bins.len().saturating_sub(width) {
                let group = &bins[start..start + width];
                let probs = group
                    .iter()
                    .map(|m| bucket_probability(&m.bin_label, forecast.mu_f, forecast.sigma_f))
                    .collect::<Option<Vec<_>>>();
                let Some(probs) = probs else { continue };
                let p_yes = python_sum(probs.iter().copied());
                let total_cost = group
                    .iter()
                    .map(|m| entry_cost(m.yes_price, m.fee_rate))
                    .collect::<Vec<_>>();
                let total_cost = python_sum(total_cost);
                let edge = p_yes - total_cost;
                if edge >= threshold {
                    let labels = group
                        .iter()
                        .map(|m| m.bin_label.clone())
                        .collect::<Vec<_>>();
                    let label = format!("corridor:{}", labels.join("|"));
                    let (shares, stake_usd) = candidate_size(total_cost, 0.0, &context.params, 1.0);
                    out.candidates.push(StrategyCandidate {
                        strategy_name: self.name().to_string(),
                        side: "YES_CORRIDOR".to_string(),
                        bin_label: label.clone(),
                        effective_ask: total_cost,
                        fee_rate: group.iter().map(|m| m.fee_rate).fold(0.0, f64::max),
                        shares,
                        stake_usd,
                        p_model: Some(p_yes),
                        edge,
                        fill_source: "corridor_sum".to_string(),
                        market_id: market_id(context, &label),
                        token_id: token(context, "corridor", &label),
                        event_slug: event_slug(context),
                        opened_at: group.first().and_then(|m| m.opened_at.clone()),
                        settled_at: None,
                        metadata: metadata(&[
                            ("underlying_bins", json!(labels)),
                            ("width", json!(width)),
                        ]),
                    });
                }
            }
        }
        out
    }
}

pub struct ThresholdUnionStrategy;
impl Strategy for ThresholdUnionStrategy {
    fn name(&self) -> &'static str {
        "threshold_union"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &[
            "market_relationships_or_bucket_ladder",
            "forecast_records",
            "price_history_or_market_snapshots",
            "settlement_records",
        ]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let mut bins = context.bins.clone();
        bins.sort_by_key(|b| b.bin_index);
        if bins.len() < 3 {
            return missing(
                self.name(),
                "needs bucket ladder to synthesize thresholds",
                &["price_history"],
            );
        }
        let mut out = StrategyOutput::default();
        let threshold = min_edge(&context.params, 0.05);
        for cut in 1..bins.len() {
            for (side_name, group) in [("LOW_TAIL", &bins[..cut]), ("HIGH_TAIL", &bins[cut..])] {
                let probs = group
                    .iter()
                    .map(|m| bucket_probability(&m.bin_label, forecast.mu_f, forecast.sigma_f))
                    .collect::<Option<Vec<_>>>();
                let Some(probs) = probs else { continue };
                let p_yes = python_sum(probs.iter().copied());
                let total_cost = group
                    .iter()
                    .map(|m| entry_cost(m.yes_price, m.fee_rate))
                    .collect::<Vec<_>>();
                let total_cost = python_sum(total_cost);
                let edge = p_yes - total_cost;
                if edge >= threshold {
                    let labels = group
                        .iter()
                        .map(|m| m.bin_label.clone())
                        .collect::<Vec<_>>();
                    let label = format!(
                        "threshold_{}:{}",
                        side_name.to_lowercase(),
                        labels.join("|")
                    );
                    let (shares, stake_usd) = candidate_size(total_cost, 0.0, &context.params, 1.0);
                    out.candidates.push(StrategyCandidate {
                        strategy_name: self.name().to_string(),
                        side: format!("YES_{}", side_name),
                        bin_label: label.clone(),
                        effective_ask: total_cost,
                        fee_rate: group.iter().map(|m| m.fee_rate).fold(0.0, f64::max),
                        shares,
                        stake_usd,
                        p_model: Some(p_yes),
                        edge,
                        fill_source: "threshold_union_sum".to_string(),
                        market_id: market_id(context, &label),
                        token_id: token(context, side_name, &label),
                        event_slug: event_slug(context),
                        opened_at: group.first().and_then(|m| m.opened_at.clone()),
                        settled_at: None,
                        metadata: metadata(&[
                            ("underlying_bins", json!(labels)),
                            ("cut_index", json!(cut)),
                        ]),
                    });
                }
            }
        }
        out
    }
}

pub struct BasketArbStrategy;
impl Strategy for BasketArbStrategy {
    fn name(&self) -> &'static str {
        "basket_arb"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &["complete_bucket_ladder", "executable_yes_prices"]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let mut bins = context.bins.clone();
        bins.sort_by_key(|b| b.bin_index);
        if bins.len() < 3 {
            return missing(
                self.name(),
                "needs full multi-bucket event",
                &["price_history"],
            );
        }
        let total_cost = bins
            .iter()
            .map(|m| entry_cost(m.yes_price, m.fee_rate))
            .collect::<Vec<_>>();
        let total_cost = python_sum(total_cost);
        let edge = 1.0 - total_cost;
        if edge
            < param_f64(
                &context.params,
                "arb_buffer",
                min_edge(&context.params, 0.01),
            )
        {
            return StrategyOutput::default();
        }
        let labels = bins.iter().map(|m| m.bin_label.clone()).collect::<Vec<_>>();
        let (shares, stake_usd) = candidate_size(total_cost, 0.0, &context.params, 1.0);
        StrategyOutput {
            candidates: vec![StrategyCandidate {
                strategy_name: self.name().to_string(),
                side: "YES_BASKET".to_string(),
                bin_label: format!("basket:{}", labels.join("|")),
                effective_ask: total_cost,
                fee_rate: bins.iter().map(|m| m.fee_rate).fold(0.0, f64::max),
                shares,
                stake_usd,
                p_model: Some(1.0),
                edge,
                fill_source: "basket_sum".to_string(),
                market_id: market_id(context, "basket"),
                token_id: token(context, "basket", "all"),
                event_slug: event_slug(context),
                opened_at: bins.first().and_then(|m| m.opened_at.clone()),
                settled_at: None,
                metadata: metadata(&[("underlying_bins", json!(labels))]),
            }],
            diagnostics: Vec::new(),
        }
    }
}

pub struct MakerQuoteSimStrategy;
impl Strategy for MakerQuoteSimStrategy {
    fn name(&self) -> &'static str {
        "maker_quote_sim"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &["market_snapshots.no_best_bid", "forecast_records"]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let mut out = StrategyOutput::default();
        let threshold = min_edge(&context.params, 0.05);
        let tick = param_f64(&context.params, "tick_size", 0.01);
        let allow_proxy = param_bool(&context.params, "maker_proxy_from_price_history", true);
        for market in &context.bins {
            let mut proxy_used = false;
            let no_best_bid = market.no_best_bid.or_else(|| {
                if allow_proxy {
                    proxy_used = true;
                    Some((market.no_price - tick).max(0.0))
                } else {
                    None
                }
            });
            let Some(no_best_bid) = no_best_bid else {
                continue;
            };
            let Some(p_yes) =
                bucket_probability(&market.bin_label, forecast.mu_f, forecast.sigma_f)
            else {
                continue;
            };
            let quote = market.effective_ask_no.min(no_best_bid + tick);
            let edge = (1.0 - p_yes) - quote;
            if edge >= threshold {
                out.candidates.push(single_candidate(
                    self.name(),
                    context,
                    market,
                    "NO",
                    p_yes,
                    edge,
                    quote,
                    0.0,
                    metadata(&[
                        ("maker_quote", json!(quote)),
                        ("no_best_bid", json!(no_best_bid)),
                        ("tick_size", json!(tick)),
                        ("maker_proxy_from_price_history", json!(proxy_used)),
                    ]),
                ));
            }
        }
        if out.candidates.is_empty() && context.bins.iter().all(|m| m.no_best_bid.is_none()) {
            out.diagnostics.push(StrategyDiagnostic {
                strategy_name: self.name().to_string(),
                severity: "missing_data".to_string(),
                reason: "no maker-side bid snapshots".to_string(),
                required_data: vec!["market_snapshots.no_best_bid".to_string()],
            });
        }
        out
    }
}

pub struct NowcastMetarStrategy;
impl Strategy for NowcastMetarStrategy {
    fn name(&self) -> &'static str {
        "nowcast_metar"
    }
    fn required_data(&self) -> &'static [&'static str] {
        &[
            "metar_records",
            "forecast_records",
            "price_history_or_market_snapshots",
            "settlement_records",
        ]
    }
    fn generate_candidates(&self, context: &StrategyContext) -> StrategyOutput {
        let Some(forecast) = context.forecast.as_ref() else {
            return missing(self.name(), "forecast unavailable", &["forecast_records"]);
        };
        let Some(metar_max) = context.metar_max_so_far_f else {
            return missing(
                self.name(),
                "no METAR max available before evaluation time",
                &["metar_records"],
            );
        };
        let hours = context
            .bins
            .first()
            .map(|b| b.hours_to_close)
            .unwrap_or(forecast.lead_hours)
            .clamp(0.0, 36.0);
        let elapsed = 1.0 - (hours / 36.0);
        let mut f = forecast.clone();
        f.mu_f = forecast
            .mu_f
            .max(elapsed * metar_max + (1.0 - elapsed) * forecast.mu_f);
        f.sigma_f = (forecast.sigma_f * (1.0 - elapsed).sqrt().max(0.35)).max(0.6);
        let mut adjusted = context.clone();
        adjusted.forecast = Some(f);
        generate_yes_no(self.name(), &adjusted)
    }
}

pub struct UnsupportedResearchStrategy {
    pub name: &'static str,
    pub reason: &'static str,
    pub required: &'static [&'static str],
}

impl Strategy for UnsupportedResearchStrategy {
    fn name(&self) -> &'static str {
        self.name
    }
    fn required_data(&self) -> &'static [&'static str] {
        self.required
    }
    fn generate_candidates(&self, _context: &StrategyContext) -> StrategyOutput {
        missing(self.name, self.reason, self.required)
    }
}

pub fn strategy_by_name(name: &str) -> Box<dyn Strategy> {
    match name {
        "base_ev" | "base_no_ev" => Box::new(BaseNoEvStrategy),
        "base_yes_no_ev" => Box::new(BaseYesNoEvStrategy),
        "ensemble_bucket_ev" => Box::new(EnsembleBucketEvStrategy),
        "dynamic_sigma_ev" => Box::new(DynamicSigmaEvStrategy),
        "bayesian_climatology_ev" => Box::new(BayesianClimatologyEvStrategy),
        "corridor_ladder" => Box::new(CorridorLadderStrategy),
        "threshold_union" => Box::new(ThresholdUnionStrategy),
        "basket_arb" => Box::new(BasketArbStrategy),
        "maker_quote_sim" => Box::new(MakerQuoteSimStrategy),
        "nowcast_metar" => Box::new(NowcastMetarStrategy),
        "monthly_remainder" => Box::new(UnsupportedResearchStrategy { name: "monthly_remainder", reason: "monthly precipitation/snow remainder data is not normalized in this DB yet", required: &["monthly_realized_totals", "monthly_contract_metadata"] }),
        "cross_market_semantic_arb" => Box::new(UnsupportedResearchStrategy { name: "cross_market_semantic_arb", reason: "requires linked threshold/bucket markets with verified equivalent rule text", required: &["market_relationships"] }),
        "exit_overlay" => Box::new(UnsupportedResearchStrategy { name: "exit_overlay", reason: "requires position lifecycle price path simulation after entry; tracked as a follow-up overlay", required: &["post_entry_price_path"] }),
        "sharp_wallet_copy" => Box::new(UnsupportedResearchStrategy { name: "sharp_wallet_copy", reason: "requires configured wallet allowlist and fetched public Data API activity", required: &["wallet_signal_snapshots"] }),
        "resolution_lag_anomaly" => Box::new(UnsupportedResearchStrategy { name: "resolution_lag_anomaly", reason: "requires official-source lag features and anomaly labels", required: &["official_source_publication_times"] }),
        _ => Box::new(BaseNoEvStrategy),
    }
}

pub fn plugin_names() -> Vec<&'static str> {
    vec![
        "base_no_ev",
        "base_yes_no_ev",
        "ensemble_bucket_ev",
        "dynamic_sigma_ev",
        "bayesian_climatology_ev",
        "corridor_ladder",
        "threshold_union",
        "basket_arb",
        "maker_quote_sim",
        "nowcast_metar",
        "monthly_remainder",
        "cross_market_semantic_arb",
        "exit_overlay",
        "sharp_wallet_copy",
        "resolution_lag_anomaly",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> StrategyContext {
        StrategyContext {
            city: "NYC".to_string(),
            target_date: "2026-04-19".to_string(),
            winning_bin: "60-61°F".to_string(),
            bins: vec![MarketBin {
                bin_label: "38-39°F".to_string(),
                bin_index: 0,
                yes_price: 0.02,
                no_price: 0.98,
                effective_ask_no: 0.9,
                fee_rate: 0.05,
                hours_to_close: 10.0,
                token_id_yes: None,
                token_id_no: None,
                no_best_bid: Some(0.89),
                no_best_ask: Some(0.9),
                fill_source: "test".to_string(),
                opened_at: None,
            }],
            forecast: Some(ForecastState {
                source: "test".to_string(),
                forecast_f: 60.0,
                lead_hours: 10.0,
                beta_f: 0.0,
                sigma_f: 2.0,
                mu_f: 60.0,
                gfs_f: Some(60.0),
                ecmwf_f: Some(61.0),
                model_spread_f: Some(1.0),
            }),
            params: json!({"min_ev_threshold": 0.01, "desired_shares": 10.0}),
            replay_mode: "interval".to_string(),
            evaluation_time: None,
            event_slug: None,
            settlement_temp_f: None,
            metar_max_so_far_f: None,
            climatology_temps_f: vec![59.0, 60.0, 61.0],
        }
    }

    #[test]
    fn base_no_generates_candidate() {
        let out = BaseNoEvStrategy.generate_candidates(&context());
        assert_eq!(out.candidates.len(), 1);
        assert_eq!(out.candidates[0].side, "NO");
    }
}
