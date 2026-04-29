use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::core::bus::EventBus;
use crate::core::events::{
    BestBidAskChanged, ConnectionStatusChanged, Event, EventSource, MarketMetadataRefreshed,
};
use crate::market::client::{effective_ask_vwap, PolymarketClient, WeatherMarket};

const MARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

#[derive(Clone)]
pub struct MarketService {
    bus: EventBus,
    client: PolymarketClient,
    event_slug: String,
    target_date: NaiveDate,
    desired_shares: f64,
}

impl MarketService {
    pub fn new(
        bus: EventBus,
        gamma_url: &str,
        clob_url: &str,
        event_slug: String,
        target_date: NaiveDate,
        desired_shares: f64,
    ) -> Self {
        Self {
            bus,
            client: PolymarketClient::new(gamma_url, clob_url),
            event_slug,
            target_date,
            desired_shares,
        }
    }

    pub async fn fetch_event_markets(
        &self,
        event_slug: &str,
        target_date: NaiveDate,
        desired_shares: f64,
    ) -> Result<Vec<WeatherMarket>> {
        let payload = self.client.get_event_by_slug(event_slug).await?;

        let end_date = payload
            .end_date
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
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
            .filter_map(|market| WeatherMarket::try_from_gamma(market, target_date))
            .collect();

        info!(
            event_slug,
            event_title,
            market_count = markets.len(),
            "fetched weather event markets from Gamma"
        );

        for market in &mut markets {
            if let Some(no_token_id) = &market.no_token_id {
                match self.fetch_clob_data(no_token_id, desired_shares).await {
                    Ok((effective_ask, fee_rate)) => {
                        market.effective_ask_no = Some(effective_ask);
                        market.fee_rate = Some(fee_rate);
                    }
                    Err(error) => {
                        warn!(
                            market_id = %market.id,
                            no_token_id = %no_token_id,
                            error = %error,
                            "failed to fetch CLOB data, market will be skipped by EV rule"
                        );
                    }
                }
            }
        }

        Ok(markets)
    }

    pub async fn publish_metadata(&self) -> Result<Vec<WeatherMarket>> {
        let markets = self
            .fetch_event_markets(&self.event_slug, self.target_date, self.desired_shares)
            .await?;

        self.bus.publish(
            EventSource::App,
            Event::MetadataRefreshed(MarketMetadataRefreshed {
                event_slug: self.event_slug.clone(),
                target_date: self.target_date,
                markets: Arc::new(markets.clone()),
            }),
        );

        Ok(markets)
    }

    pub async fn run_poller(self: Arc<Self>, markets: Vec<WeatherMarket>) -> Result<()> {
        let asset_ids = markets
            .iter()
            .filter_map(|market| market.no_token_id.clone())
            .collect::<Vec<_>>();

        if asset_ids.is_empty() {
            warn!("market service has no NO token ids to subscribe to");
            return Ok(());
        }

        loop {
            if let Err(error) = self.publish_stream(&asset_ids).await {
                warn!(error = %error, "market websocket service disconnected, retrying");
                self.bus.publish(
                    EventSource::MarketWs,
                    Event::ConnectionStatusChanged(ConnectionStatusChanged {
                        connected: false,
                        message: error.to_string(),
                    }),
                );
                sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn fetch_clob_data(&self, no_token_id: &str, desired_shares: f64) -> Result<(f64, f64)> {
        let (book, fee_rate) = tokio::try_join!(
            self.client.get_order_book(no_token_id),
            self.client.get_fee_rate(no_token_id),
        )?;

        let effective_ask = effective_ask_vwap(&book.asks, desired_shares).ok_or_else(|| {
            anyhow::anyhow!("insufficient ask depth for {} shares", desired_shares)
        })?;

        Ok((effective_ask, fee_rate))
    }

    async fn publish_stream(&self, asset_ids: &[String]) -> Result<()> {
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
            Event::ConnectionStatusChanged(ConnectionStatusChanged {
                connected: true,
                message: "connected".into(),
            }),
        );

        while let Some(message) = ws_stream.next().await {
            let message = message.context("market websocket message failed")?;
            if let Message::Text(text) = message {
                self.consume_message(&text)?;
            }
        }

        Ok(())
    }

    fn consume_message(&self, text: &str) -> Result<()> {
        let value: serde_json::Value =
            serde_json::from_str(text).context("failed to parse market websocket message")?;

        if let Some(items) = value.as_array() {
            for item in items {
                self.consume_value(item);
            }
        } else {
            self.consume_value(&value);
        }

        Ok(())
    }

    fn consume_value(&self, value: &serde_json::Value) {
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
            Event::BestBidAskChanged(BestBidAskChanged {
                token_id: token_id.to_string(),
                best_bid,
                best_ask,
            }),
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
