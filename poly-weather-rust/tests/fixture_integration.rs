use chrono::{Duration, NaiveDate, Utc};
use poly_weather_rust::analysis::{backtest, bankroll, calibration};
use poly_weather_rust::commands::{execute, settle};
use poly_weather_rust::db::Database;
use rusqlite::params;
use serde_json::json;

fn fixture_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.db");
    let db = Database::open(&path, false).unwrap();
    seed_backtest_rows(&db);
    (dir, db)
}

fn seed_backtest_rows(db: &Database) {
    db.conn()
        .execute(
            "INSERT INTO settlement_records (
                city, target_date, event_id, event_slug, winning_bin,
                settlement_temp_f, wunderground_station, resolved_at,
                raw_outcome_prices, market_type, measure, unit, city_slug,
                station_id, source_url, settlement_value, settlement_unit
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                "NYC",
                "2026-04-02",
                "event-1",
                "highest-temperature-in-nyc-on-april-2-2026",
                "80-81°F",
                80.4,
                "KLGA",
                "2026-04-02T12:00:00Z",
                "{}",
                "daily_temperature",
                "high",
                "F",
                "nyc",
                "KLGA",
                "fixture",
                80.4,
                "F",
            ],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO forecast_records (
                timestamp, city, target_date, source, temp_max_ecmwf_c,
                temp_max_gfs_c, temp_max_ecmwf_f, temp_max_gfs_f,
                model_spread_c, forecast_lead_hours, raw_json, market_type,
                measure, unit, city_slug, station_id, event_slug, value_ecmwf,
                value_gfs, value_unit
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![
                "2026-04-01T06:00:00Z",
                "NYC",
                "2026-04-02",
                "gfs-global-24h",
                26.7,
                26.7,
                80.0,
                80.0,
                0.0,
                30.0,
                r#"{"past_model_run":"2026-04-01T06:00:00Z"}"#,
                "daily_temperature",
                "high",
                "F",
                "nyc",
                "KLGA",
                "highest-temperature-in-nyc-on-april-2-2026",
                80.0,
                80.0,
                "F",
            ],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO price_history_1m (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                "NYC",
                "2026-04-02",
                "70-71°F",
                0,
                "yes-token",
                "2026-04-01T12:00:00Z",
                0.95,
                24.0,
                "fixture",
            ],
        )
        .unwrap();
}

fn fixture_calibration() -> serde_json::Value {
    json!({
        "cells": {},
        "_defaults": {
            "KLGA": {"beta_f": 0.0, "sigma_f": 2.0, "n": 100},
            "_global": {"beta_f": 0.0, "sigma_f": 2.0, "n": 100}
        }
    })
}

#[test]
fn backtest_runs_against_fixture_sqlite() {
    let (_dir, db) = fixture_db();
    let summary = backtest::run_backtest(
        &db,
        backtest::BacktestRequest {
            strategy_name: "base_no_ev".to_string(),
            params: json!({
                "strategy_id": "base_no_ev",
                "min_ev_threshold": 0.01,
                "desired_shares": 10.0,
                "max_stake_usd": 50.0,
                "fee_rate": 0.05
            }),
            date_from: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
            date_to: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
            city: Some("NYC".to_string()),
            calibration_table: fixture_calibration(),
            include_trades: true,
        },
    )
    .unwrap();
    assert_eq!(summary.total_trades, 1);
    assert!(summary.total_pnl > 0.0);
    assert_eq!(summary.trades.len(), 1);
}

#[test]
fn bankroll_replays_fixture_backtest_candidates() {
    let (_dir, db) = fixture_db();
    let summary = backtest::run_backtest(
        &db,
        backtest::BacktestRequest {
            strategy_name: "base_no_ev".to_string(),
            params: json!({
                "strategy_id": "base_no_ev",
                "min_ev_threshold": 0.01,
                "desired_shares": 10.0,
                "max_stake_usd": 50.0,
                "fee_rate": 0.05
            }),
            date_from: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
            date_to: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
            city: Some("NYC".to_string()),
            calibration_table: fixture_calibration(),
            include_trades: true,
        },
    )
    .unwrap();
    let candidates = summary
        .trades
        .iter()
        .map(|row| bankroll::ReplayCandidate::from_trade_row(row, Some("NYC"), "base_no_ev"))
        .collect();
    let ledger = bankroll::apply_bankroll_ledger(candidates, 100.0, 1.0, true);
    assert_eq!(ledger.summary.trades_opened, 1);
    assert!(ledger.summary.final_equity > 100.0);
}

#[test]
fn calibration_builds_fixture_shape() {
    let (_dir, db) = fixture_db();
    let table = calibration::build_table(&db).unwrap();
    assert!(table.get("cells").is_some());
    assert!(table.get("_defaults").is_some());
}

#[test]
fn paper_execute_and_settlement_pnl_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("paper.db");
    let positions_path = dir.path().join("open_positions.json");
    let plan_path = dir.path().join("execution_plan.json");
    let db = Database::open(&db_path, false).unwrap();
    std::fs::write(
        &plan_path,
        json!({
            "event_slug": "highest-temperature-in-nyc-on-april-2-2026",
            "target_date": "2026-04-02",
            "dry_run": true,
            "trades": [{
                "no_token_id": "no-token",
                "market_id": "market",
                "bin_label": "70-71°F",
                "effective_ask": 0.25,
                "fee_rate": 0.05,
                "shares": 10.0,
                "stake_usd": 2.5,
                "strategy_name": "base_no_ev",
                "ev_no": 0.1,
                "p_model": 0.2
            }]
        })
        .to_string(),
    )
    .unwrap();
    execute::run(execute::ExecuteArgs {
        plan: plan_path,
        positions: positions_path.clone(),
        db: Some(db_path),
        log_dir: None,
        dry_run: true,
    })
    .unwrap();
    assert_eq!(
        poly_weather_rust::db::get_open_positions(&db)
            .unwrap()
            .len(),
        1
    );
    let closed = settle::settle_positions_for_test(
        &db,
        "2026-04-02",
        "80-81°F",
        Some(&(Utc::now() + Duration::hours(1)).to_rfc3339()),
        Some("highest-temperature-in-nyc-on-april-2-2026"),
    )
    .unwrap();
    assert_eq!(closed, 1);
    assert_eq!(
        poly_weather_rust::db::load_positions(&db, Some("closed"), None)
            .unwrap()
            .len(),
        1
    );
}
