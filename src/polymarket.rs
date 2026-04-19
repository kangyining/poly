use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct PolymarketClient {
    http: Client,
    pub gamma_url: String,
    pub clob_url: String,
}

impl PolymarketClient {
    pub fn new(gamma_url: impl Into<String>, clob_url: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            gamma_url: gamma_url.into(),
            clob_url: clob_url.into(),
        }
    }

    /// Fetch all markets for an event slug from Gamma, then enrich each with
    /// CLOB orderbook data (effective_ask_no, fee_rate).
    pub async fn fetch_event_markets(
        &self,
        event_slug: &str,
        target_date: NaiveDate,
        desired_shares: f64,
    ) -> Result<Vec<WeatherMarket>> {
        let url = format!(
            "{}/events/slug/{}",
            self.gamma_url.trim_end_matches('/'),
            event_slug
        );
        let payload: GammaEvent = self
            .http
            .get(&url)
            .send()
            .await
            .context("failed to query Gamma event by slug")?
            .error_for_status()
            .context("Gamma event request returned an error")?
            .json()
            .await
            .context("failed to deserialize Gamma event response")?;

        let end_date = payload
            .end_date
            .as_deref()
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if let Some(end_dt) = end_date {
            if end_dt.date_naive() != target_date {
                anyhow::bail!(
                    "event {} resolves on {}, expected {}",
                    event_slug,
                    end_dt.date_naive(),
                    target_date
                );
            }
        }

        let event_title = payload.title.clone();
        let mut markets: Vec<WeatherMarket> = payload
            .markets
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| WeatherMarket::try_from_gamma(m, target_date))
            .collect();

        info!(
            event_slug,
            event_title,
            market_count = markets.len(),
            "fetched weather event markets from Gamma"
        );

        // Enrich with CLOB data
        for market in &mut markets {
            if let Some(no_token_id) = &market.no_token_id {
                match self.fetch_clob_data(no_token_id, desired_shares).await {
                    Ok((effective_ask, fee_rate)) => {
                        market.effective_ask_no = Some(effective_ask);
                        market.fee_rate = Some(fee_rate);
                    }
                    Err(e) => {
                        warn!(
                            market_id = %market.id,
                            no_token_id = %no_token_id,
                            error = %e,
                            "failed to fetch CLOB data, market will be skipped by EV rule"
                        );
                    }
                }
            }
        }

        Ok(markets)
    }

    /// Fetches the CLOB orderbook and fee rate for a NO token.
    /// Returns (effective_ask, fee_rate).
    async fn fetch_clob_data(
        &self,
        no_token_id: &str,
        desired_shares: f64,
    ) -> Result<(f64, f64)> {
        let (book, fee_rate) = tokio::try_join!(
            self.fetch_order_book(no_token_id),
            self.fetch_fee_rate(no_token_id),
        )?;

        let effective_ask = effective_ask_vwap(&book.asks, desired_shares)
            .ok_or_else(|| anyhow::anyhow!("insufficient ask depth for {} shares", desired_shares))?;

        Ok((effective_ask, fee_rate))
    }

    async fn fetch_order_book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book", self.clob_url.trim_end_matches('/'));
        let raw: RawOrderBook = self
            .http
            .get(&url)
            .query(&[("token_id", token_id)])
            .send()
            .await
            .context("CLOB /book request failed")?
            .error_for_status()
            .context("CLOB /book returned error status")?
            .json()
            .await
            .context("failed to parse CLOB /book response")?;

        let asks = raw
            .asks
            .into_iter()
            .filter_map(|l| {
                let price = l.price.parse::<f64>().ok()?;
                let size = l.size.parse::<f64>().ok()?;
                Some(OrderLevel { price, size })
            })
            .collect();

        Ok(OrderBook { asks })
    }

    async fn fetch_fee_rate(&self, token_id: &str) -> Result<f64> {
        let url = format!("{}/fee-rate", self.clob_url.trim_end_matches('/'));
        let raw: FeeRateResponse = self
            .http
            .get(&url)
            .query(&[("token_id", token_id)])
            .send()
            .await
            .context("CLOB /fee-rate request failed")?
            .error_for_status()
            .context("CLOB /fee-rate returned error status")?
            .json()
            .await
            .context("failed to parse CLOB /fee-rate response")?;

        // Endpoint returns basis points (e.g. 1000 = 10%). Convert to decimal.
        let rate = if raw.base_fee > 1.0 {
            raw.base_fee / 10_000.0
        } else {
            raw.base_fee
        };
        Ok(rate)
    }
}

/// Walk ask levels and return the VWAP price for buying `shares`.
/// Returns None if insufficient depth.
pub fn effective_ask_vwap(asks: &[OrderLevel], shares: f64) -> Option<f64> {
    let mut remaining = shares;
    let mut cost = 0.0;

    for level in asks {
        let take = remaining.min(level.size);
        cost += take * level.price;
        remaining -= take;
        if remaining <= 1e-9 {
            return Some(cost / shares);
        }
    }
    None
}

