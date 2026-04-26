"""
Phase-1 backtest: replay price history against settled outcomes.

For each (city, date) with a known settlement, optionally filtered by city, pick
the historical forecast and NO entry price at a given lead-hours window, apply
the calibrated EV strategy filter, simulate PnL, and persist results.

Usage (via backtest.py wrapper):
    python backtest.py --strategy base_ev --days 60
    python backtest.py --strategy base_ev --date-from 2026-01-01 --date-to 2026-04-01
    python backtest.py --strategy base_ev --days 60 --city NYC
"""

import argparse
import json
import logging
import math
import re
import sys
import uuid
from datetime import datetime, timezone, date, timedelta
from pathlib import Path
from typing import Optional

sys.path.insert(0, str(Path(__file__).parent.parent))

from data.db import Database
from config.cities import CITIES
from analysis.calibration import lookup as calibration_lookup

log = logging.getLogger(__name__)

# Default entry window: between 20–28 hours before close (one snapshot per bin)
DEFAULT_LEAD_MIN = 20.0
DEFAULT_LEAD_MAX = 28.0
DEFAULT_CALIBRATION_PATH = Path(__file__).parent.parent / "calibration.json"
DEFAULT_REPLAY_MODE = "single"
DEFAULT_INTERVAL_MINUTES = 5
TRADING_WINDOW_HOURS = 36.0
SUPPORTED_REPLAY_MODES = ("single", "interval")

FORECAST_SOURCE_SQL = (
    "'gfs-global-24h',"
    "'open-meteo',"
    "'open-meteo-historical'"
)

BELOW_PATTERN = re.compile(
    r"(?i)(\d+(?:\.\d+)?)\s*(?:or below|and below|below|or lower|and lower|lower)"
)
ABOVE_PATTERN = re.compile(
    r"(?i)(\d+(?:\.\d+)?)\s*(?:or above|and above|above|or higher|and higher|higher)"
)
BETWEEN_PATTERN = re.compile(r"(?i)(\d+(?:\.\d+)?)\s*(?:to|-|through)\s*(\d+(?:\.\d+)?)")


def _load_calibration_table(path: Path) -> dict:
    if not path.exists():
        log.warning("Calibration file not found at %s; using global defaults", path)
        return {"cells": {}, "_defaults": {}}
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def _pick_forecast(
    db: Database,
    city: str,
    target_date: str,
    target_lead_hours: float,
) -> Optional[dict]:
    """
    Pick the best historical forecast for a city/date near the target lead.

    Priority:
      1. gfs-global-24h      (true ex-ante forecast, preferred)
      2. open-meteo          (live snapshots if they exist)
      3. open-meteo-historical (reanalysis fallback)
    """
    rows = db.conn.execute(
        f"""
        SELECT source, temp_max_gfs_f, temp_max_ecmwf_f, forecast_lead_hours, timestamp
        FROM forecast_records
        WHERE city = ? AND target_date = ? AND source IN ({FORECAST_SOURCE_SQL})
        ORDER BY
            CASE source
                WHEN 'gfs-global-24h' THEN 0
                WHEN 'open-meteo' THEN 1
                WHEN 'open-meteo-historical' THEN 2
                ELSE 3
            END,
            ABS(COALESCE(forecast_lead_hours, ?) - ?) ASC,
            timestamp DESC
        """,
        (city, target_date, target_lead_hours, target_lead_hours),
    ).fetchall()

    for row in rows:
        forecast_f = row["temp_max_gfs_f"]
        if forecast_f is None:
            forecast_f = row["temp_max_ecmwf_f"]
        if forecast_f is None:
            continue
        lead_hours = row["forecast_lead_hours"]
        if lead_hours is None or float(lead_hours) < 0:
            lead_hours = target_lead_hours
        return {
            "source": row["source"],
            "forecast_f": float(forecast_f),
            "forecast_lead_hours": float(lead_hours),
        }
    return None


