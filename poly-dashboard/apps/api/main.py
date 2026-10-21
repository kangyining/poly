"""
Poly Dashboard — Weather API

Serves two route groups:
  /api/backtest/*  — backtest management and results
  /api/live/*      — live/paper trading monitoring

Start:
    python3 -m uvicorn --app-dir apps/api main:app --reload --port 8000
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from uuid import UUID, uuid4
from datetime import datetime, timezone, timedelta
from pathlib import Path

from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware

# Resolve paths relative to this file, but keep them overridable so the
# dashboard repo can sit alongside the existing trading/data repos.
API_DIR = Path(__file__).resolve().parent
APPS_DIR = API_DIR.parent
DASHBOARD_DIR = APPS_DIR.parent
WORKSPACE_DIR = DASHBOARD_DIR.parent


def _path_from_env(name: str, default: Path) -> Path:
    return Path(os.environ.get(name, str(default))).resolve()


POLY_WEATHER = _path_from_env("POLY_WEATHER_DIR", WORKSPACE_DIR / "poly-weather")
BOT_DIR = _path_from_env("POLY_WEATHER_BOT_DIR", WORKSPACE_DIR / "poly-weather-bot")
DB_PATH = _path_from_env("POLY_WEATHER_DB_PATH", POLY_WEATHER / "db" / "data.db")
STRATEGIES_JSON = _path_from_env(
    "POLY_WEATHER_STRATEGIES_PATH", POLY_WEATHER / "analysis/strategies/strategies.json",
)
BOT_CONFIG = BOT_DIR / "config.toml"
BACKTEST_DIR = _path_from_env("POLY_BACKTEST_DIR", DASHBOARD_DIR / ".data/backtests")

sys.path.insert(0, str(POLY_WEATHER))

import sqlite3
from config.cities import CITIES
from data.db import Database
from analysis.strategies import list_strategy_configs


def get_db():
    if not DB_PATH.exists():
        Database(DB_PATH).close()
    conn = sqlite3.connect(str(DB_PATH), check_same_thread=False)
    conn.row_factory = sqlite3.Row
    return conn


def _date_window(days: int) -> tuple[str, str]:
    today = datetime.now(timezone.utc).date()
    end = today + timedelta(days=days - 1)
    return today.isoformat(), end.isoformat()


app = FastAPI(title="Poly Dashboard Weather API", version="1.0.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["http://localhost:5173", "http://localhost:3000", "http://127.0.0.1:5173"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# ── Backtest routes ────────────────────────────────────────────────────────────

def _read_run(run_id: str) -> dict:
    try:
        if str(UUID(run_id)) != run_id:
            raise ValueError("Invalid run ID")
    except ValueError:
        raise HTTPException(status_code=404, detail="Backtest not found")
    path = BACKTEST_DIR / run_id / "result.json"
    if not path.is_file():
        raise HTTPException(status_code=404, detail="Backtest not found")
    return json.loads(path.read_text(encoding="utf-8"))


def _run_meta(result: dict) -> dict:
    return {**{key: value for key, value in result.items() if key not in {"trades", "artifacts"}},
            "params": json.dumps(result.get("params", {}))}


@app.get("/api/backtest/runs")
def list_backtest_runs(limit: int = Query(50, ge=1, le=500)):
    rows = []
    for path in BACKTEST_DIR.glob("*/result.json"):
        try:
            result = _read_run(path.parent.name)
            rows.append(_run_meta(result))
        except (HTTPException, ValueError, OSError):
            continue
    return sorted(rows, key=lambda row: row["created_at"], reverse=True)[:limit]


@app.post("/api/backtest/run")
def trigger_backtest(
    strategy: str = Query("base_no_ev"),
    days: int = Query(60, ge=1, le=730),
    date_from: str | None = Query(None),
    date_to: str | None = Query(None),
    city: str | None = Query(None),
    replay_mode: str = Query("interval"),
    interval_minutes: int = Query(1, ge=1, le=60),
):
    if replay_mode != "interval" or interval_minutes != 1:
        raise HTTPException(status_code=400, detail="The current engine supports 1-minute interval replay.")
    available = {row["name"] for row in list_strategy_configs(STRATEGIES_JSON)}
    if strategy not in available and strategy != "base_ev":
        raise HTTPException(status_code=400, detail="Unknown strategy")
    if city:
        city = "NYC" if city.upper() == "NYC" else city.title()
        if city not in CITIES:
            raise HTTPException(status_code=400, detail="Unsupported city")
    try:
        end = datetime.fromisoformat(date_to).date() if date_to else datetime.now(timezone.utc).date() - timedelta(days=1)
        start = datetime.fromisoformat(date_from).date() if date_from else end - timedelta(days=days - 1)
        if start > end or (end - start).days >= 730:
            raise ValueError("Invalid date range")
    except ValueError:
        raise HTTPException(status_code=400, detail="Use a valid date range of at most 730 days.")

    conn = get_db()
    conn.close()
    run_id = str(uuid4())
    run_dir = BACKTEST_DIR / run_id
    run_dir.mkdir(parents=True)
    pending = run_dir / "pending.json"
    created_at = datetime.now(timezone.utc).isoformat()
    cmd = [sys.executable, str(POLY_WEATHER / "backtest.py"),
           "--strategy", strategy, "--date-from", start.isoformat(), "--date-to", end.isoformat(),
           "--db", str(DB_PATH), "--strategies-file", str(STRATEGIES_JSON),
           "--json", str(pending), "--csv", str(run_dir / "summary.csv"),
           "--trades-csv", str(run_dir / "trades.csv"), "--report", str(run_dir / "report.md")]
    if city:
        cmd += ["--city", city]
    try:
        completed = subprocess.run(cmd, capture_output=True, text=True, timeout=300, cwd=str(POLY_WEATHER))
    except subprocess.TimeoutExpired:
        raise HTTPException(status_code=504, detail="Backtest timed out after 300s")
    if completed.returncode != 0:
        raise HTTPException(status_code=500, detail=f"Backtest failed: {completed.stderr[-2000:]}")
    try:
        result = json.loads(pending.read_text(encoding="utf-8"))
        if not isinstance(result, dict) or not isinstance(result.get("trades"), list) or "backtest_id" not in result:
            raise ValueError("Invalid result schema")
    except (OSError, ValueError):
        raise HTTPException(status_code=500, detail="Backtest produced an invalid result file")
    result.update(id=run_id, created_at=created_at, status="done", result_storage="files")
    pending.write_text(json.dumps(result, indent=2), encoding="utf-8")
    pending.replace(run_dir / "result.json")
    return _run_meta(result)


@app.get("/api/backtest/strategies")
def list_strategies():
    return list_strategy_configs(STRATEGIES_JSON)


@app.get("/api/backtest/compare")
def compare_backtests(ids: str = Query(...)):
    run_ids = [value.strip() for value in ids.split(",") if value.strip()]
    if not run_ids:
        raise HTTPException(status_code=400, detail="Provide at least one backtest ID")
    return [_run_meta(_read_run(run_id)) for run_id in run_ids]


@app.get("/api/backtest/{backtest_id}")
def get_backtest(backtest_id: str):
    result = _read_run(backtest_id)
    return {"meta": _run_meta(result), "trades": result["trades"]}


# ── Live trading routes ────────────────────────────────────────────────────────

@app.get("/api/live/positions")
def get_positions(status: str = Query("open"), limit: int = Query(100, ge=1, le=1000)):
    """Open or recent closed positions (excluding backtest rows)."""
    conn = get_db()
    if status == "all":
        rows = conn.execute("""
            SELECT * FROM positions
            WHERE backtest_id IS NULL
            ORDER BY opened_at DESC LIMIT ?
        """, (limit,)).fetchall()
    else:
        rows = conn.execute("""
            SELECT * FROM positions
            WHERE status=? AND backtest_id IS NULL
            ORDER BY opened_at DESC LIMIT ?
        """, (status, limit)).fetchall()
    conn.close()
    return [dict(r) for r in rows]


@app.get("/api/live/settlements")
def get_settlements(days: int = Query(30, ge=1, le=365)):
    """Settlement history for the last N days."""
    since = (datetime.now(timezone.utc) - timedelta(days=days)).date().isoformat()
    conn = get_db()
    rows = conn.execute("""
        SELECT city, target_date, event_slug, winning_bin,
               settlement_temp_f, resolved_at
        FROM settlement_records
        WHERE target_date >= ?
        ORDER BY target_date DESC, city
    """, (since,)).fetchall()
    conn.close()
    return [dict(r) for r in rows]


@app.get("/api/live/pnl")
def get_daily_pnl(days: int = Query(30, ge=1, le=365)):
    """Daily PnL time series from closed positions (excluding backtests)."""
    since = (datetime.now(timezone.utc) - timedelta(days=days)).date().isoformat()
    conn = get_db()
    rows = conn.execute("""
        SELECT DATE(settled_at) AS day, SUM(pnl) AS daily_pnl, COUNT(*) AS trades
        FROM positions
        WHERE status='closed' AND backtest_id IS NULL AND settled_at >= ?
        GROUP BY day
        ORDER BY day
    """, (since,)).fetchall()
    conn.close()
    # Add running cumulative PnL
    result = []
    running = 0.0
    for r in rows:
        running += r["daily_pnl"] or 0
        result.append({
            "day": r["day"],
            "daily_pnl": round(r["daily_pnl"] or 0, 4),
            "cumulative_pnl": round(running, 4),
            "trades": r["trades"],
        })
    return result


@app.get("/api/live/strategies")
def get_live_strategies():
    """Active strategy configs with per-strategy PnL summary from live/paper positions."""
    conn = get_db()

    # Per-strategy PnL from non-backtest positions
    pnl_rows = conn.execute("""
        SELECT strategy_name,
               COUNT(*) AS total,
               SUM(CASE WHEN status='open' THEN 1 ELSE 0 END) AS open_count,
               SUM(CASE WHEN status='closed' THEN 1 ELSE 0 END) AS closed_count,
               SUM(CASE WHEN status='closed' THEN pnl ELSE 0 END) AS total_pnl,
               SUM(stake_usd) AS total_staked
        FROM positions
        WHERE backtest_id IS NULL
        GROUP BY strategy_name
    """).fetchall()
    conn.close()

    pnl_by_strategy = {r["strategy_name"]: dict(r) for r in pnl_rows}

    # Merge with strategies.toml if available
    strategy_configs = []
    if BOT_CONFIG.exists():
        try:
            try:
                import tomllib
            except ImportError:
                import tomli as tomllib  # type: ignore
            with open(BOT_CONFIG, "rb") as f:
                data = tomllib.load(f)
            for s in data.get("strategies", []):
                entry = dict(s)
                entry.update(pnl_by_strategy.get(s["name"], {}))
                strategy_configs.append(entry)
        except Exception:
            pass

    # Include any strategies in DB not in toml (e.g., "default")
    for name, stats in pnl_by_strategy.items():
        if not any(s.get("name") == name for s in strategy_configs):
            strategy_configs.append(stats)

    return strategy_configs


@app.get("/api/live/daily-summary")
def get_daily_summary(
    days: int = Query(3, ge=1, le=7),
    city: str | None = Query(None),
):
    """Combined next-N-day weather + market summary for the live dashboard."""
    start_date, end_date = _date_window(days)
    selected_cities = [city] if city else list(CITIES.keys())

    summary: dict[tuple[str, str], dict] = {}
    start = datetime.fromisoformat(start_date).date()
    for city_name in selected_cities:
        for offset in range(days):
            target_date = (start + timedelta(days=offset)).isoformat()
            summary[(city_name, target_date)] = {
                "city": city_name,
                "target_date": target_date,
                "nws_temp_f": None,
                "nws_short_forecast": None,
                "nws_period_name": None,
                "nws_timestamp": None,
                "open_meteo_ecmwf_f": None,
                "open_meteo_gfs_f": None,
                "open_meteo_spread_c": None,
                "open_meteo_timestamp": None,
                "market_snapshot_at": None,
                "market_bin_count": 0,
                "market_bins_with_no_ask": 0,
                "market_avg_no_best_ask": None,
                "market_min_no_best_ask": None,
                "market_max_no_best_ask": None,
                "event_slug": None,
            }

    conn = get_db()
    params: list[object] = [start_date, end_date]
    city_filter = ""
    if city:
        city_filter = " AND city = ?"
        params.append(city)

    nws_rows = conn.execute(
        f"""
        SELECT n.city, n.target_date, n.timestamp, n.temp_f, n.short_forecast, n.period_name
        FROM nws_forecasts n
        INNER JOIN (
            SELECT city, target_date, MAX(timestamp) AS latest
            FROM nws_forecasts
            WHERE target_date BETWEEN ? AND ?{city_filter}
            GROUP BY city, target_date
        ) mx
          ON n.city = mx.city
         AND n.target_date = mx.target_date
         AND n.timestamp = mx.latest
        ORDER BY n.city, n.target_date
        """,
        params,
    ).fetchall()

    forecast_rows = conn.execute(
        f"""
        SELECT f.city, f.target_date, f.timestamp, f.temp_max_ecmwf_f,
               f.temp_max_gfs_f, f.model_spread_c
        FROM forecast_records f
        INNER JOIN (
            SELECT city, target_date, MAX(timestamp) AS latest
            FROM forecast_records
            WHERE source = 'open-meteo' AND target_date BETWEEN ? AND ?{city_filter}
            GROUP BY city, target_date
        ) mx
          ON f.city = mx.city
         AND f.target_date = mx.target_date
         AND f.timestamp = mx.latest
        WHERE f.source = 'open-meteo'
        ORDER BY f.city, f.target_date
        """,
        params,
    ).fetchall()

    snapshot_rows = conn.execute(
        f"""
        SELECT latest.city,
               latest.target_date,
               MAX(latest.timestamp) AS snapshot_at,
               MAX(latest.event_slug) AS event_slug,
               COUNT(*) AS market_bin_count,
               SUM(CASE WHEN latest.no_best_ask IS NOT NULL THEN 1 ELSE 0 END) AS market_bins_with_no_ask,
               AVG(latest.no_best_ask) AS market_avg_no_best_ask,
               MIN(latest.no_best_ask) AS market_min_no_best_ask,
               MAX(latest.no_best_ask) AS market_max_no_best_ask
        FROM (
            SELECT s.*
            FROM market_snapshots s
            INNER JOIN (
                SELECT city, target_date, bin_label, MAX(timestamp) AS latest
                FROM market_snapshots
                WHERE target_date BETWEEN ? AND ?{city_filter}
                GROUP BY city, target_date, bin_label
            ) mx
              ON s.city = mx.city
             AND s.target_date = mx.target_date
             AND s.bin_label = mx.bin_label
             AND s.timestamp = mx.latest
        ) latest
        GROUP BY latest.city, latest.target_date
        ORDER BY latest.city, latest.target_date
        """,
        params,
    ).fetchall()
    conn.close()

    for row in nws_rows:
        key = (row["city"], row["target_date"])
        if key in summary:
            summary[key].update({
                "nws_temp_f": row["temp_f"],
                "nws_short_forecast": row["short_forecast"],
                "nws_period_name": row["period_name"],
                "nws_timestamp": row["timestamp"],
            })

    for row in forecast_rows:
        key = (row["city"], row["target_date"])
        if key in summary:
            summary[key].update({
                "open_meteo_ecmwf_f": row["temp_max_ecmwf_f"],
                "open_meteo_gfs_f": row["temp_max_gfs_f"],
                "open_meteo_spread_c": row["model_spread_c"],
                "open_meteo_timestamp": row["timestamp"],
            })

    for row in snapshot_rows:
        key = (row["city"], row["target_date"])
        if key in summary:
            summary[key].update(dict(row))

    return list(summary.values())


@app.get("/api/live/snapshots/latest")
def get_latest_snapshots(
    days: int = Query(2, ge=1, le=7),
    city: str | None = Query(None),
):
    """Latest market snapshot per (city, target_date, bin_label)."""
    start_date, end_date = _date_window(days)
    params: list[object] = [start_date, end_date]
    city_filter = ""
    if city:
        city_filter = " AND city = ?"
        params.append(city)

    conn = get_db()
    rows = conn.execute(f"""
        SELECT s.*
        FROM market_snapshots s
        INNER JOIN (
            SELECT city, target_date, bin_label, MAX(timestamp) AS latest
            FROM market_snapshots
            WHERE target_date BETWEEN ? AND ?{city_filter}
            GROUP BY city, target_date, bin_label
        ) mx
          ON s.city = mx.city
         AND s.target_date = mx.target_date
         AND s.bin_label = mx.bin_label
         AND s.timestamp = mx.latest
        ORDER BY s.city, s.target_date, s.bin_index
    """, params).fetchall()
    conn.close()
    return [dict(r) for r in rows]


@app.get("/health")
def health():
    return {
        "status": "ok",
        "apps_dir": str(APPS_DIR),
        "dashboard_dir": str(DASHBOARD_DIR),
        "poly_weather_dir": str(POLY_WEATHER),
        "bot_dir": str(BOT_DIR),
        "db": str(DB_PATH),
        "db_exists": DB_PATH.exists(),
    }
