use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use std::collections::BTreeMap;

use crate::analysis::backtest::{market_close_utc, market_open_utc, TRADING_WINDOW_HOURS};
use crate::config;
use crate::db::{self, Database};

#[derive(Debug, Clone)]
pub struct ReplayDataset {
    pub days: Vec<ReplayDay>,
}

#[derive(Debug, Clone)]
pub struct ReplayDay {
    pub settlement: db::SettlementRow,
    pub event_slug: String,
    pub target_dt: NaiveDate,
    pub fallback_settled_at: DateTime<Utc>,
    pub resolved_at: DateTime<Utc>,
    pub station: String,
    pub forecasts: Vec<db::ForecastSnapshot>,
    pub price_bars: BTreeMap<DateTime<Utc>, PriceBarBucket>,
    pub snapshot_fills: BTreeMap<(DateTime<Utc>, String), SnapshotFillRaw>,
    pub metar: MetarPrefix,
    pub climatology: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct PriceBarBucket {
    order: Vec<String>,
    bars: BTreeMap<String, db::PriceBar>,
}

impl PriceBarBucket {
    pub fn insert(&mut self, row: db::PriceBar) {
        if !self.bars.contains_key(&row.bin_label) {
            self.order.push(row.bin_label.clone());
        }
        self.bars.insert(row.bin_label.clone(), row);
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn reference_market(&self) -> Option<&db::PriceBar> {
        self.order.first().and_then(|label| self.bars.get(label))
    }

    pub fn sorted_markets(&self) -> Vec<&db::PriceBar> {
        let mut markets = self.bars.values().collect::<Vec<_>>();
        markets.sort_by_key(|market| market.bin_index);
        markets
    }

    pub fn bin_index(&self, bin_label: &str) -> i64 {
        self.bars
            .get(bin_label)
            .map(|bar| bar.bin_index)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotFillRaw {
    pub hours_to_close: f64,
    pub fee_rate: Option<f64>,
    pub ask_levels: Vec<(f64, f64)>,
    pub no_vwap_10: Option<f64>,
    pub no_best_bid: Option<f64>,
    pub no_best_ask: Option<f64>,
}

impl SnapshotFillRaw {
    pub fn to_fill(&self, shares: f64) -> Option<db::SnapshotFill> {
        let mut effective_ask = effective_ask_from_levels(&self.ask_levels, shares);
        if effective_ask.is_none() && (shares - 10.0).abs() <= f64::EPSILON {
            effective_ask = self.no_vwap_10;
        }
        if effective_ask.is_none() {
            effective_ask = self.no_best_ask;
        }
        let effective_ask = effective_ask?;
        Some(db::SnapshotFill {
            effective_ask: effective_ask.clamp(0.0, 1.0),
            fee_rate: self.fee_rate,
            hours_to_close: self.hours_to_close,
            no_best_bid: self.no_best_bid,
            no_best_ask: self.no_best_ask,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct MetarPrefix {
    times: Vec<DateTime<Utc>>,
    maxes: Vec<f64>,
}

impl MetarPrefix {
    pub fn from_records(mut records: Vec<(DateTime<Utc>, f64)>) -> Self {
        records.sort_by_key(|(ts, _)| *ts);
        let mut times = Vec::with_capacity(records.len());
        let mut maxes = Vec::with_capacity(records.len());
        let mut current: Option<f64> = None;
        for (ts, temp) in records {
            current = Some(current.map(|v| v.max(temp)).unwrap_or(temp));
            times.push(ts);
            maxes.push(current.unwrap_or(temp));
        }
        Self { times, maxes }
    }

    pub fn max_before(&self, evaluation_time: DateTime<Utc>) -> Option<f64> {
        let idx = self.times.partition_point(|ts| *ts <= evaluation_time);
        if idx == 0 {
            None
        } else {
            self.maxes.get(idx - 1).copied()
        }
    }
}

impl ReplayDataset {
    pub fn load(
        db: &Database,
        date_from: NaiveDate,
        date_to: NaiveDate,
        city: Option<&str>,
        interval_minutes: i64,
    ) -> Result<Self> {
        let date_from_s = date_from.to_string();
        let date_to_s = date_to.to_string();
        let settlements = db::load_settlements(db, &date_from_s, &date_to_s, city)?;
        let mut days = Vec::with_capacity(settlements.len());

        let forecasts = load_forecasts(db, &date_from_s, &date_to_s, city)?;
        let prices = if city.is_some() {
            load_price_bars(db, &date_from_s, &date_to_s, city, interval_minutes)?
        } else {
            load_price_bars_by_settlement(db, &settlements, interval_minutes)?
        };
        let snapshots = if city.is_some() {
            load_snapshot_fills(db, &date_from_s, &date_to_s, city, interval_minutes)?
        } else {
            load_snapshot_fills_by_settlement(db, &settlements, interval_minutes)?
        };
        let metars = load_metars(db, &date_to_s, city)?;
        let climatology_source = load_climatology_source(db, city)?;

        for settlement in settlements {
            let settlement_city = settlement.city.clone();
            let target_date = settlement.target_date.clone();
            let target_dt = NaiveDate::parse_from_str(&target_date, "%Y-%m-%d")?;
            let event_slug = settlement
                .event_slug
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| config::default_event_slug(&settlement_city, &target_date));
            let fallback_settled_at = market_close_utc(&target_date)?;
            let resolved_at = settlement
                .resolved_at
                .as_deref()
                .and_then(db::parse_ts)
                .unwrap_or_else(|| fallback_settled_at + chrono::Duration::days(1));
            let key = (settlement_city.clone(), target_date.clone());
            days.push(ReplayDay {
                settlement,
                event_slug,
                target_dt,
                fallback_settled_at,
                resolved_at,
                station: config::station_for_city(&settlement_city),
                forecasts: forecasts.get(&key).cloned().unwrap_or_default(),
                price_bars: prices.get(&key).cloned().unwrap_or_default(),
                snapshot_fills: snapshots.get(&key).cloned().unwrap_or_default(),
                metar: MetarPrefix::from_records(
                    metars.get(&settlement_city).cloned().unwrap_or_default(),
                ),
                climatology: climatology_for(
                    climatology_source
                        .get(&settlement_city)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    &target_date,
                    14,
                ),
            });
        }

        Ok(Self { days })
    }
}

type DayKey = (String, String);

fn load_forecasts(
    db: &Database,
    date_from: &str,
    date_to: &str,
    city: Option<&str>,
) -> Result<BTreeMap<DayKey, Vec<db::ForecastSnapshot>>> {
    let sql = if city.is_some() {
        "SELECT city, target_date, source, temp_max_gfs_f, temp_max_ecmwf_f, model_spread_c,
                forecast_lead_hours, timestamp, raw_json
         FROM forecast_records
         WHERE target_date >= ?1 AND target_date <= ?2 AND city = ?3
           AND source IN ('open-meteo', 'gfs-global-24h', 'historical-open-meteo', 'open-meteo-historical')
         ORDER BY city, target_date, timestamp ASC"
    } else {
        "SELECT city, target_date, source, temp_max_gfs_f, temp_max_ecmwf_f, model_spread_c,
                forecast_lead_hours, timestamp, raw_json
         FROM forecast_records
         WHERE target_date >= ?1 AND target_date <= ?2
           AND source IN ('open-meteo', 'gfs-global-24h', 'historical-open-meteo', 'open-meteo-historical')
         ORDER BY city, target_date, timestamp ASC"
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = city {
        stmt.query_map((date_from, date_to, city), forecast_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map((date_from, date_to), forecast_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = BTreeMap::<DayKey, Vec<db::ForecastSnapshot>>::new();
    for row in rows {
        let (
            city,
            target_date,
            source,
            forecast_f,
            gfs_f,
            ecmwf_f,
            model_spread_c,
            forecast_lead_hours,
            timestamp,
            raw_json,
        ) = row;
        let Some(forecast_f) = forecast_f else {
            continue;
        };
        let replay_open = market_open_utc(&target_date)?;
        let (available_at, priority) = match source.as_str() {
            "gfs-global-24h" => (
                db::parse_gfs_model_run(raw_json.as_deref())
                    .or_else(|| db::parse_ts(&timestamp))
                    .unwrap_or(replay_open),
                0,
            ),
            "open-meteo" => (db::parse_ts(&timestamp).unwrap_or(replay_open), 1),
            _ => (replay_open, 2),
        };
        out.entry((city, target_date))
            .or_default()
            .push(db::ForecastSnapshot {
                source,
                forecast_f,
                gfs_f,
                ecmwf_f,
                model_spread_c,
                forecast_lead_hours,
                available_at,
                priority,
            });
    }
    Ok(out)
}

type ForecastRow = (
    String,
    String,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    String,
    Option<String>,
);

fn forecast_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ForecastRow> {
    let gfs_f: Option<f64> = row.get("temp_max_gfs_f")?;
    let ecmwf_f: Option<f64> = row.get("temp_max_ecmwf_f")?;
    Ok((
        row.get("city")?,
        row.get("target_date")?,
        row.get("source")?,
        gfs_f.or(ecmwf_f),
        gfs_f,
        ecmwf_f,
        row.get("model_spread_c")?,
        row.get("forecast_lead_hours")?,
        row.get("timestamp")?,
        row.get("raw_json")?,
    ))
}

fn load_price_bars(
    db: &Database,
    date_from: &str,
    date_to: &str,
    city: Option<&str>,
    interval_minutes: i64,
) -> Result<BTreeMap<DayKey, BTreeMap<DateTime<Utc>, PriceBarBucket>>> {
    let sql = if city.is_some() {
        "SELECT city, target_date, bin_label, bin_index, token_id_yes, timestamp, yes_price, hours_to_close
         FROM price_history_1m
         WHERE target_date >= ?1 AND target_date <= ?2 AND city = ?3
           AND hours_to_close > 0 AND hours_to_close <= ?4
         ORDER BY city, target_date, timestamp ASC, bin_index ASC"
    } else {
        "SELECT city, target_date, bin_label, bin_index, token_id_yes, timestamp, yes_price, hours_to_close
         FROM price_history_1m
         WHERE target_date >= ?1 AND target_date <= ?2
           AND hours_to_close > 0 AND hours_to_close <= ?3
         ORDER BY city, target_date, timestamp ASC, bin_index ASC"
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = city {
        stmt.query_map((date_from, date_to, city, TRADING_WINDOW_HOURS), price_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map((date_from, date_to, TRADING_WINDOW_HOURS), price_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = BTreeMap::<DayKey, BTreeMap<DateTime<Utc>, PriceBarBucket>>::new();
    for (city, target_date, bar) in rows {
        let bucket = floor_timestamp(bar.timestamp, interval_minutes);
        out.entry((city, target_date))
            .or_default()
            .entry(bucket)
            .or_default()
            .insert(bar);
    }
    Ok(out)
}

fn load_price_bars_by_settlement(
    db: &Database,
    settlements: &[db::SettlementRow],
    interval_minutes: i64,
) -> Result<BTreeMap<DayKey, BTreeMap<DateTime<Utc>, PriceBarBucket>>> {
    let mut out = BTreeMap::new();
    for settlement in settlements {
        let rows = db::load_price_bars(
            db,
            &settlement.city,
            &settlement.target_date,
            TRADING_WINDOW_HOURS,
        )?;
        let mut grouped = BTreeMap::<DateTime<Utc>, PriceBarBucket>::new();
        for row in rows {
            let bucket = floor_timestamp(row.timestamp, interval_minutes);
            grouped.entry(bucket).or_default().insert(row);
        }
        out.insert(
            (settlement.city.clone(), settlement.target_date.clone()),
            grouped,
        );
    }
    Ok(out)
}

fn price_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, db::PriceBar)> {
    let timestamp: String = row.get("timestamp")?;
    Ok((
        row.get("city")?,
        row.get("target_date")?,
        db::PriceBar {
            bin_label: row.get("bin_label")?,
            bin_index: row.get::<_, Option<i64>>("bin_index")?.unwrap_or(0),
            token_id_yes: row.get("token_id_yes")?,
            timestamp: db::parse_ts(&timestamp).unwrap_or_else(Utc::now),
            yes_price: row.get("yes_price")?,
            hours_to_close: row.get("hours_to_close")?,
        },
    ))
}

fn load_snapshot_fills(
    db: &Database,
    date_from: &str,
    date_to: &str,
    city: Option<&str>,
    interval_minutes: i64,
) -> Result<BTreeMap<DayKey, BTreeMap<(DateTime<Utc>, String), SnapshotFillRaw>>> {
    let sql = if city.is_some() {
        "SELECT city, target_date, timestamp, bin_label, hours_to_close, no_asks_json, fee_rate,
                no_vwap_10, no_best_bid, no_best_ask
         FROM market_snapshots
         WHERE target_date >= ?1 AND target_date <= ?2 AND city = ?3
           AND (no_asks_json IS NOT NULL OR no_vwap_10 IS NOT NULL OR no_best_ask IS NOT NULL)
         ORDER BY city, target_date, timestamp ASC"
    } else {
        "SELECT city, target_date, timestamp, bin_label, hours_to_close, no_asks_json, fee_rate,
                no_vwap_10, no_best_bid, no_best_ask
         FROM market_snapshots
         WHERE target_date >= ?1 AND target_date <= ?2
           AND (no_asks_json IS NOT NULL OR no_vwap_10 IS NOT NULL OR no_best_ask IS NOT NULL)
         ORDER BY city, target_date, timestamp ASC"
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = city {
        stmt.query_map((date_from, date_to, city), snapshot_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map((date_from, date_to), snapshot_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = BTreeMap::new();
    for (city, target_date, timestamp, bin_label, fill) in rows {
        let Some(ts) = db::parse_ts(&timestamp) else {
            continue;
        };
        let hours_to_close = fill.hours_to_close;
        if hours_to_close <= 0.0 || hours_to_close > TRADING_WINDOW_HOURS {
            continue;
        }
        let bucket = floor_timestamp(ts, interval_minutes);
        out.entry((city, target_date))
            .or_insert_with(BTreeMap::new)
            .insert((bucket, bin_label), fill);
    }
    Ok(out)
}

fn load_snapshot_fills_by_settlement(
    db: &Database,
    settlements: &[db::SettlementRow],
    interval_minutes: i64,
) -> Result<BTreeMap<DayKey, BTreeMap<(DateTime<Utc>, String), SnapshotFillRaw>>> {
    let mut out = BTreeMap::new();
    for settlement in settlements {
        let fills = load_snapshot_fills_one(
            db,
            &settlement.city,
            &settlement.target_date,
            interval_minutes,
        )?;
        out.insert(
            (settlement.city.clone(), settlement.target_date.clone()),
            fills,
        );
    }
    Ok(out)
}

fn load_snapshot_fills_one(
    db: &Database,
    city: &str,
    target_date: &str,
    interval_minutes: i64,
) -> Result<BTreeMap<(DateTime<Utc>, String), SnapshotFillRaw>> {
    let mut stmt = db.conn().prepare(
        "SELECT city, target_date, timestamp, bin_label, hours_to_close, no_asks_json, fee_rate,
                no_vwap_10, no_best_bid, no_best_ask
         FROM market_snapshots
         WHERE city = ?1 AND target_date = ?2
           AND (no_asks_json IS NOT NULL OR no_vwap_10 IS NOT NULL OR no_best_ask IS NOT NULL)
         ORDER BY timestamp ASC",
    )?;
    let rows = stmt
        .query_map((city, target_date), snapshot_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = BTreeMap::new();
    for (_city, _target_date, timestamp, bin_label, fill) in rows {
        let Some(ts) = db::parse_ts(&timestamp) else {
            continue;
        };
        if fill.hours_to_close <= 0.0 || fill.hours_to_close > TRADING_WINDOW_HOURS {
            continue;
        }
        out.insert((floor_timestamp(ts, interval_minutes), bin_label), fill);
    }
    Ok(out)
}

fn snapshot_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, SnapshotFillRaw)> {
    let target_date: String = row.get("target_date")?;
    let timestamp: String = row.get("timestamp")?;
    let ts = db::parse_ts(&timestamp);
    let hours_to_close: Option<f64> = row.get("hours_to_close")?;
    let hours_to_close = hours_to_close.unwrap_or_else(|| {
        ts.and_then(|ts| {
            market_close_utc(&target_date)
                .ok()
                .map(|close| (close - ts).num_seconds() as f64 / 3600.0)
        })
        .unwrap_or(0.0)
    });
    let no_asks_json: Option<String> = row.get("no_asks_json")?;
    Ok((
        row.get("city")?,
        target_date,
        timestamp,
        row.get("bin_label")?,
        SnapshotFillRaw {
            hours_to_close,
            fee_rate: row.get("fee_rate")?,
            ask_levels: no_asks_json
                .as_deref()
                .map(parse_order_levels)
                .unwrap_or_default(),
            no_vwap_10: row.get("no_vwap_10")?,
            no_best_bid: row.get("no_best_bid")?,
            no_best_ask: row.get("no_best_ask")?,
        },
    ))
}

fn load_metars(
    db: &Database,
    date_to: &str,
    city: Option<&str>,
) -> Result<BTreeMap<String, Vec<(DateTime<Utc>, f64)>>> {
    let sql = if city.is_some() {
        "SELECT city, obs_time, timestamp, temp_f
         FROM metar_records
         WHERE DATE(COALESCE(obs_time, timestamp)) <= ?1 AND city = ?2
         ORDER BY city, COALESCE(obs_time, timestamp)"
    } else {
        "SELECT city, obs_time, timestamp, temp_f
         FROM metar_records
         WHERE DATE(COALESCE(obs_time, timestamp)) <= ?1
         ORDER BY city, COALESCE(obs_time, timestamp)"
    };
    let mut out = BTreeMap::<String, Vec<(DateTime<Utc>, f64)>>::new();
    let mut stmt = match db.conn().prepare(sql) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(out),
    };
    let rows = if let Some(city) = city {
        stmt.query_map((date_to, city), metar_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([date_to], metar_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (city, ts, temp) in rows {
        if temp.is_finite() {
            out.entry(city).or_default().push((ts, temp));
        }
    }
    Ok(out)
}

fn metar_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, DateTime<Utc>, f64)> {
    let obs_time: Option<String> = row.get("obs_time")?;
    let timestamp: String = row.get("timestamp")?;
    let temp_f: Option<f64> = row.get("temp_f")?;
    Ok((
        row.get("city")?,
        obs_time
            .as_deref()
            .and_then(db::parse_ts)
            .or_else(|| db::parse_ts(&timestamp))
            .unwrap_or_else(Utc::now),
        temp_f.unwrap_or(f64::NAN),
    ))
}

fn load_climatology_source(
    db: &Database,
    city: Option<&str>,
) -> Result<BTreeMap<String, Vec<(String, f64)>>> {
    let sql = if city.is_some() {
        "SELECT city, target_date, settlement_temp_f
         FROM settlement_records
         WHERE city = ?1 AND settlement_temp_f IS NOT NULL
         ORDER BY city, target_date"
    } else {
        "SELECT city, target_date, settlement_temp_f
         FROM settlement_records
         WHERE settlement_temp_f IS NOT NULL
         ORDER BY city, target_date"
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = city {
        stmt.query_map([city], |row| {
            Ok((
                row.get::<_, String>("city")?,
                row.get::<_, String>("target_date")?,
                row.get::<_, f64>("settlement_temp_f")?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>("city")?,
                row.get::<_, String>("target_date")?,
                row.get::<_, f64>("settlement_temp_f")?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = BTreeMap::<String, Vec<(String, f64)>>::new();
    for (city, target_date, temp) in rows {
        out.entry(city).or_default().push((target_date, temp));
    }
    Ok(out)
}

fn climatology_for(source: &[(String, f64)], target_date: &str, window_days: i64) -> Vec<f64> {
    let Ok(target) = NaiveDate::parse_from_str(target_date, "%Y-%m-%d") else {
        return Vec::new();
    };
    let Some(pseudo_target) = NaiveDate::from_ymd_opt(2001, target.month(), target.day()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (other_date, temp) in source {
        if other_date == target_date {
            continue;
        }
        let Ok(other) = NaiveDate::parse_from_str(other_date, "%Y-%m-%d") else {
            continue;
        };
        let Some(pseudo_other) = NaiveDate::from_ymd_opt(2001, other.month(), other.day()) else {
            continue;
        };
        let delta = (pseudo_other - pseudo_target).num_days().abs();
        let delta = delta.min(365 - delta);
        if delta <= window_days {
            out.push(*temp);
        }
    }
    out
}

fn floor_timestamp(ts: DateTime<Utc>, interval_minutes: i64) -> DateTime<Utc> {
    let minute = (ts.minute() as i64 / interval_minutes) * interval_minutes;
    ts.with_minute(minute as u32)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

fn parse_order_levels(raw: &str) -> Vec<(f64, f64)> {
    let Ok(levels) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let mut out = levels
        .into_iter()
        .filter_map(|level| {
            let price = value_as_f64(level.get("price")?)?;
            let size = value_as_f64(level.get("size")?)?;
            Some((price, size))
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn effective_ask_from_levels(levels: &[(f64, f64)], shares: f64) -> Option<f64> {
    if levels.is_empty() || shares <= 0.0 {
        return None;
    }
    let mut remaining = shares;
    let mut cost = 0.0;
    for (price, size) in levels {
        let take = remaining.min(*size);
        cost += take * price;
        remaining -= take;
        if remaining <= 1e-9 {
            return Some(cost / shares);
        }
    }
    None
}

fn value_as_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}