def _norm_cdf(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


def _parse_temperature_range(bin_label: str) -> Optional[tuple[str, float, Optional[float]]]:
    normalized = bin_label.replace("°", "").replace("F", "").replace("f", "")

    below = BELOW_PATTERN.search(normalized)
    if below:
        return ("below_or_equal", float(below.group(1)), None)

    above = ABOVE_PATTERN.search(normalized)
    if above:
        return ("above_or_equal", float(above.group(1)), None)

    between = BETWEEN_PATTERN.search(normalized)
    if between:
        return ("between", float(between.group(1)), float(between.group(2)))

    return None


def _bucket_probability(bin_label: str, mu_f: float, sigma_f: float) -> Optional[float]:
    parsed = _parse_temperature_range(bin_label)
    if not parsed:
        return None

    if sigma_f <= 0:
        return None

    kind, lower, upper = parsed
    if kind == "below_or_equal":
        return _norm_cdf((lower + 0.5 - mu_f) / sigma_f)
    if kind == "above_or_equal":
        return 1.0 - _norm_cdf((lower - 0.5 - mu_f) / sigma_f)

    assert upper is not None
    hi = _norm_cdf((upper + 0.5 - mu_f) / sigma_f)
    lo = _norm_cdf((lower - 0.5 - mu_f) / sigma_f)
    return max(0.0, hi - lo)


def _pick_entry_price(db: Database, city: str, target_date: str, bin_label: str,
                      lead_min: float, lead_max: float) -> Optional[tuple[float, float]]:
    """Return (yes_price, hours_to_close) for the snapshot closest to lead_min."""
    row = db.conn.execute("""
        SELECT yes_price, hours_to_close FROM price_history
        WHERE city=? AND target_date=? AND bin_label=?
          AND hours_to_close >= ? AND hours_to_close <= ?
        ORDER BY ABS(hours_to_close - ?) ASC
        LIMIT 1
    """, (city, target_date, bin_label, lead_min, lead_max,
          (lead_min + lead_max) / 2)).fetchone()
    return (row["yes_price"], row["hours_to_close"]) if row else None


def _effective_ask_from_levels(levels_json: str, shares: float) -> Optional[float]:
    try:
        levels = json.loads(levels_json)
    except (TypeError, ValueError, json.JSONDecodeError):
        return None

    if not isinstance(levels, list) or shares <= 0:
        return None

    try:
        normalized = sorted(
            (
                {"price": float(level.get("price", 0)), "size": float(level.get("size", 0))}
                for level in levels
            ),
            key=lambda level: level["price"],
        )
    except (TypeError, ValueError):
        return None

    remaining = shares
    cost = 0.0
    for level in normalized:
        take = min(remaining, level["size"])
        cost += take * level["price"]
        remaining -= take
        if remaining <= 1e-9:
            return cost / shares
    return None


def _pick_snapshot_fill(
    db: Database,
    city: str,
    target_date: str,
    bin_label: str,
    lead_min: float,
    lead_max: float,
    shares: float,
) -> Optional[tuple[float, Optional[float], float]]:
    """
    Return (effective_ask, fee_rate, hours_to_close) from market_snapshots when available.
    """
    row = db.conn.execute(
        """
        SELECT hours_to_close, no_asks_json, fee_rate, no_vwap_10, no_best_ask
        FROM market_snapshots
        WHERE city = ? AND target_date = ? AND bin_label = ?
          AND hours_to_close >= ? AND hours_to_close <= ?
        ORDER BY ABS(hours_to_close - ?) ASC, timestamp DESC
        LIMIT 1
        """,
        (city, target_date, bin_label, lead_min, lead_max, (lead_min + lead_max) / 2),
    ).fetchone()
    if row is None:
        return None

    effective_ask = None
    if row["no_asks_json"]:
        effective_ask = _effective_ask_from_levels(row["no_asks_json"], shares)
    if effective_ask is None and shares == 10.0 and row["no_vwap_10"] is not None:
        effective_ask = float(row["no_vwap_10"])
    if effective_ask is None and row["no_best_ask"] is not None:
        effective_ask = float(row["no_best_ask"])
    if effective_ask is None:
        return None

    fee_rate = float(row["fee_rate"]) if row["fee_rate"] is not None else None
    return effective_ask, fee_rate, float(row["hours_to_close"])


def _parse_iso_timestamp(value: str | None) -> Optional[datetime]:
    if not value:
        return None
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        return datetime.fromisoformat(normalized)
    except ValueError:
        return None


def _market_close_utc(target_date: str | date) -> datetime:
    target = target_date if isinstance(target_date, date) else date.fromisoformat(target_date)
    return datetime(target.year, target.month, target.day, 12, 0, 0, tzinfo=timezone.utc)


def _market_open_utc(target_date: str | date) -> datetime:
    return _market_close_utc(target_date) - timedelta(hours=TRADING_WINDOW_HOURS)


def _hours_to_close_at(ts: datetime, target_date: str) -> float:
    return (_market_close_utc(target_date) - ts).total_seconds() / 3600.0


def _floor_timestamp(ts: datetime, interval_minutes: int) -> datetime:
    floored_minute = (ts.minute // interval_minutes) * interval_minutes
    return ts.replace(minute=floored_minute, second=0, microsecond=0)


def _start_backtest_run(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city_filter: Optional[str],
    replay_mode: str,
    interval_minutes: Optional[int] = None,
) -> tuple[str, str, dict]:
    backtest_id = str(uuid.uuid4())
    created_at = datetime.now(timezone.utc).isoformat()
    run_params = dict(params)
    if city_filter:
        run_params["city"] = city_filter
    run_params["replay_mode"] = replay_mode
    if replay_mode == "interval" and interval_minutes is not None:
        run_params["interval_minutes"] = interval_minutes

    db.conn.execute("""
        INSERT INTO backtests (id, strategy, params, date_from, date_to, created_at, status)
        VALUES (?,?,?,?,?,?,'running')
    """, (backtest_id, strategy_name, json.dumps(run_params),
          date_from.isoformat(), date_to.isoformat(), created_at))
    db.conn.commit()
    return backtest_id, created_at, run_params


def _persist_backtest_trade(
    db: Database,
    backtest_id: str,
    strategy_name: str,
    created_at: str,
    settlement_city: str,
    target_date: str,
    trade: dict,
) -> None:
    opened_at = trade.get("opened_at", created_at)
    settled_at = trade.get("settled_at", created_at)
    token_id = trade.get("token_id")
    if not token_id:
        opened_marker = str(opened_at).replace("-", "").replace(":", "").replace("+", "").replace(".", "").replace("T", "t")
        token_id = f"backtest-{backtest_id[:8]}-{settlement_city}-{target_date}-{trade['bin_label']}-{opened_marker}"

    rationale = trade.get("trade_rationale")
    if rationale is not None and not isinstance(rationale, str):
        rationale = json.dumps(rationale)

    db.conn.execute("""
        INSERT OR IGNORE INTO positions (
            token_id, market_id, bin_label, event_slug, target_date,
            entry_price, fee_rate, shares, stake_usd, order_id,
            status, pnl, opened_at, settled_at, is_paper,
            strategy_name, backtest_id, trade_rationale
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
    """, (
        token_id,
        trade.get("market_id", f"backtest-{settlement_city}-{target_date}"),
        trade["bin_label"],
        trade.get("event_slug", f"highest-temperature-in-{settlement_city}-on-{target_date}"),
        target_date,
        trade["effective_ask"], trade.get("fee_rate"),
        trade["shares"], trade["stake_usd"],
        None,
        "closed", round(trade["pnl"], 4),
        opened_at, settled_at,
        0,
        strategy_name, backtest_id, rationale,
    ))


def _finalize_backtest_run(
    db: Database,
    backtest_id: str,
    strategy_name: str,
    run_params: dict,
    city_filter: Optional[str],
    date_from: date,
    date_to: date,
    trades: list[dict],
    total_stake: float,
    extra_summary: dict,
) -> dict:
    total_pnl = sum(t["pnl"] for t in trades)
    wins = sum(1 for t in trades if t["won"])
    win_rate = wins / len(trades) if trades else 0.0

    from collections import defaultdict
    daily = defaultdict(float)
    for t in trades:
        daily[t["target_date"]] += t["pnl"]
    daily_pnls = list(daily.values())
    if len(daily_pnls) > 1:
        import statistics
        mean_pnl = statistics.mean(daily_pnls)
        std_pnl = statistics.stdev(daily_pnls)
        sharpe = (mean_pnl / std_pnl * (252 ** 0.5)) if std_pnl > 0 else 0.0
    else:
        sharpe = 0.0

    db.conn.execute("""
        UPDATE backtests SET
            total_trades=?, win_rate=?, total_pnl=?, sharpe=?, status='done'
        WHERE id=?
    """, (len(trades), win_rate, round(total_pnl, 4), round(sharpe, 4), backtest_id))
    db.conn.commit()

    summary = {
        "backtest_id": backtest_id,
        "strategy": strategy_name,
        "params": run_params,
        "city": city_filter,
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "total_trades": len(trades),
        "wins": wins,
        "win_rate": round(win_rate, 4),
        "total_pnl": round(total_pnl, 4),
        "sharpe": round(sharpe, 4),
        "total_stake": round(total_stake, 4),
    }
    summary.update(extra_summary)
    log.info("Backtest complete: %s", summary)
    return summary


def _parse_gfs_model_run(raw_json: str | None) -> Optional[datetime]:
    if not raw_json:
        return None
    try:
        payload = json.loads(raw_json)
    except (TypeError, ValueError, json.JSONDecodeError):
        return None
    model_run = payload.get("past_model_run")
    if not isinstance(model_run, str):
        return None
    if model_run.endswith("Z"):
        model_run = model_run[:-1] + "+00:00"
    elif "+" not in model_run:
        model_run = model_run + "+00:00"
    try:
        return datetime.fromisoformat(model_run)
    except ValueError:
        return None


def _load_forecast_snapshots(db: Database, city: str, target_date: str) -> list[dict]:
    rows = db.conn.execute(
        f"""
        SELECT source, temp_max_gfs_f, temp_max_ecmwf_f, forecast_lead_hours, timestamp, raw_json
        FROM forecast_records
        WHERE city = ? AND target_date = ? AND source IN ({FORECAST_SOURCE_SQL})
        ORDER BY timestamp ASC
        """,
        (city, target_date),
    ).fetchall()

    replay_open = _market_open_utc(target_date)
    snapshots = []
    for row in rows:
        forecast_f = row["temp_max_gfs_f"]
        if forecast_f is None:
            forecast_f = row["temp_max_ecmwf_f"]
        if forecast_f is None:
            continue

        source = row["source"]
        if source == "gfs-global-24h":
            available_at = _parse_gfs_model_run(row["raw_json"]) or _parse_iso_timestamp(row["timestamp"])
            priority = 0
        elif source == "open-meteo":
            available_at = _parse_iso_timestamp(row["timestamp"])
            priority = 1
        else:
            available_at = replay_open
            priority = 2

        snapshots.append({
            "source": source,
            "forecast_f": float(forecast_f),
            "forecast_lead_hours": float(row["forecast_lead_hours"]) if row["forecast_lead_hours"] is not None else None,
            "available_at": available_at or replay_open,
            "priority": priority,
        })

    return snapshots


def _select_active_forecast(
    snapshots: list[dict],
    evaluation_time: datetime,
    target_lead_hours: float,
) -> Optional[dict]:
    if not snapshots:
        return None

    available = [snapshot for snapshot in snapshots if snapshot["available_at"] <= evaluation_time]
    candidates = available or snapshots
    return sorted(
        candidates,
        key=lambda snapshot: (
            snapshot["priority"],
            abs((snapshot["forecast_lead_hours"] if snapshot["forecast_lead_hours"] is not None else target_lead_hours) - target_lead_hours),
            -snapshot["available_at"].timestamp(),
        ),
    )[0]


def _load_interval_price_bars(
    db: Database,
    city: str,
    target_date: str,
    interval_minutes: int,
) -> dict[datetime, dict[str, dict]]:
    rows = db.conn.execute(
        """
        SELECT bin_label, bin_index, token_id_yes, timestamp, yes_price, hours_to_close
        FROM price_history_1m
        WHERE city = ? AND target_date = ?
          AND hours_to_close > 0 AND hours_to_close <= ?
        ORDER BY timestamp ASC, bin_index ASC
        """,
        (city, target_date, TRADING_WINDOW_HOURS),
    ).fetchall()

    bars: dict[datetime, dict[str, dict]] = {}
    for row in rows:
        ts = _parse_iso_timestamp(row["timestamp"])
        if ts is None:
            continue
        bucket = _floor_timestamp(ts, interval_minutes)
        bar = bars.setdefault(bucket, {})
        bar[row["bin_label"]] = {
            "bin_label": row["bin_label"],
            "bin_index": row["bin_index"],
            "token_id_yes": row["token_id_yes"],
            "yes_price": float(row["yes_price"]),
            "hours_to_close": float(row["hours_to_close"]),
            "timestamp": ts,
        }
    return bars


def _load_snapshot_fill_bars(
    db: Database,
    city: str,
    target_date: str,
    interval_minutes: int,
    shares: float,
) -> dict[tuple[datetime, str], dict]:
    rows = db.conn.execute(
        """
        SELECT timestamp, bin_label, hours_to_close, no_asks_json, fee_rate, no_vwap_10, no_best_ask
        FROM market_snapshots
        WHERE city = ? AND target_date = ?
          AND (no_asks_json IS NOT NULL OR no_vwap_10 IS NOT NULL OR no_best_ask IS NOT NULL)
        ORDER BY timestamp ASC
        """,
        (city, target_date),
    ).fetchall()

    snapshot_fills: dict[tuple[datetime, str], dict] = {}
    for row in rows:
        ts = _parse_iso_timestamp(row["timestamp"])
        if ts is None:
            continue

        hours_to_close = float(row["hours_to_close"]) if row["hours_to_close"] is not None else _hours_to_close_at(ts, target_date)
        if hours_to_close <= 0 or hours_to_close > TRADING_WINDOW_HOURS:
            continue

        effective_ask = None
        if row["no_asks_json"]:
            effective_ask = _effective_ask_from_levels(row["no_asks_json"], shares)
        if effective_ask is None and shares == 10.0 and row["no_vwap_10"] is not None:
            effective_ask = float(row["no_vwap_10"])
        if effective_ask is None and row["no_best_ask"] is not None:
            effective_ask = float(row["no_best_ask"])
        if effective_ask is None:
            continue

        snapshot_fills[(_floor_timestamp(ts, interval_minutes), row["bin_label"])] = {
            "effective_ask": max(0.0, min(1.0, float(effective_ask))),
            "fee_rate": float(row["fee_rate"]) if row["fee_rate"] is not None else None,
            "hours_to_close": hours_to_close,
        }

    return snapshot_fills


def _run_single_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: Optional[str] = None,
    lead_min: float = DEFAULT_LEAD_MIN,
    lead_max: float = DEFAULT_LEAD_MAX,
    calibration_table: Optional[dict] = None,
) -> dict:
    """
    Simulate trades for all settled (city, date) pairs in [date_from, date_to],
    optionally filtered to a single city.
    Persists a backtests row and tagged positions rows. Returns a summary dict.
    """
    city_filter = city
    backtest_id, created_at, run_params = _start_backtest_run(
        db, strategy_name, params, date_from, date_to, city_filter, replay_mode="single"
    )

    min_ev = params.get("min_ev_threshold", 0.05)
    desired_shares = params.get("desired_shares", 10.0)
    max_stake = params.get("max_stake_usd", 50.0)
    funding_pct = params.get("funding_pct", 1.0)
    fee_rate = params.get("fee_rate", 0.05)
    target_lead_hours = (lead_min + lead_max) / 2.0
    calibration_table = calibration_table or {"cells": {}, "_defaults": {}}

    # Fetch all settlements in range
    settlement_sql = """
        SELECT city, target_date, winning_bin FROM settlement_records
        WHERE target_date >= ? AND target_date <= ?
    """
    settlement_params: list[object] = [date_from.isoformat(), date_to.isoformat()]
    if city_filter:
        settlement_sql += " AND city = ?"
        settlement_params.append(city_filter)
    settlement_sql += " ORDER BY target_date, city"
    settlements = db.conn.execute(settlement_sql, settlement_params).fetchall()

    trades = []
    total_stake = 0.0
    days_considered = 0
    days_with_forecast = 0
    days_with_price_history = 0
    days_with_snapshot_fills = 0
    days_missing_forecast = 0
    days_missing_price_history = 0

    for row in settlements:
        settlement_city = row["city"]
        target_date = row["target_date"]
        winning_bin = row["winning_bin"]
        days_considered += 1

        if not winning_bin:
            continue

        target_dt = date.fromisoformat(target_date)
        forecast = _pick_forecast(db, settlement_city, target_date, target_lead_hours)
        if forecast is None:
            days_missing_forecast += 1
            continue

        days_with_forecast += 1
        station_code = CITIES.get(settlement_city, {}).get("icao", settlement_city)
        beta_f, sigma_f = calibration_lookup(
            calibration_table,
            station_code,
            target_dt.month,
            forecast["forecast_lead_hours"],
        )
        mu_f = forecast["forecast_f"] + beta_f

        # Get all bins with price history for this city/date
        bins = db.conn.execute("""
            SELECT DISTINCT bin_label FROM price_history
            WHERE city=? AND target_date=?
        """, (settlement_city, target_date)).fetchall()

        if not bins:
            days_missing_price_history += 1
            continue

        days_with_price_history += 1

        day_stake = 0.0
        used_snapshot_fill = False
        shares = desired_shares * funding_pct

        for bin_row in bins:
            bin_label = bin_row["bin_label"]

            entry = _pick_entry_price(db, settlement_city, target_date, bin_label, lead_min, lead_max)
            if entry is None:
                continue

            yes_price, hours_to_close = entry
            snapshot_fill = _pick_snapshot_fill(
                db,
                settlement_city,
                target_date,
                bin_label,
                lead_min,
                lead_max,
                shares,
            )
            if snapshot_fill is not None:
                effective_ask, fee_rate_snapshot, hours_to_close = snapshot_fill
                effective_ask = max(0.0, min(1.0, effective_ask))
                effective_fee_rate = fee_rate_snapshot if fee_rate_snapshot is not None else fee_rate
                used_snapshot_fill = True
            else:
                effective_ask = max(0.0, min(1.0, 1.0 - yes_price))
                effective_fee_rate = fee_rate
            p_model = _bucket_probability(bin_label, mu_f, sigma_f)
            if p_model is None:
                continue

            ev_no = (1.0 - p_model) - effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask))

            if ev_no < min_ev:
                continue

            stake = shares * effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask))
            if day_stake + stake > max_stake or total_stake + stake > max_stake * 10:
                continue

            # Settle PnL
            if bin_label == winning_bin:
                pnl = -(shares * effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask)))
            else:
                pnl = shares * (1.0 - effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask)))

            day_stake += stake
            total_stake += stake

            trade = {
                "city": settlement_city,
                "target_date": target_date,
                "bin_label": bin_label,
                "winning_bin": winning_bin,
                "forecast_source": forecast["source"],
                "forecast_f": forecast["forecast_f"],
                "forecast_lead_hours": forecast["forecast_lead_hours"],
                "beta_f": beta_f,
                "mu_f": mu_f,
                "sigma_f": sigma_f,
                "p_model": p_model,
                "yes_price": yes_price,
                "effective_ask": effective_ask,
                "fill_source": "market_snapshots" if snapshot_fill is not None else "price_history_proxy",
                "fee_rate": effective_fee_rate,
                "ev_no": ev_no,
                "hours_to_close": hours_to_close,
                "shares": shares,
                "stake_usd": stake,
                "pnl": pnl,
                "won": bin_label != winning_bin,
            }
            trades.append(trade)

            _persist_backtest_trade(
                db,
                backtest_id,
                strategy_name,
                created_at,
                settlement_city,
                target_date,
                {
                    **trade,
                    "trade_rationale": {
                        "ev_no": ev_no,
                        "p_model": p_model,
                        "forecast_source": forecast["source"],
                        "forecast_f": forecast["forecast_f"],
                        "forecast_lead_hours": forecast["forecast_lead_hours"],
                        "beta_f": beta_f,
                        "mu_f": mu_f,
                        "sigma_f": sigma_f,
                        "yes_price": yes_price,
                        "fee_rate": effective_fee_rate,
                        "fill_source": "market_snapshots" if snapshot_fill is not None else "price_history_proxy",
                        "hours_to_close": hours_to_close,
                    },
                },
            )

        if used_snapshot_fill:
            days_with_snapshot_fills += 1

    db.conn.commit()

    return _finalize_backtest_run(
        db=db,
        backtest_id=backtest_id,
        strategy_name=strategy_name,
        run_params=run_params,
        city_filter=city_filter,
        date_from=date_from,
        date_to=date_to,
        trades=trades,
        total_stake=total_stake,
        extra_summary={
        "days_considered": days_considered,
        "days_with_forecast": days_with_forecast,
        "days_with_price_history": days_with_price_history,
        "days_with_snapshot_fills": days_with_snapshot_fills,
        "days_missing_forecast": days_missing_forecast,
        "days_missing_price_history": days_missing_price_history,
        },
    )


