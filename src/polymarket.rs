use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PolymarketClient {
    http: Client,
    gamma_url: String,
    #[allow(dead_code)]
    clob_url: String,
}

impl PolymarketClient {
    pub fn new(gamma_url: impl Into<String>, clob_url: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            gamma_url: gamma_url.into(),
            clob_url: clob_url.into(),
        }
    }

    pub async fn fetch_event_markets(
        &self,
        event_slug: &str,
        target_date: NaiveDate,
    ) -> Result<Vec<WeatherMarket>> {
        let url = format!(
            "{}/events/slug/{}",
            self.gamma_url.trim_end_matches('/'),
            event_slug
        );
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("failed to query Gamma event by slug")?
            .error_for_status()
            .context("Gamma event request returned an error")?;

        let payload: GammaEvent = response
            .json()
            .await
            .context("failed to deserialize Gamma event response")?;

        let end_date = payload
            .end_date
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if let Some(end_dt) = end_date {
            if end_dt.date_naive() != target_date {
                anyhow::bail!(
                    "event {} resolves on {}, expected target_date {}",
                    event_slug,
                    end_dt.date_naive(),
                    target_date
                );
            }
        }

        let event_title = payload.title.clone();
        let markets: Vec<WeatherMarket> = payload
            .markets
            .unwrap_or_default()
            .into_iter()
            .filter_map(|market| WeatherMarket::try_from_gamma(market, target_date))
            .collect();

        info!(
            event_slug,
            event_title,
            market_count = markets.len(),
            "fetched weather event markets from Gamma"
        );

        Ok(markets)
    }
}

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
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
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
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TemperatureRange {
    BelowOrEqual { upper_f: f64 },
    Between { low_f: f64, high_f: f64 },
    AboveOrEqual { lower_f: f64 },
}

impl TemperatureRange {
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
        if let Some(captures) = below.captures(&normalized) {
            let upper_f = captures.get(1)?.as_str().parse().ok()?;
            return Some(Self::BelowOrEqual { upper_f });
        }

        let above = Regex::new(
            r"(?i)(\d+(?:\.\d+)?)\s*(?:or above|and above|above|or higher|and higher|higher)",
        )
        .ok()?;
        if let Some(captures) = above.captures(&normalized) {
            let lower_f = captures.get(1)?.as_str().parse().ok()?;
            return Some(Self::AboveOrEqual { lower_f });
        }

        let between =
            Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(?:to|-|through)\s*(\d+(?:\.\d+)?)").ok()?;
        if let Some(captures) = between.captures(&normalized) {
            let low_f = captures.get(1)?.as_str().parse().ok()?;
            let high_f = captures.get(2)?.as_str().parse().ok()?;
            return Some(Self::Between { low_f, high_f });
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
            .filter_map(|item| item.parse::<f64>().ok())
            .collect();
    }

    if let Ok(items) = serde_json::from_str::<Vec<f64>>(raw) {
        return items;
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::TemperatureRange;

    #[test]
    fn parses_below_ranges() {
        let parsed = TemperatureRange::parse("Will the high temperature be 44F or below?");
        match parsed {
            Some(TemperatureRange::BelowOrEqual { upper_f }) => assert_eq!(upper_f, 44.0),
            _ => panic!("expected below-or-equal range"),
        }
    }

    #[test]
    fn parses_between_ranges() {
        let parsed = TemperatureRange::parse("Will the high temperature be 48F to 50F?");
        match parsed {
            Some(TemperatureRange::Between { low_f, high_f }) => {
                assert_eq!(low_f, 48.0);
                assert_eq!(high_f, 50.0);
            }
            _ => panic!("expected between range"),
        }
    }

    #[test]
    fn parses_above_ranges() {
        let parsed = TemperatureRange::parse("Will the high temperature be 56F or above?");
        match parsed {
            Some(TemperatureRange::AboveOrEqual { lower_f }) => assert_eq!(lower_f, 56.0),
            _ => panic!("expected above-or-equal range"),
        }
    }

    #[test]
    fn parses_higher_ranges() {
        let parsed = TemperatureRange::parse("96F or higher");
        match parsed {
            Some(TemperatureRange::AboveOrEqual { lower_f }) => assert_eq!(lower_f, 96.0),
            _ => panic!("expected above-or-equal range"),
        }
    }

    #[test]
    fn computes_distance_from_forecast() {
        let below = TemperatureRange::BelowOrEqual { upper_f: 44.0 };
        let between = TemperatureRange::Between {
            low_f: 48.0,
            high_f: 50.0,
        };
        let above = TemperatureRange::AboveOrEqual { lower_f: 56.0 };

        assert_eq!(below.distance_from(50.0), 6.0);
        assert_eq!(between.distance_from(50.0), 0.0);
        assert_eq!(above.distance_from(50.0), 6.0);
    }
}
