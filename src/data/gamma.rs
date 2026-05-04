use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use std::{thread, time};

const GAMMA_BASE: &str = "https://gamma-api.polymarket.com";
const CLOB_BASE: &str = "https://clob.polymarket.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedMarket {
    pub market_id: String,
    pub bin_label: String,
    pub bin_index: i64,
    pub token_id_yes: String,
    pub token_id_no: String,
    pub yes_price: f64,
    pub no_price: f64,
    pub yes_best_bid: f64,
    pub yes_best_ask: f64,
    pub last_trade: f64,
    pub liquidity: f64,
    pub volume_24h: f64,
    pub neg_risk: bool,
    pub closed: bool,
}

pub fn build_slug(city_slug: &str, target: NaiveDate) -> String {
    crate::config::build_daily_high_slug(city_slug, target)
}

pub fn fetch_event(city_slug: &str, target: NaiveDate) -> Result<Option<Value>> {
    fetch_event_by_slug(&build_slug(city_slug, target))
}

pub fn fetch_event_by_slug(slug: &str) -> Result<Option<Value>> {
    let payload = fetch_events_payload(&[("slug", slug.to_string())], 15)
        .with_context(|| format!("failed to fetch Gamma event slug {slug}"))?;
    let event = payload
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .or_else(|| payload.as_object().map(|_| payload.clone()));
    Ok(event.map(|mut event| {
        if let Some(obj) = event.as_object_mut() {
            obj.insert("_slug".to_string(), Value::String(slug.to_string()));
        }
        event
    }))
}

pub fn fetch_events_range(city_slug: &str, start: NaiveDate, end: NaiveDate) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut day = start;
    while day <= end {
        if let Ok(Some(event)) = fetch_event(city_slug, day) {
            out.push(event);
        }
        day = day
            .succ_opt()
            .context("date overflow while fetching event range")?;
        thread::sleep(time::Duration::from_millis(200));
    }
    Ok(out)
}

pub fn fetch_events_payload(params: &[(&str, String)], timeout_secs: u64) -> Result<Value> {
    let retries = 3;
    for attempt in 0..retries {
        let result = (|| -> Result<Value> {
            let payload = client(timeout_secs)?
                .get(format!("{GAMMA_BASE}/events"))
                .query(params)
                .send()
                .context("failed to send Gamma events request")?
                .error_for_status()
                .context("Gamma events request returned an error status")?
                .json()
                .context("failed to decode Gamma events response")?;
            Ok(payload)
        })();
        match result {
            Ok(payload) => return Ok(payload),
            Err(err) if attempt + 1 == retries => return Err(err),
            Err(_) => thread::sleep(time::Duration::from_secs(1_u64 << attempt)),
        }
    }
    unreachable!("retry loop always returns")
}