def _run_interval_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: Optional[str] = None,
    interval_minutes: int = DEFAULT_INTERVAL_MINUTES,
    calibration_table: Optional[dict] = None,
) -> dict:
    city_filter = city
    backtest_id, created_at, run_params = _start_backtest_run(
        db, strategy_name, params, date_from, date_to, city_filter,
        replay_mode="interval", interval_minutes=interval_minutes
    )

    min_ev = params.get("min_ev_threshold", 0.05)
    desired_shares = params.get("desired_shares", 10.0)
    max_stake = params.get("max_stake_usd", 50.0)
    funding_pct = params.get("funding_pct", 1.0)
    fee_rate = params.get("fee_rate", 0.05)
    calibration_table = calibration_table or {"cells": {}, "_defaults": {}}

    settlement_sql = """
        SELECT city, target_date, winning_bin FROM settlement_records
        WHERE target_date >= ? AND target_date <= ?
    """
    settlement_params: list[object] = [date_from.isoformat(), date_to.isoformat()]
    if city_filter:
        settlement_sql += " AND city = ?"
        settlement_params.append(city_filter)
    settlement_sql += " ORDER BY target_date, city"
    settlements = db.conn.execute(settlement_sql, settlement_params).fetchall()

    trades = []
    total_stake = 0.0
    days_considered = 0
    days_with_forecast = 0
    days_with_price_history = 0
    days_with_minute_price_history = 0
    days_with_snapshot_fills = 0
    days_missing_forecast = 0
    days_missing_price_history = 0
    interval_bars_considered = 0
    intervals_with_trades = 0
    forecast_switches = 0

    shares = desired_shares * funding_pct

    for row in settlements:
        settlement_city = row["city"]
        target_date = row["target_date"]
        winning_bin = row["winning_bin"]
        days_considered += 1

        if not winning_bin:
            continue

        forecast_snapshots = _load_forecast_snapshots(db, settlement_city, target_date)
        if not forecast_snapshots:
            days_missing_forecast += 1
            continue

        days_with_forecast += 1
        price_bars = _load_interval_price_bars(db, settlement_city, target_date, interval_minutes)
        if not price_bars:
            days_missing_price_history += 1
            continue

        days_with_price_history += 1
        days_with_minute_price_history += 1

        snapshot_fills = _load_snapshot_fill_bars(
            db, settlement_city, target_date, interval_minutes, shares
        )
        station_code = CITIES.get(settlement_city, {}).get("icao", settlement_city)
        target_dt = date.fromisoformat(target_date)

        day_stake = 0.0
        opened_bins: set[str] = set()
        used_snapshot_fill = False
        last_forecast_key = None

        for bucket_ts in sorted(price_bars):
            bars = price_bars[bucket_ts]
            if not bars:
                continue

            interval_bars_considered += 1
            reference_market = next(iter(bars.values()))
            current_lead_hours = float(reference_market["hours_to_close"])
            active_forecast = _select_active_forecast(
                forecast_snapshots,
                bucket_ts,
                current_lead_hours,
            )
            if active_forecast is None:
                continue

            forecast_key = (
                active_forecast["source"],
                active_forecast["available_at"].isoformat(),
                round(active_forecast["forecast_f"], 4),
            )
            if forecast_key != last_forecast_key:
                forecast_switches += 1
                last_forecast_key = forecast_key

            beta_f, sigma_f = calibration_lookup(
                calibration_table,
                station_code,
                target_dt.month,
                current_lead_hours,
            )
            mu_f = active_forecast["forecast_f"] + beta_f

            candidates = []
            for bin_label, market in sorted(bars.items(), key=lambda item: item[1]["bin_index"]):
                if bin_label in opened_bins:
                    continue

                snapshot_fill = snapshot_fills.get((bucket_ts, bin_label))
                if snapshot_fill is not None:
                    effective_ask = snapshot_fill["effective_ask"]
                    effective_fee_rate = snapshot_fill["fee_rate"] if snapshot_fill["fee_rate"] is not None else fee_rate
                    hours_to_close = snapshot_fill["hours_to_close"]
                    fill_source = "market_snapshots"
                    used_snapshot_fill = True
                else:
                    effective_ask = max(0.0, min(1.0, 1.0 - float(market["yes_price"])))
                    effective_fee_rate = fee_rate
                    hours_to_close = current_lead_hours
                    fill_source = "price_history_1m_proxy"

                p_model = _bucket_probability(bin_label, mu_f, sigma_f)
                if p_model is None:
                    continue

                ev_no = (1.0 - p_model) - effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask))
                if ev_no < min_ev:
                    continue

                stake = shares * effective_ask * (1.0 + effective_fee_rate * (1.0 - effective_ask))
                candidates.append({
                    "city": settlement_city,
                    "target_date": target_date,
                    "bin_label": bin_label,
                    "winning_bin": winning_bin,
                    "forecast_source": active_forecast["source"],
                    "forecast_f": active_forecast["forecast_f"],
                    "forecast_lead_hours": current_lead_hours,
                    "beta_f": beta_f,
                    "mu_f": mu_f,
                    "sigma_f": sigma_f,
                    "p_model": p_model,
                    "yes_price": float(market["yes_price"]),
                    "effective_ask": effective_ask,
                    "fill_source": fill_source,
                    "fee_rate": effective_fee_rate,
                    "ev_no": ev_no,
                    "hours_to_close": hours_to_close,
                    "shares": shares,
                    "stake_usd": stake,
                    "won": bin_label != winning_bin,
                    "opened_at": bucket_ts.isoformat(),
                    "settled_at": _market_close_utc(target_date).isoformat(),
                    "trade_rationale": {
                        "ev_no": ev_no,
                        "p_model": p_model,
                        "forecast_source": active_forecast["source"],
                        "forecast_f": active_forecast["forecast_f"],
                        "forecast_lead_hours": current_lead_hours,
                        "beta_f": beta_f,
                        "mu_f": mu_f,
                        "sigma_f": sigma_f,
                        "yes_price": float(market["yes_price"]),
                        "fee_rate": effective_fee_rate,
                        "fill_source": fill_source,
                        "hours_to_close": hours_to_close,
                        "evaluation_time": bucket_ts.isoformat(),
                        "replay_mode": "interval",
                        "interval_minutes": interval_minutes,
                    },
                })

            trades_this_interval = 0
            for candidate in candidates:
                if day_stake + candidate["stake_usd"] > max_stake or total_stake + candidate["stake_usd"] > max_stake * 10:
                    continue

                if candidate["bin_label"] == winning_bin:
                    candidate["pnl"] = -(candidate["shares"] * candidate["effective_ask"] * (1.0 + candidate["fee_rate"] * (1.0 - candidate["effective_ask"])))
                else:
                    candidate["pnl"] = candidate["shares"] * (1.0 - candidate["effective_ask"] * (1.0 + candidate["fee_rate"] * (1.0 - candidate["effective_ask"])))

                day_stake += candidate["stake_usd"]
                total_stake += candidate["stake_usd"]
                opened_bins.add(candidate["bin_label"])
                trades.append(candidate)
                _persist_backtest_trade(
                    db,
                    backtest_id,
                    strategy_name,
                    created_at,
                    settlement_city,
                    target_date,
                    candidate,
                )
                trades_this_interval += 1

            if trades_this_interval > 0:
                intervals_with_trades += 1

        if used_snapshot_fill:
            days_with_snapshot_fills += 1

    db.conn.commit()

    return _finalize_backtest_run(
        db=db,
        backtest_id=backtest_id,
        strategy_name=strategy_name,
        run_params=run_params,
        city_filter=city_filter,
        date_from=date_from,
        date_to=date_to,
        trades=trades,
        total_stake=total_stake,
        extra_summary={
            "days_considered": days_considered,
            "days_with_forecast": days_with_forecast,
            "days_with_price_history": days_with_price_history,
            "days_with_minute_price_history": days_with_minute_price_history,
            "days_with_snapshot_fills": days_with_snapshot_fills,
            "days_missing_forecast": days_missing_forecast,
            "days_missing_price_history": days_missing_price_history,
            "interval_minutes": interval_minutes,
            "interval_bars_considered": interval_bars_considered,
            "intervals_with_trades": intervals_with_trades,
            "forecast_switches": forecast_switches,
        },
    )


