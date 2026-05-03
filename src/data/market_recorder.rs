use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::data::gamma;
use crate::data::models::MarketSnapshot;
use crate::data::weather_contracts;
use crate::db::{self, Database};

pub const DEFAULT_VWAP_SHARES: f64 = 10.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRunSummary {
    pub contracts_seen: usize,
    pub contracts_valid: usize,
    pub contracts_skipped: usize,
    pub snapshots_saved: usize,
    pub order_book_errors: usize,
}

pub fn dates_to_record(now: DateTime<Utc>) -> Vec<NaiveDate> {
    vec![now.date_naive(), (now + Duration::days(1)).date_naive()]
}

pub fn hours_to_close(ts: DateTime<Utc>, target_date: &str) -> Option<f64> {
    let target = NaiveDate::parse_from_str(target_date, "%Y-%m-%d").ok()?;
    let close = target.and_hms_opt(12, 0, 0)?.and_utc();
    Some((close - ts).num_seconds() as f64 / 3600.0)
}

pub fn record_market_snapshots(db: &Database) -> Result<SnapshotRunSummary> {
    let now = Utc::now();
    let mut summary = SnapshotRunSummary {
        contracts_seen: 0,
        contracts_valid: 0,
        contracts_skipped: 0,
        snapshots_saved: 0,
        order_book_errors: 0,
    };
    let discoveries =
        weather_contracts::discover_configured_weather_contracts(&dates_to_record(now), true);
    for discovery in discoveries {
        summary.contracts_seen += 1;
        db::upsert_weather_contract(db, &discovery.contract)?;
        if !discovery.is_valid() {
            summary.contracts_skipped += 1;
            continue;
        }
        summary.contracts_valid += 1;
        let Some(event) = discovery.event else {
            continue;
        };
        if event
            .get("closed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !all_markets_open(&event)
        {
            continue;
        }
        let event_id = gamma::value_to_string(event.get("id")).unwrap_or_default();
        let event_slug = event
            .get("_slug")
            .or_else(|| event.get("slug"))
            .and_then(Value::as_str)
            .unwrap_or(&discovery.contract.event_slug)
            .to_string();
        for market in gamma::parse_markets(&event) {
            let mut no_book = Value::Object(Default::default());
            let mut yes_book = Value::Object(Default::default());
            let mut fee_rate = None;
            let mut no_vwap_10 = None;
            if !market.token_id_no.is_empty() {
                match gamma::fetch_order_book(&market.token_id_no) {
                    Ok(book) => {
                        no_vwap_10 = gamma::effective_ask_vwap(&book, DEFAULT_VWAP_SHARES);
                        no_book = book;
                    }
                    Err(_) => summary.order_book_errors += 1,
                }
                if let Ok(rate) = gamma::fetch_fee_rate(&market.token_id_no) {
                    fee_rate = rate;
                }
            }
            if !market.token_id_yes.is_empty() {
                match gamma::fetch_order_book(&market.token_id_yes) {
                    Ok(book) => yes_book = book,
                    Err(_) => summary.order_book_errors += 1,
                }
            }
            let no_asks_json = no_book
                .get("asks")
                .and_then(Value::as_array)
                .map(|asks| Value::Array(asks.clone()).to_string());
            let snapshot = MarketSnapshot {
                timestamp: now,
                city: discovery.contract.display_city.clone(),
                target_date: discovery.contract.target_date.clone(),
                hours_to_close: hours_to_close(now, &discovery.contract.target_date).unwrap_or(0.0),
                event_id: event_id.clone(),
                event_slug: event_slug.clone(),
                bin_label: market.bin_label,
                bin_index: market.bin_index,
                token_id_yes: market.token_id_yes,
                token_id_no: market.token_id_no,
                yes_price: market.yes_price,
                no_price: market.no_price,
                yes_best_bid: gamma::best_price(&yes_book, "bids"),
                yes_best_ask: gamma::best_price(&yes_book, "asks"),
                no_best_bid: gamma::best_price(&no_book, "bids"),
                no_best_ask: gamma::best_price(&no_book, "asks"),
                book_depth_bid: gamma::book_depth(&no_book, "bids"),
                book_depth_ask: gamma::book_depth(&no_book, "asks"),
                no_asks_json,
                fee_rate,
                no_vwap_10,
                volume_24h: market.volume_24h,
                liquidity: market.liquidity,
                market_type: Some(discovery.contract.market_type.clone()),
                measure: Some(discovery.contract.measure.clone()),
                unit: Some(discovery.contract.unit.clone()),
                city_slug: Some(discovery.contract.city_slug.clone()),
                station_id: Some(discovery.contract.station_id.clone()),
                source_url: Some(discovery.contract.source_url.clone()),
                validation_status: Some(discovery.contract.validation_status.clone()),
                skip_reason: discovery.contract.skip_reason.clone(),
            };
            db::insert_market_snapshot(db, &snapshot)?;
            summary.snapshots_saved += 1;
        }
    }
    Ok(summary)
}

pub fn all_markets_open(event: &Value) -> bool {
    event
        .get("markets")
        .and_then(Value::as_array)
        .map(|markets| {
            markets.iter().any(|market| {
                !market
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn computes_hours_to_noon_utc() {
        let ts = "2026-04-19T06:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(hours_to_close(ts, "2026-04-19"), Some(6.0));
    }

    #[test]
    fn detects_any_open_market() {
        let event = json!({"markets": [{"closed": true}, {"closed": false}]});
        assert!(all_markets_open(&event));
    }
}