pub fn parse_markets(event: &Value) -> Vec<ParsedMarket> {
    event
        .get("markets")
        .and_then(Value::as_array)
        .map(|markets| {
            markets
                .iter()
                .enumerate()
                .map(|(idx, market)| {
                    let token_ids = parse_json_array_value(
                        market
                            .get("clobTokenIds")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    );
                    let prices = parse_json_array_value(
                        market
                            .get("outcomePrices")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    );
                    ParsedMarket {
                        market_id: value_to_string(market.get("id")).unwrap_or_default(),
                        bin_label: value_to_string(market.get("groupItemTitle"))
                            .unwrap_or_default(),
                        bin_index: idx as i64,
                        token_id_yes: value_to_string(token_ids.first()).unwrap_or_default(),
                        token_id_no: value_to_string(token_ids.get(1)).unwrap_or_default(),
                        yes_price: value_to_f64(prices.first()).unwrap_or(0.0),
                        no_price: value_to_f64(prices.get(1)).unwrap_or(0.0),
                        yes_best_bid: value_to_f64(market.get("bestBid")).unwrap_or(0.0),
                        yes_best_ask: value_to_f64(market.get("bestAsk")).unwrap_or(0.0),
                        last_trade: value_to_f64(market.get("lastTradePrice")).unwrap_or(0.0),
                        liquidity: value_to_f64(market.get("liquidityNum"))
                            .or_else(|| value_to_f64(market.get("liquidity")))
                            .unwrap_or(0.0),
                        volume_24h: value_to_f64(market.get("volume24hr"))
                            .or_else(|| value_to_f64(market.get("volume")))
                            .unwrap_or(0.0),
                        neg_risk: market
                            .get("negRisk")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        closed: market
                            .get("closed")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn fetch_order_book(token_id: &str) -> Result<Value> {
    let client = client(20)?;
    let payload = client
        .get(format!("{CLOB_BASE}/book"))
        .query(&[("token_id", token_id)])
        .send()
        .with_context(|| {
            format!(
                "failed to fetch CLOB order book for {}",
                abbreviated(token_id)
            )
        })?
        .error_for_status()?
        .json()?;
    Ok(payload)
}

pub fn fetch_fee_rate(token_id: &str) -> Result<Option<f64>> {
    let client = client(20)?;
    let payload: Value = client
        .get(format!("{CLOB_BASE}/fee-rate"))
        .query(&[("token_id", token_id)])
        .send()
        .with_context(|| {
            format!(
                "failed to fetch CLOB fee rate for {}",
                abbreviated(token_id)
            )
        })?
        .error_for_status()?
        .json()?;
    let raw = payload
        .get("base_fee")
        .or_else(|| payload.get("baseFee"))
        .or_else(|| payload.get("fee_rate_bps"))
        .or_else(|| payload.get("feeRateBps"));
    Ok(raw
        .and_then(|value| value_to_f64(Some(value)))
        .and_then(normalize_fee_rate))
}

pub fn normalize_fee_rate(raw: f64) -> Option<f64> {
    let rate = if raw > 1.0 { raw / 20_000.0 } else { raw };
    rate.is_finite().then_some(rate)
}

pub fn effective_ask_vwap(book: &Value, shares: f64) -> Option<f64> {
    let mut levels = book
        .get("asks")?
        .as_array()?
        .iter()
        .filter_map(|level| {
            Some((
                value_to_f64(level.get("price"))?,
                value_to_f64(level.get("size"))?,
            ))
        })
        .collect::<Vec<_>>();
    if levels.is_empty() || shares <= 0.0 {
        return None;
    }
    levels.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut remaining = shares;
    let mut cost = 0.0;
    for (price, size) in levels {
        let take = remaining.min(size);
        cost += take * price;
        remaining -= take;
        if remaining <= 1e-9 {
            return Some(cost / shares);
        }
    }
    None
}

pub fn book_depth(book: &Value, side: &str) -> f64 {
    book.get(side)
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| value_to_f64(level.get("size")))
                .sum()
        })
        .unwrap_or(0.0)
}

pub fn best_price(book: &Value, side: &str) -> f64 {
    let Some(levels) = book.get(side).and_then(Value::as_array) else {
        return 0.0;
    };
    let prices = levels
        .iter()
        .filter_map(|level| value_to_f64(level.get("price")))
        .collect::<Vec<_>>();
    if prices.is_empty() {
        return 0.0;
    }
    if side == "bids" {
        prices.into_iter().reduce(f64::max).unwrap_or(0.0)
    } else {
        prices.into_iter().reduce(f64::min).unwrap_or(0.0)
    }
}

pub fn winning_bin(event: &Value) -> Option<String> {
    for market in event.get("markets")?.as_array()? {
        let prices = parse_json_array_value(
            market
                .get("outcomePrices")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        );
        if value_to_f64(prices.first()).unwrap_or(0.0) >= 0.99 {
            return value_to_string(market.get("groupItemTitle"));
        }
    }
    None
}

fn client(timeout_secs: u64) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .context("failed to build HTTP client")
}

pub(crate) fn value_to_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|n| n as f64))
            .or_else(|| v.as_u64().map(|n| n as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    })
}

pub(crate) fn value_to_string(value: Option<&Value>) -> Option<String> {
    value.and_then(|v| {
        v.as_str()
            .map(ToString::to_string)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })
}

fn parse_json_array_value(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::String(raw) => serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn abbreviated(token_id: &str) -> String {
    token_id.chars().take(16).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_markets_with_encoded_arrays() {
        let event = json!({
            "markets": [{
                "id": "m1",
                "groupItemTitle": "70-71°F",
                "clobTokenIds": "[\"yes\",\"no\"]",
                "outcomePrices": "[\"0.25\",\"0.75\"]",
                "bestBid": "0.2",
                "bestAsk": "0.3",
                "liquidityNum": "100",
                "volume24hr": "12"
            }]
        });
        let markets = parse_markets(&event);
        assert_eq!(markets[0].token_id_yes, "yes");
        assert_eq!(markets[0].token_id_no, "no");
        assert_eq!(markets[0].yes_price, 0.25);
    }

    #[test]
    fn computes_book_helpers() {
        let book = json!({
            "asks": [{"price": "0.30", "size": "4"}, {"price": "0.40", "size": "6"}],
            "bids": [{"price": "0.20", "size": "2"}]
        });
        assert_eq!(best_price(&book, "asks"), 0.30);
        assert_eq!(best_price(&book, "bids"), 0.20);
        assert_eq!(book_depth(&book, "asks"), 10.0);
        assert!((effective_ask_vwap(&book, 10.0).unwrap() - 0.36).abs() < 1e-9);
    }

    #[test]
    fn normalizes_fee_rate_like_python() {
        assert_eq!(normalize_fee_rate(1000.0), Some(0.05));
        assert_eq!(normalize_fee_rate(0.05), Some(0.05));
    }
}