def run_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: Optional[str] = None,
    lead_min: float = DEFAULT_LEAD_MIN,
    lead_max: float = DEFAULT_LEAD_MAX,
    calibration_table: Optional[dict] = None,
    replay_mode: str = DEFAULT_REPLAY_MODE,
    interval_minutes: int = DEFAULT_INTERVAL_MINUTES,
) -> dict:
    if replay_mode not in SUPPORTED_REPLAY_MODES:
        raise ValueError(f"Unsupported replay_mode '{replay_mode}'")
    if replay_mode == "interval":
        return _run_interval_backtest(
            db=db,
            strategy_name=strategy_name,
            params=params,
            date_from=date_from,
            date_to=date_to,
            city=city,
            interval_minutes=interval_minutes,
            calibration_table=calibration_table,
        )
    return _run_single_backtest(
        db=db,
        strategy_name=strategy_name,
        params=params,
        date_from=date_from,
        date_to=date_to,
        city=city,
        lead_min=lead_min,
        lead_max=lead_max,
        calibration_table=calibration_table,
    )


def _load_strategy_params(strategy_name: str, strategies_toml: Optional[Path]) -> dict:
    """Load params from strategies.toml if present, else return defaults."""
    if strategies_toml and strategies_toml.exists():
        try:
            import tomllib  # Python 3.11+
        except ImportError:
            try:
                import tomli as tomllib
            except ImportError:
                log.warning("tomllib not available, using default strategy params")
                return {}

        with open(strategies_toml, "rb") as f:
            data = tomllib.load(f)
        for s in data.get("strategy", []):
            if s["name"] == strategy_name:
                return {
                    "min_ev_threshold": s.get("min_ev_threshold", 0.05),
                    "desired_shares": s.get("desired_shares", 10.0),
                    "max_stake_usd": s.get("max_stake_usd", 50.0),
                    "funding_pct": s.get("funding_pct", 1.0),
                    "fee_rate": 0.05,
                }
        log.warning("Strategy '%s' not found in %s, using defaults", strategy_name, strategies_toml)
    return {
        "min_ev_threshold": 0.05,
        "desired_shares": 10.0,
        "max_stake_usd": 50.0,
        "funding_pct": 1.0,
        "fee_rate": 0.05,
    }


