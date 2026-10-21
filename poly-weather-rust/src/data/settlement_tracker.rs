use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

use crate::config;
use crate::data::{gamma, weather_contracts};
use crate::db::{self, Database, NewSettlement};
use crate::strategy::utils::entry_cost;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettlementSummary {
    pub contracts_seen: usize,
    pub settlements_saved: usize,
    pub positions_closed: usize,
    pub skipped_existing: usize,
    pub fetch_errors: usize,
}

pub fn daily_check(
    db: &Database,
    target_date: Option<NaiveDate>,
    positions_path: Option<&Path>,
) -> Result<SettlementSummary> {
    let target = target_date.unwrap_or_else(|| Utc::now().date_naive() - Duration::days(1));
    let summary = check_dates(db, &[target], None, false)?;
    if let Some(path) = positions_path {
        db::export_open_positions_json(db, path)?;
    }
    Ok(summary)
}

pub fn backfill(db: &Database, days_back: i64, city: Option<&str>) -> Result<SettlementSummary> {
    let today = Utc::now().date_naive();
    let dates = (1..=days_back.max(0))
        .map(|offset| today - Duration::days(offset))
        .collect::<Vec<_>>();
    check_dates(db, &dates, city, true)
}

pub fn check_dates(
    db: &Database,
    dates: &[NaiveDate],
    city: Option<&str>,
    require_closed: bool,
) -> Result<SettlementSummary> {
    let city_filter = city.and_then(config::contract_for_city).map(|cfg| cfg.slug);
    let mut summary = SettlementSummary::default();
    for target in dates {
        for configured in weather_contracts::configured_weather_contracts(&[*target]) {
            if let Some(city_filter) = city_filter {
                if configured.city_slug != city_filter {
                    continue;
                }
            }
            summary.contracts_seen += 1;
            if db::settlement_exists(db, &configured.display_city, &configured.target_date)? {
                summary.skipped_existing += 1;
                continue;
            }
            let event = match gamma::fetch_event_by_slug(&configured.event_slug) {
                Ok(Some(event)) => event,
                Ok(None) => continue,
                Err(_) => {
                    summary.fetch_errors += 1;
                    continue;
                }
            };
            let validation = weather_contracts::validate_weather_event(&event);
            db::upsert_weather_contract(db, &validation.contract)?;
            if !validation.is_valid() {
                continue;
            }
            if require_closed
                && !event
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }
            let Some(rec) = extract_settlement(&event, &validation.contract) else {
                continue;
            };
            db::insert_settlement(db, &rec)?;
            summary.settlements_saved += 1;
            summary.positions_closed += settle_positions(
                db,
                &rec.target_date,
                &rec.winning_bin,
                rec.resolved_at.as_deref(),
                Some(&rec.event_slug),
            )?;
        }
    }
    Ok(summary)
}

pub fn extract_settlement(
    event: &Value,
    contract: &crate::data::models::WeatherContract,
) -> Option<NewSettlement> {
    let win_bin = gamma::winning_bin(event)?;
    let settlement_temp = parse_bin_midpoint(&win_bin);
    let resolved_at = event
        .get("closedTime")
        .or_else(|| event.get("updatedAt"))
        .and_then(Value::as_str)
        .and_then(crate::db::parse_ts)
        .map(|dt| dt.to_rfc3339());
    Some(NewSettlement {
        city: contract.display_city.clone(),
        target_date: contract.target_date.clone(),
        event_id: gamma::value_to_string(event.get("id")).unwrap_or_default(),
        event_slug: event
            .get("_slug")
            .and_then(Value::as_str)
            .unwrap_or(&contract.event_slug)
            .to_string(),
        winning_bin: win_bin.clone(),
        settlement_temp_f: settlement_temp,
        wunderground_station: contract.station_id.clone(),
        resolved_at,
        raw_outcome_prices: outcome_snapshot(event).to_string(),
        market_type: Some(contract.market_type.clone()),
        measure: Some(contract.measure.clone()),
        unit: Some(contract.unit.clone()),
        city_slug: Some(contract.city_slug.clone()),
        station_id: Some(contract.station_id.clone()),
        source_url: Some(contract.source_url.clone()),
        settlement_value: settlement_temp,
        settlement_unit: Some(contract.unit.clone()),
        settlement_temp_source: Some("polymarket-gamma-outcome".to_string()),
        settlement_temp_station: Some(contract.station_id.clone()),
        settlement_temp_raw_json: Some(
            json!({
                "source": "polymarket-gamma-outcome",
                "event_slug": contract.event_slug,
                "winning_bin": win_bin,
                "settlement_temp_f": settlement_temp,
            })
            .to_string(),
        ),
    })
}

