use std::collections::HashMap;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{sleep_until, Instant};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::polymarket::WeatherMarket;

const MARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const USER_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/user";

#[derive(Debug, Clone, Default)]
pub struct MarketSnapshot {
    pub books: HashMap<String, BookState>,
}

impl MarketSnapshot {
    pub fn update_market_prices(
        &self,
        markets: &mut [WeatherMarket],
        max_staleness: Duration,
    ) -> usize {
        let now = Utc::now();
        let mut updated = 0;

        for market in markets {
            let Some(no_token_id) = &market.no_token_id else {
                continue;
            };
            let Some(book) = self.books.get(no_token_id) else {
                continue;
            };
            if now - book.updated_at > max_staleness {
                warn!(
                    token_id = %no_token_id,
                    last_update = %book.updated_at,
                    "skipping stale websocket book price"
                );
                continue;
            }
            if let Some(best_ask) = book.best_ask {
                market.no_price = Some(best_ask);
                updated += 1;
            }
        }

        updated
    }
}

#[derive(Debug, Clone)]
pub struct BookState {
    pub asset_id: String,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PolymarketMarketDataClient {
    ws_url: String,
}

impl PolymarketMarketDataClient {
    pub fn new() -> Self {
        Self {
            ws_url: MARKET_WS_URL.to_string(),
        }
    }

    pub async fn sync_snapshot(
        &self,
        asset_ids: &[String],
        sync_for: StdDuration,
    ) -> Result<MarketSnapshot> {
        if asset_ids.is_empty() {
            return Ok(MarketSnapshot::default());
        }

        let (mut ws_stream, _) = connect_async(&self.ws_url)
            .await
            .with_context(|| format!("failed to connect to market websocket {}", self.ws_url))?;

        let subscribe = json!({
            "type": "market",
            "assets_ids": asset_ids,
        });
        ws_stream
            .send(Message::Text(subscribe.to_string().into()))
            .await
            .context("failed to send market websocket subscription")?;

        info!(
            asset_count = asset_ids.len(),
            sync_seconds = sync_for.as_secs(),
            "subscribed to Polymarket market websocket"
        );

        let deadline = Instant::now() + sync_for;
        let mut snapshot = MarketSnapshot::default();

        loop {
            tokio::select! {
                _ = sleep_until(deadline) => break,
                message = ws_stream.next() => {
                    let Some(message) = message else {
                        warn!("market websocket closed before sync deadline");
                        break;
                    };
                    let message = message.context("market websocket message failed")?;
                    if let Message::Text(text) = message {
                        handle_market_message(&text, &mut snapshot)?;
                    }
                }
            }
        }

        info!(
            book_count = snapshot.books.len(),
            "finished market websocket snapshot sync"
        );

        Ok(snapshot)
    }
}

fn handle_market_message(text: &str, snapshot: &mut MarketSnapshot) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("failed to parse market websocket JSON")?;

    if let Some(items) = value.as_array() {
        for item in items {
            handle_market_value(item, snapshot);
        }
    } else {
        handle_market_value(&value, snapshot);
    }

    Ok(())
}

fn handle_market_value(value: &serde_json::Value, snapshot: &mut MarketSnapshot) {
    let event_type = value
        .get("event_type")
        .or_else(|| value.get("type"))
        .and_then(|item| item.as_str())
        .unwrap_or_default();

    match event_type {
        "book" => handle_book_message(value, snapshot),
        "best_bid_ask" => handle_best_bid_ask_message(value, snapshot),
        "price_change" => handle_price_change_message(value, snapshot),
        "last_trade_price" => debug!("received last_trade_price market websocket message"),
        _ => debug!(event_type, "ignored market websocket message"),
    }
}

fn handle_book_message(value: &serde_json::Value, snapshot: &mut MarketSnapshot) {
    let Some(asset_id) = value.get("asset_id").and_then(|item| item.as_str()) else {
        return;
    };
    let best_bid = best_price(value.get("bids"), PriceSide::Bid);
    let best_ask = best_price(value.get("asks"), PriceSide::Ask);

    snapshot.books.insert(
        asset_id.to_string(),
        BookState {
            asset_id: asset_id.to_string(),
            best_bid,
            best_ask,
            updated_at: Utc::now(),
        },
    );
}

fn handle_best_bid_ask_message(value: &serde_json::Value, snapshot: &mut MarketSnapshot) {
    let Some(asset_id) = value.get("asset_id").and_then(|item| item.as_str()) else {
        return;
    };
    let best_bid = parse_number(value.get("bid").or_else(|| value.get("best_bid")));
    let best_ask = parse_number(value.get("ask").or_else(|| value.get("best_ask")));

    upsert_book(snapshot, asset_id, best_bid, best_ask);
}

fn handle_price_change_message(value: &serde_json::Value, snapshot: &mut MarketSnapshot) {
    let Some(changes) = value.get("changes").and_then(|item| item.as_array()) else {
        return;
    };

    for change in changes {
        let Some(asset_id) = change.get("asset_id").and_then(|item| item.as_str()) else {
            continue;
        };
        let best_bid = parse_number(change.get("best_bid"));
        let best_ask = parse_number(change.get("best_ask"));
        upsert_book(snapshot, asset_id, best_bid, best_ask);
    }
}

fn upsert_book(
    snapshot: &mut MarketSnapshot,
    asset_id: &str,
    best_bid: Option<f64>,
    best_ask: Option<f64>,
) {
    let entry = snapshot
        .books
        .entry(asset_id.to_string())
        .or_insert_with(|| BookState {
            asset_id: asset_id.to_string(),
            best_bid: None,
            best_ask: None,
            updated_at: Utc::now(),
        });

    if best_bid.is_some() {
        entry.best_bid = best_bid;
    }
    if best_ask.is_some() {
        entry.best_ask = best_ask;
    }
    entry.updated_at = Utc::now();
}

#[derive(Debug, Clone, Copy)]
enum PriceSide {
    Bid,
    Ask,
}

fn best_price(value: Option<&serde_json::Value>, side: PriceSide) -> Option<f64> {
    let prices = value?.as_array()?.iter().filter_map(|item| {
        item.get("price")
            .or_else(|| item.get(0))
            .and_then(|price| parse_number(Some(price)))
    });

    match side {
        PriceSide::Bid => prices.max_by(f64::total_cmp),
        PriceSide::Ask => prices.min_by(f64::total_cmp),
    }
}

fn parse_number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    value.as_str()?.parse().ok()
}

#[derive(Debug, Clone)]
pub struct PolymarketUserStreamClient {
    ws_url: String,
}

impl PolymarketUserStreamClient {
    pub fn new() -> Self {
        Self {
            ws_url: USER_WS_URL.to_string(),
        }
    }

    pub async fn connect_requires_auth(&self) -> Result<()> {
        anyhow::bail!(
            "user websocket {} requires authenticated Polymarket credentials; not connected",
            self.ws_url
        )
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UserStreamMessage {
    event_type: Option<String>,
}
