use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::bus::EventBus;
use crate::events::{
    BestBidAskChanged, ConnectionStatusChanged, Event, EventSource, MarketEvent,
    MarketMetadataRefreshed,
};
use crate::polymarket::{PolymarketClient, WeatherMarket};

const MARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

pub struct MarketSource {
    bus: EventBus,
    client: PolymarketClient,
    event_slug: String,
    target_date: chrono::NaiveDate,
    desired_shares: f64,
}

impl MarketSource {
    pub fn new(
        bus: EventBus,
        client: PolymarketClient,
        event_slug: String,
        target_date: chrono::NaiveDate,
        desired_shares: f64,
    ) -> Self {
        Self {
            bus,
            client,
            event_slug,
            target_date,
            desired_shares,
        }
    }

    pub async fn bootstrap_metadata(&self) -> Result<Vec<WeatherMarket>> {
        let markets = self
            .client
            .fetch_event_markets(&self.event_slug, self.target_date, self.desired_shares)
            .await?;

        self.bus.publish(
            EventSource::App,
            Event::Market(MarketEvent::MetadataRefreshed(MarketMetadataRefreshed {
                event_slug: self.event_slug.clone(),
                target_date: self.target_date,
                markets: std::sync::Arc::new(markets.clone()),
            })),
        );

        Ok(markets)
    }

    pub async fn run(self, markets: Vec<WeatherMarket>) -> Result<()> {
        let asset_ids = markets
            .iter()
            .filter_map(|market| market.no_token_id.clone())
            .collect::<Vec<_>>();

        if asset_ids.is_empty() {
            warn!("market source has no NO token ids to subscribe to");
            return Ok(());
        }

        loop {
            if let Err(error) = self.run_stream(&asset_ids).await {
                warn!(error = %error, "market websocket source disconnected, retrying");
                self.bus.publish(
                    EventSource::MarketWs,
                    Event::Market(MarketEvent::ConnectionStatusChanged(
                        ConnectionStatusChanged {
                            connected: false,
                            message: error.to_string(),
                        },
                    )),
                );
                sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn run_stream(&self, asset_ids: &[String]) -> Result<()> {
        let (mut ws_stream, _) = connect_async(MARKET_WS_URL)
            .await
            .context("failed to connect to market websocket")?;

        let subscribe = json!({
            "type": "market",
            "assets_ids": asset_ids,
        });
        ws_stream
            .send(Message::Text(subscribe.to_string().into()))
            .await
            .context("failed to subscribe to market websocket")?;

        info!(asset_count = asset_ids.len(), "market websocket connected");
        self.bus.publish(
            EventSource::MarketWs,
            Event::Market(MarketEvent::ConnectionStatusChanged(
                ConnectionStatusChanged {
                    connected: true,
                    message: "connected".into(),
                },
            )),
        );

        while let Some(message) = ws_stream.next().await {
            let message = message.context("market websocket message failed")?;
            if let Message::Text(text) = message {
                self.handle_message(&text)?;
            }
        }

        Ok(())
    }

    fn handle_message(&self, text: &str) -> Result<()> {
        let value: serde_json::Value =
            serde_json::from_str(text).context("failed to parse market websocket message")?;

        if let Some(items) = value.as_array() {
            for item in items {
                self.handle_value(item);
            }
        } else {
            self.handle_value(&value);
        }

        Ok(())
    }

    fn handle_value(&self, value: &serde_json::Value) {
        let event_type = value
            .get("event_type")
            .or_else(|| value.get("type"))
            .and_then(|item| item.as_str())
            .unwrap_or_default();

        match event_type {
            "book" => {
                let token_id = value.get("asset_id").and_then(|item| item.as_str());
                let best_bid = best_price(value.get("bids"), true);
                let best_ask = best_price(value.get("asks"), false);
                self.publish_price_change(token_id, best_bid, best_ask);
            }
            "best_bid_ask" => {
                let token_id = value.get("asset_id").and_then(|item| item.as_str());
                let best_bid = parse_number(value.get("bid").or_else(|| value.get("best_bid")));
                let best_ask = parse_number(value.get("ask").or_else(|| value.get("best_ask")));
                self.publish_price_change(token_id, best_bid, best_ask);
            }
            "price_change" => {
                if let Some(changes) = value.get("changes").and_then(|item| item.as_array()) {
                    for change in changes {
                        let token_id = change.get("asset_id").and_then(|item| item.as_str());
                        let best_bid = parse_number(change.get("best_bid"));
                        let best_ask = parse_number(change.get("best_ask"));
                        self.publish_price_change(token_id, best_bid, best_ask);
                    }
                }
            }
            "last_trade_price" => debug!("received last_trade_price market websocket message"),
            _ => debug!(event_type, "ignored market websocket message"),
        }
    }

    fn publish_price_change(
        &self,
        token_id: Option<&str>,
        best_bid: Option<f64>,
        best_ask: Option<f64>,
    ) {
        let Some(token_id) = token_id else {
            return;
        };
        self.bus.publish(
            EventSource::MarketWs,
            Event::Market(MarketEvent::BestBidAskChanged(BestBidAskChanged {
                token_id: token_id.to_string(),
                best_bid,
                best_ask,
            })),
        );
    }
}

fn best_price(value: Option<&serde_json::Value>, is_bid: bool) -> Option<f64> {
    let prices = value?.as_array()?.iter().filter_map(|item| {
        item.get("price")
            .or_else(|| item.get(0))
            .and_then(|price| parse_number(Some(price)))
    });

    if is_bid {
        prices.max_by(f64::total_cmp)
    } else {
        prices.min_by(f64::total_cmp)
    }
}

fn parse_number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    value.as_str()?.parse().ok()
}