fn outcome_snapshot(event: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(markets) = event.get("markets").and_then(Value::as_array) {
        for market in markets {
            if let Some(label) = market.get("groupItemTitle").and_then(Value::as_str) {
                out.insert(
                    label.to_string(),
                    market.get("outcomePrices").cloned().unwrap_or(Value::Null),
                );
            }
        }
    }
    Value::Object(out)
}

pub fn parse_bin_midpoint(bin_label: &str) -> Option<f64> {
    let label = bin_label
        .replace("°F", "")
        .replace('F', "")
        .trim()
        .to_string();
    if label.starts_with('≤') || label.starts_with("<=") {
        return label
            .trim_start_matches(['≤', '<', '='])
            .trim()
            .parse()
            .ok();
    }
    if label.starts_with('≥') || label.starts_with(">=") {
        return label
            .trim_start_matches(['≥', '>', '='])
            .trim()
            .parse()
            .ok();
    }
    let (lo, hi) = label.split_once('-')?;
    Some((lo.trim().parse::<f64>().ok()? + hi.trim().parse::<f64>().ok()?) / 2.0)
}

pub fn settle_positions(
    db: &Database,
    target_date: &str,
    winning_bin_label: &str,
    resolved_at: Option<&str>,
    event_slug: Option<&str>,
) -> Result<usize> {
    let settled_ts = resolved_at
        .map(ToString::to_string)
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let positions = db::get_open_positions(db)?;
    let mut closed = 0;
    for pos in positions {
        if pos.target_date != target_date {
            continue;
        }
        if let Some(event_slug) = event_slug {
            if pos.event_slug.as_deref() != Some(event_slug) {
                continue;
            }
        }
        let fee_rate = pos.fee_rate.unwrap_or(0.0);
        let cost = entry_cost(pos.entry_price, fee_rate);
        let pnl = if pos.bin_label == winning_bin_label {
            -(pos.shares * cost)
        } else {
            pos.shares * (1.0 - cost)
        };
        if let Some(id) = pos.id {
            db::close_position(db, id, (pnl * 10_000.0).round() / 10_000.0, &settled_ts)?;
            closed += 1;
        }
    }
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::models::WeatherContract;

    #[test]
    fn parses_bin_midpoints_like_python() {
        assert_eq!(parse_bin_midpoint("38-39°F"), Some(38.5));
        assert_eq!(parse_bin_midpoint("≤27°F"), Some(27.0));
        assert_eq!(parse_bin_midpoint(">=46F"), Some(46.0));
    }

    #[test]
    fn extracts_gamma_settlement_from_yes_winner() {
        let event = json!({
            "id": "evt-1",
            "_slug": "highest-temperature-in-nyc-on-april-1-2026",
            "closedTime": "2026-04-02T00:00:00Z",
            "markets": [
                {"groupItemTitle": "70-71°F", "outcomePrices": "[\"0\", \"1\"]"},
                {"groupItemTitle": "71-72°F", "outcomePrices": "[\"1\", \"0\"]"}
            ]
        });
        let contract = WeatherContract {
            event_slug: "highest-temperature-in-nyc-on-april-1-2026".to_string(),
            event_id: Some("evt-1".to_string()),
            target_date: "2026-04-01".to_string(),
            market_type: "daily_temperature".to_string(),
            measure: "high".to_string(),
            unit: "F".to_string(),
            city_slug: "nyc".to_string(),
            display_city: "NYC".to_string(),
            resolution_source_kind: "wunderground".to_string(),
            station_id: "KLGA".to_string(),
            source_url: "https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA"
                .to_string(),
            timezone: "America/New_York".to_string(),
            lat: 0.0,
            lon: 0.0,
            validation_status: "valid".to_string(),
            skip_reason: None,
            raw_json: None,
        };
        let rec = extract_settlement(&event, &contract).unwrap();
        assert_eq!(rec.winning_bin, "71-72°F");
        assert_eq!(rec.settlement_temp_f, Some(71.5));
        assert_eq!(rec.resolved_at.unwrap(), "2026-04-02T00:00:00+00:00");
    }
}
