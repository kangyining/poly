use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use clap::Args;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::config;
use crate::db::{self, Database, NewSettlement};
use crate::strategy::utils::entry_cost;

#[derive(Debug, Clone, Args)]
pub struct SettleArgs {
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long)]
    pub backfill: Option<i64>,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value = "open_positions.json")]
    pub positions: PathBuf,
}

pub fn run(args: SettleArgs) -> Result<()> {
    let db_path = args
        .db
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_DB_PATH));
    let db = Database::open(&db_path, false)?;
    let mut saved = 0;
    let mut closed = 0;
    let dates = if let Some(days) = args.backfill {
        let today = Utc::now().date_naive();
        (1..=days.max(0))
            .rev()
            .map(|offset| today - Duration::days(offset))
            .collect::<Vec<_>>()
    } else {
        vec![args
            .date
            .as_deref()
            .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
            .transpose()?
            .unwrap_or_else(|| Utc::now().date_naive() - Duration::days(1))]
    };

    for target in dates {
        for cfg in config::enabled_contracts() {
            if db::settlement_exists(&db, cfg.display_city, &target.to_string())? {
                continue;
            }
            let slug = config::build_daily_high_slug(cfg.slug, target);
            let Some(event) = fetch_event_by_slug(&slug)? else {
                continue;
            };
            let Some(win_bin) = winning_bin(&event) else {
                continue;
            };
            let resolved_at = event
                .get("closedTime")
                .or_else(|| event.get("updatedAt"))
                .and_then(Value::as_str)
                .and_then(crate::db::parse_ts)
                .map(|dt| dt.to_rfc3339());
            let settlement_temp = parse_bin_midpoint(&win_bin);
            let rec = NewSettlement {
                city: cfg.display_city.to_string(),
                target_date: target.to_string(),
                event_id: event.get("id").map(|v| v.to_string()).unwrap_or_default(),
                event_slug: slug.clone(),
                winning_bin: win_bin.clone(),
                settlement_temp_f: settlement_temp,
                wunderground_station: cfg.station_id.to_string(),
                resolved_at: resolved_at.clone(),
                raw_outcome_prices: outcome_snapshot(&event).to_string(),
                market_type: Some("daily_temperature".to_string()),
                measure: Some("high".to_string()),
                unit: Some("F".to_string()),
                city_slug: Some(cfg.slug.to_string()),
                station_id: Some(cfg.station_id.to_string()),
                source_url: Some(cfg.source_url.to_string()),
                settlement_value: settlement_temp,
                settlement_unit: Some("F".to_string()),
                settlement_temp_source: Some("polymarket-gamma-outcome".to_string()),
                settlement_temp_station: Some(cfg.station_id.to_string()),
                settlement_temp_raw_json: Some(
                    json!({
                        "source": "polymarket-gamma-outcome",
                        "event_slug": slug,
                        "winning_bin": win_bin,
                        "settlement_temp_f": settlement_temp,
                    })
                    .to_string(),
                ),
            };
            db::insert_settlement(&db, &rec)?;
            saved += 1;
            closed += settle_positions(
                &db,
                &target.to_string(),
                &win_bin,
                resolved_at.as_deref(),
                Some(&slug),
            )?;
        }
    }
    db::export_open_positions_json(&db, &args.positions)?;
    println!(
        "{}",
        json!({"settlements_saved": saved, "positions_closed": closed, "positions": args.positions})
    );
    Ok(())
}

fn fetch_event_by_slug(slug: &str) -> Result<Option<Value>> {
    let response: Value = reqwest::blocking::Client::new()
        .get("https://gamma-api.polymarket.com/events")
        .query(&[("slug", slug)])
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .with_context(|| format!("failed to fetch event {slug}"))?
        .error_for_status()?
        .json()?;
    Ok(response.as_array().and_then(|items| items.first()).cloned())
}

fn winning_bin(event: &Value) -> Option<String> {
    for market in event.get("markets")?.as_array()? {
        let raw = market.get("outcomePrices")?;
        let prices = if let Some(s) = raw.as_str() {
            serde_json::from_str::<Vec<Value>>(s).ok()?
        } else {
            raw.as_array()?.clone()
        };
        if prices
            .first()
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0.0)
            >= 0.99
        {
            return market
                .get("groupItemTitle")?
                .as_str()
                .map(ToString::to_string);
        }
    }
    None
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

fn parse_bin_midpoint(bin_label: &str) -> Option<f64> {
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

fn settle_positions(
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

pub fn settle_positions_for_test(
    db: &Database,
    target_date: &str,
    winning_bin_label: &str,
    resolved_at: Option<&str>,
    event_slug: Option<&str>,
) -> Result<usize> {
    settle_positions(db, target_date, winning_bin_label, resolved_at, event_slug)
}