// ── Market types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WeatherMarket {
    pub id: String,
    pub question: String,
    pub slug: Option<String>,
    pub end_date: Option<DateTime<Utc>>,
    pub range: TemperatureRange,
    pub yes_price: Option<f64>,
    pub no_price: Option<f64>,
    pub yes_token_id: Option<String>,
    pub no_token_id: Option<String>,
    /// VWAP ask price for desired_shares on the NO token CLOB book
    pub effective_ask_no: Option<f64>,
    /// Normalized fee rate for this NO token (e.g. 0.05)
    pub fee_rate: Option<f64>,
}

impl WeatherMarket {
    fn try_from_gamma(market: GammaMarket, target_date: NaiveDate) -> Option<Self> {
        if !market.enable_order_book.unwrap_or(false) {
            return None;
        }

        let question = market.question?;
        let range = TemperatureRange::parse(&question)?;

        let end_date = market
            .end_date
            .as_deref()
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if let Some(end_dt) = end_date {
            if end_dt.date_naive() != target_date {
                return None;
            }
        }

        let outcome_names = parse_string_array(market.outcomes.as_deref());
        let outcome_prices = parse_f64_array(market.outcome_prices.as_deref());
        let token_ids = parse_string_array(market.clob_token_ids.as_deref());

        let mut yes_price = None;
        let mut no_price = None;
        let mut yes_token_id = None;
        let mut no_token_id = None;

        for (idx, name) in outcome_names.iter().enumerate() {
            if name.eq_ignore_ascii_case("yes") {
                yes_price = outcome_prices.get(idx).copied();
                yes_token_id = token_ids.get(idx).cloned();
            } else if name.eq_ignore_ascii_case("no") {
                no_price = outcome_prices.get(idx).copied();
                no_token_id = token_ids.get(idx).cloned();
            }
        }

        Some(Self {
            id: market.id.unwrap_or_else(|| question.clone()),
            question,
            slug: market.slug,
            end_date,
            range,
            yes_price,
            no_price,
            yes_token_id,
            no_token_id,
            effective_ask_no: None,
            fee_rate: None,
        })
    }
}

// ── Temperature range ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum TemperatureRange {
    BelowOrEqual { upper_f: f64 },
    Between { low_f: f64, high_f: f64 },
    AboveOrEqual { lower_f: f64 },
}

impl TemperatureRange {
    /// Gaussian CDF bucket probability: P(temp ∈ bin) given N(mu_f, sigma_f).
    /// Uses ±0.5°F half-integer correction for whole-degree bucket boundaries.
    pub fn bucket_probability(&self, mu_f: f64, sigma_f: f64) -> f64 {
        match self {
            Self::BelowOrEqual { upper_f } => {
                norm_cdf((*upper_f + 0.5 - mu_f) / sigma_f)
            }
            Self::Between { low_f, high_f } => {
                let hi = norm_cdf((*high_f + 0.5 - mu_f) / sigma_f);
                let lo = norm_cdf((*low_f - 0.5 - mu_f) / sigma_f);
                (hi - lo).max(0.0)
            }
            Self::AboveOrEqual { lower_f } => {
                1.0 - norm_cdf((*lower_f - 0.5 - mu_f) / sigma_f)
            }
        }
    }

    pub fn distance_from(self, forecast_high_f: f64) -> f64 {
        match self {
            Self::BelowOrEqual { upper_f } => (forecast_high_f - upper_f).max(0.0),
            Self::Between { low_f, high_f } => {
                if (low_f..=high_f).contains(&forecast_high_f) {
                    0.0
                } else if forecast_high_f < low_f {
                    low_f - forecast_high_f
                } else {
                    forecast_high_f - high_f
                }
            }
            Self::AboveOrEqual { lower_f } => (lower_f - forecast_high_f).max(0.0),
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        let normalized = input.replace('°', "").replace('F', "").replace('f', "");

        let below = Regex::new(
            r"(?i)(\d+(?:\.\d+)?)\s*(?:or below|and below|below|or lower|and lower|lower)",
        )
        .ok()?;
        if let Some(cap) = below.captures(&normalized) {
            return Some(Self::BelowOrEqual {
                upper_f: cap.get(1)?.as_str().parse().ok()?,
            });
        }

        let above = Regex::new(
            r"(?i)(\d+(?:\.\d+)?)\s*(?:or above|and above|above|or higher|and higher|higher)",
        )
        .ok()?;
        if let Some(cap) = above.captures(&normalized) {
            return Some(Self::AboveOrEqual {
                lower_f: cap.get(1)?.as_str().parse().ok()?,
            });
        }

        let between =
            Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(?:to|-|through)\s*(\d+(?:\.\d+)?)").ok()?;
        if let Some(cap) = between.captures(&normalized) {
            return Some(Self::Between {
                low_f: cap.get(1)?.as_str().parse().ok()?,
                high_f: cap.get(2)?.as_str().parse().ok()?,
            });
        }

        None
    }

    pub fn label(self) -> String {
        match self {
            Self::BelowOrEqual { upper_f } => format!("{upper_f:.0}F or below"),
            Self::Between { low_f, high_f } => format!("{low_f:.0}F to {high_f:.0}F"),
            Self::AboveOrEqual { lower_f } => format!("{lower_f:.0}F or above"),
        }
    }
}

// ── Normal CDF ────────────────────────────────────────────────────────────────

/// Gaussian CDF via Abramowitz & Stegun erf approximation (max error < 1.5e-7).
fn norm_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0_f64 } else { 1.0_f64 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t
            - 0.284_496_736)
            * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