def main():
    parser = argparse.ArgumentParser(description="Run a strategy backtest")
    parser.add_argument("--strategy", default="base_ev", help="Strategy name")
    parser.add_argument("--days", type=int, default=60, help="Days of history to backtest")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD (overrides --days)")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD (default: yesterday)")
    parser.add_argument("--city", help="Optional city filter, e.g. NYC")
    parser.add_argument("--replay-mode", choices=SUPPORTED_REPLAY_MODES, default=DEFAULT_REPLAY_MODE,
                        help="Backtest engine: 'single' picks one entry window, 'interval' replays minute history in fixed buckets")
    parser.add_argument("--interval-minutes", type=int, default=DEFAULT_INTERVAL_MINUTES,
                        help="Bucket size for interval replay mode (default: 5)")
    parser.add_argument("--lead-min", type=float, default=DEFAULT_LEAD_MIN,
                        help="Min hours before close to use as entry price (single replay mode)")
    parser.add_argument("--lead-max", type=float, default=DEFAULT_LEAD_MAX,
                        help="Max hours before close to use as entry price (single replay mode)")
    parser.add_argument("--strategies-file", default="../poly-weather-bot/strategies.toml",
                        help="Path to strategies.toml")
    parser.add_argument("--calibration-file", default=str(DEFAULT_CALIBRATION_PATH),
                        help="Path to calibration.json")
    parser.add_argument("--db", default=None, help="Path to data.db")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s — %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )

    today = datetime.now(timezone.utc).date()
    date_to = date.fromisoformat(args.date_to) if args.date_to else today - timedelta(days=1)
    if args.date_from:
        date_from = date.fromisoformat(args.date_from)
    else:
        date_from = date_to - timedelta(days=args.days - 1)
    city = args.city.upper() if args.city else None
    if city and city not in CITIES:
        parser.error(f"--city must be one of: {', '.join(sorted(CITIES))}")
    if args.interval_minutes <= 0:
        parser.error("--interval-minutes must be positive")

    strategies_file = Path(args.strategies_file) if args.strategies_file else None
    calibration_file = Path(args.calibration_file)
    params = _load_strategy_params(args.strategy, strategies_file)
    calibration_table = _load_calibration_table(calibration_file)

    from data.db import Database
    db = Database() if not args.db else Database(Path(args.db))

    summary = run_backtest(
        db=db,
        strategy_name=args.strategy,
        params=params,
        date_from=date_from,
        date_to=date_to,
        city=city,
        lead_min=args.lead_min,
        lead_max=args.lead_max,
        calibration_table=calibration_table,
        replay_mode=args.replay_mode,
        interval_minutes=args.interval_minutes,
    )

    print(json.dumps(summary, indent=2))
    db.close()


if __name__ == "__main__":
    main()
