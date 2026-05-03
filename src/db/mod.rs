use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

use crate::data::models::{
    ForecastModelRun, ForecastRecord, MarketSnapshot, MetarRecord, NwsForecast, PriceHistoryRecord,
    TafRecord, WeatherContract,
};

const SCHEMA_SQL: &str = include_str!("../../db/schema.sql");

#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>, read_only: bool) -> Result<Self> {
        let path = path.as_ref();
        let conn = if read_only {
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let conn = Connection::open(path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            ensure_dynamic_columns(&conn)?;
            conn
        };
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

fn ensure_dynamic_columns(conn: &Connection) -> Result<()> {
    ensure_column(conn, "settlement_records", "settlement_temp_source", "TEXT")?;
    ensure_column(
        conn,
        "settlement_records",
        "settlement_temp_station",
        "TEXT",
    )?;
    ensure_column(
        conn,
        "settlement_records",
        "settlement_temp_raw_json",
        "TEXT",
    )?;
    Ok(())
}

pub fn ensure_column(conn: &Connection, table: &str, column: &str, sql_type: &str) -> Result<()> {
    if !is_safe_identifier(table) || !is_safe_identifier(column) {
        anyhow::bail!("unsafe SQLite identifier in ensure_column");
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {sql_type}"),
            [],
        )?;
    }
    Ok(())
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementRow {
    pub city: String,
    pub target_date: String,
    pub winning_bin: Option<String>,
    pub event_slug: Option<String>,
    pub settlement_temp_f: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastSnapshot {
    pub source: String,
    pub forecast_f: f64,
    pub gfs_f: Option<f64>,
    pub ecmwf_f: Option<f64>,
    pub model_spread_c: Option<f64>,
    pub forecast_lead_hours: Option<f64>,
    pub available_at: DateTime<Utc>,
    pub priority: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceBar {
    pub bin_label: String,
    pub bin_index: i64,
    pub token_id_yes: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub yes_price: f64,
    pub hours_to_close: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotFill {
    pub effective_ask: f64,
    pub fee_rate: Option<f64>,
    pub hours_to_close: f64,
    pub no_best_bid: Option<f64>,
    pub no_best_ask: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionRow {
    pub id: Option<i64>,
    pub token_id: Option<String>,
    pub market_id: Option<String>,
    pub bin_label: String,
    pub event_slug: Option<String>,
    pub target_date: String,
    pub entry_price: f64,
    pub fee_rate: Option<f64>,
    pub shares: f64,
    pub stake_usd: f64,
    pub order_id: Option<String>,
    pub pnl: Option<f64>,
    pub status: String,
    pub is_paper: bool,
    pub opened_at: String,
    pub settled_at: Option<String>,
    pub strategy_name: Option<String>,
    pub trade_rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPosition {
    pub token_id: String,
    pub market_id: String,
    pub bin_label: String,
    pub event_slug: String,
    pub target_date: String,
    pub entry_price: f64,
    pub fee_rate: Option<f64>,
    pub shares: f64,
    pub stake_usd: f64,
    pub order_id: Option<String>,
    pub status: String,
    pub pnl: Option<f64>,
    pub opened_at: String,
    pub settled_at: Option<String>,
    pub is_paper: bool,
    pub strategy_name: String,
    pub backtest_id: Option<String>,
    pub trade_rationale: Option<String>,
}

pub fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M%:z")
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
                .ok()
                .map(|dt| dt.and_utc())
        })
}

pub fn parse_gfs_model_run(raw_json: Option<&str>) -> Option<DateTime<Utc>> {
    let payload: Value = serde_json::from_str(raw_json?).ok()?;
    let mut model_run = payload.get("past_model_run")?.as_str()?.to_string();
    if let Some(stripped) = model_run.strip_suffix('Z') {
        model_run = format!("{stripped}+00:00");
    } else if !model_run.contains('+') {
        model_run.push_str("+00:00");
    }
    parse_ts(&model_run)
}

fn settlement_from_row(row: &Row<'_>) -> rusqlite::Result<SettlementRow> {
    Ok(SettlementRow {
        city: row.get("city")?,
        target_date: row.get("target_date")?,
        winning_bin: row.get("winning_bin")?,
        event_slug: row.get("event_slug")?,
        settlement_temp_f: row.get("settlement_temp_f")?,
    })
}

pub fn load_settlements(
    db: &Database,
    date_from: &str,
    date_to: &str,
    city: Option<&str>,
) -> Result<Vec<SettlementRow>> {
    let sql = if city.is_some() {
        "SELECT city, target_date, winning_bin, event_slug, settlement_temp_f
         FROM settlement_records
         WHERE target_date >= ?1 AND target_date <= ?2 AND city = ?3
         ORDER BY target_date, city"
    } else {
        "SELECT city, target_date, winning_bin, event_slug, settlement_temp_f
         FROM settlement_records
         WHERE target_date >= ?1 AND target_date <= ?2
         ORDER BY target_date, city"
    };
    let mut stmt = db.conn().prepare(sql)?;
    let rows = if let Some(city) = city {
        stmt.query_map((date_from, date_to, city), settlement_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map((date_from, date_to), settlement_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

pub fn load_forecast_snapshots(
    db: &Database,
    city: &str,
    target_date: &str,
    replay_open: DateTime<Utc>,
) -> Result<Vec<ForecastSnapshot>> {
    let mut stmt = db.conn().prepare(
        "SELECT source, temp_max_gfs_f, temp_max_ecmwf_f, model_spread_c,
                forecast_lead_hours, timestamp, raw_json
         FROM forecast_records
         WHERE city = ?1 AND target_date = ?2
           AND source IN ('open-meteo', 'gfs-global-24h', 'historical-open-meteo', 'open-meteo-historical')
         ORDER BY timestamp ASC",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map((city, target_date), |row| {
        let source: String = row.get("source")?;
        let gfs: Option<f64> = row.get("temp_max_gfs_f")?;
        let ecmwf: Option<f64> = row.get("temp_max_ecmwf_f")?;
        let timestamp: String = row.get("timestamp")?;
        let forecast_f = gfs.or(ecmwf);
        Ok((
            source,
            forecast_f,
            gfs,
            ecmwf,
            row.get::<_, Option<f64>>("model_spread_c")?,
            row.get::<_, Option<f64>>("forecast_lead_hours")?,
            timestamp,
            row.get::<_, Option<String>>("raw_json")?,
        ))
    })? {
        let (
            source,
            forecast_f,
            gfs_f,
            ecmwf_f,
            model_spread_c,
            forecast_lead_hours,
            timestamp,
            raw_json,
        ) = row?;
        let Some(forecast_f) = forecast_f else {
            continue;
        };
        let (available_at, priority) = match source.as_str() {
            "gfs-global-24h" => (
                parse_gfs_model_run(raw_json.as_deref())
                    .or_else(|| parse_ts(&timestamp))
                    .unwrap_or(replay_open),
                0,
            ),
            "open-meteo" => (parse_ts(&timestamp).unwrap_or(replay_open), 1),
            _ => (replay_open, 2),
        };
        out.push(ForecastSnapshot {
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

#[derive(Debug, Deserialize)]
struct OrderLevel {
    price: Value,
    size: Value,
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

pub fn effective_ask_from_levels(levels_json: &str, shares: f64) -> Option<f64> {
    let mut levels = serde_json::from_str::<Vec<OrderLevel>>(levels_json)
        .ok()?
        .into_iter()
        .filter_map(|level| Some((value_as_f64(&level.price)?, value_as_f64(&level.size)?)))
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

pub fn load_snapshot_fill_bars(
    db: &Database,
    city: &str,
    target_date: &str,
    interval_minutes: i64,
    shares: f64,
    trading_window_hours: f64,
) -> Result<BTreeMap<(DateTime<Utc>, String), SnapshotFill>> {
    let mut stmt = db.conn().prepare(
        "SELECT timestamp, bin_label, hours_to_close, no_asks_json, fee_rate,
                no_vwap_10, no_best_bid, no_best_ask
         FROM market_snapshots
         WHERE city = ?1 AND target_date = ?2
           AND (no_asks_json IS NOT NULL OR no_vwap_10 IS NOT NULL OR no_best_ask IS NOT NULL)
         ORDER BY timestamp ASC",
    )?;
    let mut out = BTreeMap::new();
    for row in stmt.query_map((city, target_date), |row| {
        Ok((
            row.get::<_, String>("timestamp")?,
            row.get::<_, String>("bin_label")?,
            row.get::<_, Option<f64>>("hours_to_close")?,
            row.get::<_, Option<String>>("no_asks_json")?,
            row.get::<_, Option<f64>>("fee_rate")?,
            row.get::<_, Option<f64>>("no_vwap_10")?,
            row.get::<_, Option<f64>>("no_best_bid")?,
            row.get::<_, Option<f64>>("no_best_ask")?,
        ))
    })? {
        let (
            timestamp,
            bin_label,
            hours_to_close,
            no_asks_json,
            fee_rate,
            no_vwap_10,
            no_best_bid,
            no_best_ask,
        ) = row?;
        let Some(ts) = parse_ts(&timestamp) else {
            continue;
        };
        let hours_to_close = hours_to_close.unwrap_or_else(|| {
            crate::analysis::backtest::market_close_utc(target_date)
                .map(|close| (close - ts).num_seconds() as f64 / 3600.0)
                .unwrap_or(0.0)
        });
        if hours_to_close <= 0.0 || hours_to_close > trading_window_hours {
            continue;
        }
        let mut effective_ask = no_asks_json
            .as_deref()
            .and_then(|levels| effective_ask_from_levels(levels, shares));
        if effective_ask.is_none() && (shares - 10.0).abs() <= f64::EPSILON {
            effective_ask = no_vwap_10;
        }
        if effective_ask.is_none() {
            effective_ask = no_best_ask;
        }
        let Some(effective_ask) = effective_ask else {
            continue;
        };
        let bucket = floor_timestamp(ts, interval_minutes);
        out.insert(
            (bucket, bin_label),
            SnapshotFill {
                effective_ask: effective_ask.clamp(0.0, 1.0),
                fee_rate,
                hours_to_close,
                no_best_bid,
                no_best_ask,
            },
        );
    }
    Ok(out)
}

fn floor_timestamp(ts: DateTime<Utc>, interval_minutes: i64) -> DateTime<Utc> {
    use chrono::Timelike;
    let minute = (ts.minute() as i64 / interval_minutes) * interval_minutes;
    ts.with_minute(minute as u32)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

pub fn load_price_bars(
    db: &Database,
    city: &str,
    target_date: &str,
    trading_window_hours: f64,
) -> Result<Vec<PriceBar>> {
    let mut stmt = db.conn().prepare(
        "SELECT bin_label, bin_index, token_id_yes, timestamp, yes_price, hours_to_close
         FROM price_history_1m
         WHERE city = ?1 AND target_date = ?2
           AND hours_to_close > 0 AND hours_to_close <= ?3
         ORDER BY timestamp ASC, bin_index ASC",
    )?;
    let rows = stmt.query_map((city, target_date, trading_window_hours), |row| {
        let timestamp: String = row.get("timestamp")?;
        Ok((
            row.get::<_, String>("bin_label")?,
            row.get::<_, Option<i64>>("bin_index")?.unwrap_or(0),
            row.get::<_, Option<String>>("token_id_yes")?,
            timestamp,
            row.get::<_, f64>("yes_price")?,
            row.get::<_, f64>("hours_to_close")?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (bin_label, bin_index, token_id_yes, timestamp, yes_price, hours_to_close) = row?;
        if let Some(timestamp) = parse_ts(&timestamp) {
            out.push(PriceBar {
                bin_label,
                bin_index,
                token_id_yes,
                timestamp,
                yes_price,
                hours_to_close,
            });
        }
    }
    Ok(out)
}

pub fn load_metar_records(
    db: &Database,
    city: &str,
    target_date: &str,
) -> Result<Vec<(DateTime<Utc>, f64)>> {
    let mut stmt = db.conn().prepare(
        "SELECT obs_time, timestamp, temp_f
         FROM metar_records
         WHERE city = ?1 AND DATE(COALESCE(obs_time, timestamp)) <= ?2
         ORDER BY COALESCE(obs_time, timestamp)",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map((city, target_date), |row| {
        Ok((
            row.get::<_, Option<String>>("obs_time")?,
            row.get::<_, String>("timestamp")?,
            row.get::<_, Option<f64>>("temp_f")?,
        ))
    })? {
        let (obs_time, timestamp, temp_f) = row?;
        let Some(temp_f) = temp_f else { continue };
        let ts = obs_time
            .as_deref()
            .and_then(parse_ts)
            .or_else(|| parse_ts(&timestamp));
        if let Some(ts) = ts {
            out.push((ts, temp_f));
        }
    }
    Ok(out)
}

pub fn load_climatology_temps(
    db: &Database,
    city: &str,
    target_date: &str,
    window_days: i64,
) -> Result<Vec<f64>> {
    use chrono::NaiveDate;
    let target = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")?;
    let pseudo_target = NaiveDate::from_ymd_opt(2001, target.month(), target.day()).unwrap();
    let mut stmt = db.conn().prepare(
        "SELECT target_date, settlement_temp_f
         FROM settlement_records
         WHERE city = ?1 AND settlement_temp_f IS NOT NULL AND target_date != ?2",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map((city, target_date), |row| {
        Ok((
            row.get::<_, String>("target_date")?,
            row.get::<_, f64>("settlement_temp_f")?,
        ))
    })? {
        let (other_date, temp) = row?;
        let Ok(other) = NaiveDate::parse_from_str(&other_date, "%Y-%m-%d") else {
            continue;
        };
        let Some(pseudo_other) = NaiveDate::from_ymd_opt(2001, other.month(), other.day()) else {
            continue;
        };
        let delta = (pseudo_other - pseudo_target).num_days().abs();
        let delta = delta.min(365 - delta);
        if delta <= window_days {
            out.push(temp);
        }
    }
    Ok(out)
}

pub fn load_positions(
    db: &Database,
    status: Option<&str>,
    date: Option<&str>,
) -> Result<Vec<PositionRow>> {
    let mut sql = "SELECT id, token_id, market_id, bin_label, event_slug, target_date,
                          entry_price, fee_rate, shares, stake_usd, order_id, pnl,
                          status, is_paper, opened_at, settled_at, strategy_name, trade_rationale
                   FROM positions"
        .to_string();
    let mut clauses = Vec::new();
    if status.is_some() {
        clauses.push("status = ?");
    }
    if date.is_some() {
        clauses.push("target_date = ?");
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY opened_at");
    let mut stmt = db.conn().prepare(&sql)?;
    let map_row = |row: &Row<'_>| -> rusqlite::Result<PositionRow> {
        Ok(PositionRow {
            id: row.get(0)?,
            token_id: row.get(1)?,
            market_id: row.get(2)?,
            bin_label: row.get(3)?,
            event_slug: row.get(4)?,
            target_date: row.get(5)?,
            entry_price: row.get(6)?,
            fee_rate: row.get(7)?,
            shares: row.get(8)?,
            stake_usd: row.get(9)?,
            order_id: row.get(10)?,
            pnl: row.get(11)?,
            status: row.get(12)?,
            is_paper: row.get::<_, i64>(13)? != 0,
            opened_at: row.get(14)?,
            settled_at: row.get(15)?,
            strategy_name: row.get(16)?,
            trade_rationale: row.get(17)?,
        })
    };
    let rows = match (status, date) {
        (Some(status), Some(date)) => stmt
            .query_map((status, date), map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        (Some(status), None) => stmt
            .query_map([status], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        (None, Some(date)) => stmt
            .query_map([date], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        (None, None) => stmt
            .query_map([], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };
    Ok(rows)
}

pub fn upsert_weather_contract(db: &Database, contract: &WeatherContract) -> Result<()> {
    db.conn().execute(
        "INSERT INTO weather_contracts (
            event_slug, event_id, target_date, market_type, measure, unit,
            city_slug, display_city, resolution_source_kind, station_id,
            source_url, timezone, lat, lon, validation_status, skip_reason,
            discovered_at, raw_json
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
        ON CONFLICT(event_slug) DO UPDATE SET
            event_id=excluded.event_id,
            target_date=excluded.target_date,
            market_type=excluded.market_type,
            measure=excluded.measure,
            unit=excluded.unit,
            city_slug=excluded.city_slug,
            display_city=excluded.display_city,
            resolution_source_kind=excluded.resolution_source_kind,
            station_id=excluded.station_id,
            source_url=excluded.source_url,
            timezone=excluded.timezone,
            lat=excluded.lat,
            lon=excluded.lon,
            validation_status=excluded.validation_status,
            skip_reason=excluded.skip_reason,
            discovered_at=excluded.discovered_at,
            raw_json=excluded.raw_json",
        params![
            contract.event_slug,
            contract.event_id,
            contract.target_date,
            contract.market_type,
            contract.measure,
            contract.unit,
            contract.city_slug,
            contract.display_city,
            contract.resolution_source_kind,
            contract.station_id,
            contract.source_url,
            contract.timezone,
            contract.lat,
            contract.lon,
            contract.validation_status,
            contract.skip_reason,
            chrono::Utc::now().to_rfc3339(),
            contract.raw_json,
        ],
    )?;
    Ok(())
}

pub fn insert_market_snapshot(db: &Database, snapshot: &MarketSnapshot) -> Result<()> {
    db.conn().execute(
        "INSERT INTO market_snapshots (
            timestamp, city, target_date, hours_to_close, event_id, event_slug,
            bin_label, bin_index, token_id_yes, token_id_no,
            yes_price, no_price,
            yes_best_bid, yes_best_ask, no_best_bid, no_best_ask,
            book_depth_bid, book_depth_ask, no_asks_json, fee_rate, no_vwap_10,
            volume_24h, liquidity,
            market_type, measure, unit, city_slug, station_id, source_url,
            validation_status, skip_reason
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31)",
        params![
            snapshot.timestamp.to_rfc3339(),
            snapshot.city,
            snapshot.target_date,
            snapshot.hours_to_close,
            snapshot.event_id,
            snapshot.event_slug,
            snapshot.bin_label,
            snapshot.bin_index,
            snapshot.token_id_yes,
            snapshot.token_id_no,
            snapshot.yes_price,
            snapshot.no_price,
            snapshot.yes_best_bid,
            snapshot.yes_best_ask,
            snapshot.no_best_bid,
            snapshot.no_best_ask,
            snapshot.book_depth_bid,
            snapshot.book_depth_ask,
            snapshot.no_asks_json,
            snapshot.fee_rate,
            snapshot.no_vwap_10,
            snapshot.volume_24h,
            snapshot.liquidity,
            snapshot.market_type,
            snapshot.measure,
            snapshot.unit,
            snapshot.city_slug,
            snapshot.station_id,
            snapshot.source_url,
            snapshot.validation_status,
            snapshot.skip_reason,
        ],
    )?;
    Ok(())
}

pub fn insert_metar(db: &Database, metar: &MetarRecord) -> Result<()> {
    db.conn().execute(
        "INSERT INTO metar_records (
            timestamp, icao, city, temp_c, temp_f,
            dewpoint_c, obs_time, raw_metar
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            metar.timestamp.to_rfc3339(),
            metar.icao,
            metar.city,
            metar.temp_c,
            metar.temp_f,
            metar.dewpoint_c,
            metar.obs_time.to_rfc3339(),
            metar.raw_metar,
        ],
    )?;
    Ok(())
}

pub fn insert_taf(db: &Database, taf: &TafRecord) -> Result<()> {
    db.conn().execute(
        "INSERT INTO taf_records (
            timestamp, icao, city, issue_time, valid_from, valid_to, raw_taf
        ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            taf.timestamp.to_rfc3339(),
            taf.icao,
            taf.city,
            taf.issue_time,
            taf.valid_from,
            taf.valid_to,
            taf.raw_taf,
        ],
    )?;
    Ok(())
}

pub fn insert_nws_forecast(db: &Database, forecast: &NwsForecast) -> Result<()> {
    db.conn().execute(
        "INSERT INTO nws_forecasts (
            timestamp, city, target_date, period_name, is_daytime,
            temp_f, short_forecast, forecast_lead_hours
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            forecast.timestamp.to_rfc3339(),
            forecast.city,
            forecast.target_date,
            forecast.period_name,
            forecast.is_daytime.map(|v| if v { 1 } else { 0 }),
            forecast.temp_f,
            forecast.short_forecast,
            forecast.forecast_lead_hours,
        ],
    )?;
    Ok(())
}

pub fn insert_forecast(db: &Database, forecast: &ForecastRecord) -> Result<()> {
    db.conn().execute(
        "INSERT INTO forecast_records (
            timestamp, city, target_date, source,
            temp_max_ecmwf_c, temp_max_gfs_c,
            temp_max_ecmwf_f, temp_max_gfs_f,
            model_spread_c, forecast_lead_hours, raw_json,
            market_type, measure, unit, city_slug, station_id, event_slug,
            value_ecmwf, value_gfs, value_unit
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        params![
            forecast.timestamp.to_rfc3339(),
            forecast.city,
            forecast.target_date,
            forecast.source,
            forecast.temp_max_ecmwf_c,
            forecast.temp_max_gfs_c,
            forecast.temp_max_ecmwf_f,
            forecast.temp_max_gfs_f,
            forecast.model_spread_c,
            forecast.forecast_lead_hours,
            forecast.raw_json,
            forecast.market_type,
            forecast.measure,
            forecast.unit,
            forecast.city_slug,
            forecast.station_id,
            forecast.event_slug,
            forecast.value_ecmwf,
            forecast.value_gfs,
            forecast.value_unit,
        ],
    )?;
    Ok(())
}

pub fn insert_price_history(db: &Database, record: &PriceHistoryRecord) -> Result<()> {
    let table = if record.minute {
        "price_history_1m"
    } else {
        "price_history"
    };
    let sql = format!(
        "INSERT OR REPLACE INTO {table} (
            city, target_date, bin_label, bin_index, token_id_yes,
            timestamp, yes_price, hours_to_close, source
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"
    );
    db.conn().execute(
        &sql,
        params![
            record.city,
            record.target_date,
            record.bin_label,
            record.bin_index,
            record.token_id_yes,
            record.timestamp.to_rfc3339(),
            record.yes_price,
            record.hours_to_close,
            record.source,
        ],
    )?;
    Ok(())
}

pub fn insert_forecast_model_run(db: &Database, run: &ForecastModelRun) -> Result<i64> {
    db.conn().execute(
        "INSERT INTO forecast_model_runs (
            timestamp, city, target_date, source, model_name, model_run_time,
            forecast_lead_hours, temp_max_f, raw_json,
            market_type, measure, unit, city_slug, station_id, event_slug,
            value, value_unit
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            run.timestamp.to_rfc3339(),
            run.city,
            run.target_date,
            run.source,
            run.model_name,
            run.model_run_time,
            run.forecast_lead_hours,
            run.temp_max_f,
            run.raw_json,
            run.market_type,
            run.measure,
            run.unit,
            run.city_slug,
            run.station_id,
            run.event_slug,
            run.value,
            run.value_unit,
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

pub fn insert_position(db: &Database, pos: &NewPosition) -> Result<i64> {
    db.conn().execute(
        "INSERT OR IGNORE INTO positions (
            token_id, market_id, bin_label, event_slug, target_date,
            entry_price, fee_rate, shares, stake_usd, order_id,
            status, pnl, opened_at, settled_at, is_paper,
            strategy_name, backtest_id, trade_rationale
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
        params![
            pos.token_id,
            pos.market_id,
            pos.bin_label,
            pos.event_slug,
            pos.target_date,
            pos.entry_price,
            pos.fee_rate,
            pos.shares,
            pos.stake_usd,
            pos.order_id,
            pos.status,
            pos.pnl,
            pos.opened_at,
            pos.settled_at,
            if pos.is_paper { 1 } else { 0 },
            pos.strategy_name,
            pos.backtest_id,
            pos.trade_rationale,
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

pub fn get_open_positions(db: &Database) -> Result<Vec<PositionRow>> {
    load_positions(db, Some("open"), None)
}

pub fn close_position(db: &Database, position_id: i64, pnl: f64, settled_at: &str) -> Result<()> {
    db.conn().execute(
        "UPDATE positions SET status='closed', pnl=?1, settled_at=?2 WHERE id=?3",
        params![pnl, settled_at, position_id],
    )?;
    Ok(())
}

pub fn export_open_positions_json(db: &Database, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rows = get_open_positions(db)?;
    let out = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "token_id": r.token_id,
                "market_id": r.market_id,
                "bin_label": r.bin_label,
                "event_slug": r.event_slug,
                "target_date": r.target_date,
                "entry_price": r.entry_price,
                "shares": r.shares,
                "stake_usd": r.stake_usd,
                "order_id": r.order_id,
                "status": r.status,
                "pnl": r.pnl,
                "opened_at": r.opened_at,
                "settled_at": r.settled_at,
                "is_paper": r.is_paper,
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(path, serde_json::to_string_pretty(&out)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn settlement_exists(db: &Database, city: &str, target_date: &str) -> Result<bool> {
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM settlement_records WHERE city=?1 AND target_date=?2",
        params![city, target_date],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSettlement {
    pub city: String,
    pub target_date: String,
    pub event_id: String,
    pub event_slug: String,
    pub winning_bin: String,
    pub settlement_temp_f: Option<f64>,
    pub wunderground_station: String,
    pub resolved_at: Option<String>,
    pub raw_outcome_prices: String,
    pub market_type: Option<String>,
    pub measure: Option<String>,
    pub unit: Option<String>,
    pub city_slug: Option<String>,
    pub station_id: Option<String>,
    pub source_url: Option<String>,
    pub settlement_value: Option<f64>,
    pub settlement_unit: Option<String>,
    pub settlement_temp_source: Option<String>,
    pub settlement_temp_station: Option<String>,
    pub settlement_temp_raw_json: Option<String>,
}

pub fn insert_settlement(db: &Database, rec: &NewSettlement) -> Result<()> {
    db.conn().execute(
        "INSERT OR REPLACE INTO settlement_records (
            city, target_date, event_id, event_slug, winning_bin, settlement_temp_f,
            wunderground_station, resolved_at, raw_outcome_prices, market_type,
            measure, unit, city_slug, station_id, source_url, settlement_value,
            settlement_unit, settlement_temp_source, settlement_temp_station,
            settlement_temp_raw_json
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        params![
            rec.city,
            rec.target_date,
            rec.event_id,
            rec.event_slug,
            rec.winning_bin,
            rec.settlement_temp_f,
            rec.wunderground_station,
            rec.resolved_at,
            rec.raw_outcome_prices,
            rec.market_type,
            rec.measure,
            rec.unit,
            rec.city_slug,
            rec.station_id,
            rec.source_url,
            rec.settlement_value,
            rec.settlement_unit,
            rec.settlement_temp_source,
            rec.settlement_temp_station,
            rec.settlement_temp_raw_json,
        ],
    )?;
    Ok(())
}

pub fn scalar_count(db: &Database, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(db
        .conn()
        .query_row(&sql, [], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}

use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gfs_model_run_from_raw_json() {
        let parsed =
            parse_gfs_model_run(Some(r#"{"past_model_run":"2026-04-01T06:00:00Z"}"#)).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-04-01T06:00:00+00:00");
    }

    #[test]
    fn parses_python_minute_precision_model_run() {
        let parsed = parse_gfs_model_run(Some(r#"{"past_model_run":"2026-03-31T06:00"}"#)).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-03-31T06:00:00+00:00");
    }

    #[test]
    fn computes_snapshot_vwap_from_ask_ladder() {
        let ask = r#"[{"price":"0.20","size":"4"},{"price":"0.30","size":"6"}]"#;
        let vwap = effective_ask_from_levels(ask, 10.0).unwrap();
        assert!((vwap - 0.26).abs() < 1e-9);
        assert!(effective_ask_from_levels(ask, 11.0).is_none());
    }
}