// ── CLOB / Gamma deserialization types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OrderLevel {
    pub price: f64,
    pub size: f64,
}

#[derive(Debug, Deserialize)]
struct RawOrderLevel {
    price: String,
    size: String,
}

#[derive(Debug)]
pub struct OrderBook {
    pub asks: Vec<OrderLevel>,
}

#[derive(Debug, Deserialize)]
struct RawOrderBook {
    #[serde(default)]
    asks: Vec<RawOrderLevel>,
}

#[derive(Debug, Deserialize)]
struct FeeRateResponse {
    base_fee: f64,
}

#[derive(Debug, Deserialize)]
struct GammaMarket {
    id: Option<String>,
    question: Option<String>,
    slug: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
    #[serde(rename = "enableOrderBook")]
    enable_order_book: Option<bool>,
    outcomes: Option<String>,
    #[serde(rename = "outcomePrices")]
    outcome_prices: Option<String>,
    #[serde(rename = "clobTokenIds")]
    clob_token_ids: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GammaEvent {
    title: String,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
    markets: Option<Vec<GammaMarket>>,
}

fn parse_string_array(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

fn parse_f64_array(value: Option<&str>) -> Vec<f64> {
    let Some(raw) = value else {
        return Vec::new();
    };
    if let Ok(items) = serde_json::from_str::<Vec<String>>(raw) {
        return items
            .into_iter()
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
    }
    serde_json::from_str::<Vec<f64>>(raw).unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{TemperatureRange, effective_ask_vwap, OrderLevel};

    #[test]
    fn parses_below_ranges() {
        let r = TemperatureRange::parse("Will the high temperature be 44F or below?");
        matches!(r, Some(TemperatureRange::BelowOrEqual { upper_f }) if upper_f == 44.0);
    }

    #[test]
    fn parses_between_ranges() {
        match TemperatureRange::parse("Will the high temperature be 48F to 50F?") {
            Some(TemperatureRange::Between { low_f, high_f }) => {
                assert_eq!(low_f, 48.0);
                assert_eq!(high_f, 50.0);
            }
            _ => panic!("expected between range"),
        }
    }

    #[test]
    fn parses_above_ranges() {
        match TemperatureRange::parse("Will the high temperature be 56F or above?") {
            Some(TemperatureRange::AboveOrEqual { lower_f }) => assert_eq!(lower_f, 56.0),
            _ => panic!("expected above-or-equal range"),
        }
    }

    #[test]
    fn bucket_probability_sums_to_one() {
        // With mu=50, sigma=3, probabilities across a full span should sum ~1
        let mu = 50.0_f64;
        let sigma = 3.0_f64;
        let p_below = TemperatureRange::BelowOrEqual { upper_f: 44.0 }.bucket_probability(mu, sigma);
        let p_mid = TemperatureRange::Between { low_f: 45.0, high_f: 55.0 }.bucket_probability(mu, sigma);
        let p_above = TemperatureRange::AboveOrEqual { lower_f: 56.0 }.bucket_probability(mu, sigma);
        let total = p_below + p_mid + p_above;
        // Not exactly 1 because gaps exist between bins, but should be < 1 and sum reasonably
        assert!(total > 0.95 && total <= 1.001, "total={total}");
    }

    #[test]
    fn effective_ask_walks_book() {
        let asks = vec![
            OrderLevel { price: 0.96, size: 5.0 },
            OrderLevel { price: 0.97, size: 10.0 },
        ];
        // Want 8 shares: 5 @ 0.96 + 3 @ 0.97
        let ea = effective_ask_vwap(&asks, 8.0).unwrap();
        let expected = (5.0 * 0.96 + 3.0 * 0.97) / 8.0;
        assert!((ea - expected).abs() < 1e-10);
    }

    #[test]
    fn effective_ask_returns_none_when_depth_insufficient() {
        let asks = vec![OrderLevel { price: 0.96, size: 3.0 }];
        assert!(effective_ask_vwap(&asks, 10.0).is_none());
    }
}
