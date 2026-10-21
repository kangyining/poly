"""
Phase-1 backtest: replay price history against settled outcomes.

For each (city, date) with a known settlement, optionally filtered by city,
replay 1-minute historical prices through the strategy plugins, simulate PnL,
and write file-based results.

Usage (via backtest.py wrapper):
    python backtest.py --strategy base_ev --days 60
    python backtest.py --strategy base_ev --date-from 2026-01-01 --date-to 2026-04-01
    python backtest.py --strategy base_ev --days 60 --city NYC
"""

import argparse
import csv
import bisect
import json
import logging
import re
import sys
import uuid
from datetime import datetime, timezone, date, timedelta
from pathlib import Path
from typing import Any, Optional

sys.path.insert(0, str(Path(__file__).parent.parent))

from data.db import Database
from config.weather_contracts import (
    contract_for_city,
    enabled_daily_high_contracts,
)
from data.weather_contracts import build_daily_high_slug
from analysis.calibration import lookup as calibration_lookup
from analysis.strategies import ForecastState, MarketBin, StrategyContext, get_strategy, group_members, list_strategy_configs, load_strategy_params
from analysis.strategies.core import StrategyCandidate, StrategyDiagnostic
from analysis.strategies.registry import DEFAULT_CONFIG as DEFAULT_STRATEGY_CONFIG, resolve_strategy_name
from analysis.strategies.utils import entry_cost, settle_single, settle_union

log = logging.getLogger(__name__)

DEFAULT_CALIBRATION_PATH = Path(__file__).parent.parent / "calibration.json"
DEFAULT_STRATEGIES_PATH = DEFAULT_STRATEGY_CONFIG
DEFAULT_INTERVAL_MINUTES = 1
DEFAULT_OUTPUT_DIR = Path(__file__).parent.parent / "backtest"
TRADING_WINDOW_HOURS = 36.0

FORECAST_SOURCE_SQL = (
    "'gfs-global-24h',"
    "'open-meteo',"
    "'open-meteo-historical'"
)

def _load_calibration_table(path: Path) -> dict:
    if not path.exists():
        log.warning("Calibration file not found at %s; using global defaults", path)
        return {"cells": {}, "_defaults": {}}
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def _slug(value: str) -> str:
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9]+", "_", value)
    return value.strip("_") or "run"


def _auto_artifact_paths(
    strategy_names: list[str],
    city: str | None,
    date_from: date,
    date_to: date,
) -> dict[str, Path]:
    city_part = _slug(city or "all_cities")
    if len(strategy_names) == 1:
        strategy_part = _slug(strategy_names[0])
    elif len(strategy_names) <= 3:
        strategy_part = _slug("_".join(strategy_names))
    else:
        strategy_part = f"{len(strategy_names)}_strategies"
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    stem = f"{city_part}_{date_from.isoformat()}_{date_to.isoformat()}_interval_1m_{strategy_part}_{stamp}"
    return {
        "summary_csv": DEFAULT_OUTPUT_DIR / f"{stem}_summary.csv",
        "trades_csv": DEFAULT_OUTPUT_DIR / f"{stem}_trades.csv",
        "report": DEFAULT_OUTPUT_DIR / f"{stem}_report.md",
    }


def _seed_research_sources(db: Database) -> None:
    path = Path(__file__).parent / "research_sources.json"
    if not path.exists():
        return
    try:
        sources = json.loads(path.read_text(encoding="utf-8"))
        saved = db.seed_research_sources(sources)
        if saved:
            log.info("Seeded %d research sources", saved)
    except Exception as e:
        log.warning("Failed to seed research sources: %s", e)


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


def _forecast_state(forecast: dict, beta_f: float, sigma_f: float) -> ForecastState:
    spread_f = None
    if forecast.get("model_spread_c") is not None:
        spread_f = float(forecast["model_spread_c"]) * 9.0 / 5.0
    return ForecastState(
        source=forecast["source"],
        forecast_f=float(forecast["forecast_f"]),
        lead_hours=float(forecast["forecast_lead_hours"]),
        beta_f=beta_f,
        sigma_f=sigma_f,
        mu_f=float(forecast["forecast_f"]) + beta_f,
        gfs_f=forecast.get("gfs_f"),
        ecmwf_f=forecast.get("ecmwf_f"),
        model_spread_f=spread_f,
    )


def _build_market_bin(
    city: str,
    target_date: str,
    bin_label: str,
    bin_index: int,
    yes_price: float,
    hours_to_close: float,
    shares: float,
    fee_rate: float,
    snapshot_fill: tuple[float, Optional[float], float] | None = None,
    opened_at: str | None = None,
    token_id_yes: str | None = None,
    snapshot_row: dict | None = None,
) -> MarketBin:
    no_best_bid = None
    no_best_ask = None
    if snapshot_row:
        no_best_bid = snapshot_row.get("no_best_bid")
        no_best_ask = snapshot_row.get("no_best_ask")

    if snapshot_fill is not None:
        effective_ask, fee_rate_snapshot, hours_to_close = snapshot_fill
        effective_ask_no = max(0.0, min(1.0, effective_ask))
        effective_fee_rate = fee_rate_snapshot if fee_rate_snapshot is not None else fee_rate
        fill_source = "market_snapshots"
    else:
        effective_ask_no = max(0.0, min(1.0, 1.0 - yes_price))
        effective_fee_rate = fee_rate
        fill_source = "price_history_proxy"

    return MarketBin(
        bin_label=bin_label,
        bin_index=bin_index,
        yes_price=max(0.0, min(1.0, float(yes_price))),
        no_price=max(0.0, min(1.0, 1.0 - float(yes_price))),
        effective_ask_no=effective_ask_no,
        fee_rate=float(effective_fee_rate),
        hours_to_close=float(hours_to_close),
        token_id_yes=token_id_yes,
        token_id_no=None,
        no_best_bid=float(no_best_bid) if no_best_bid is not None else None,
        no_best_ask=float(no_best_ask) if no_best_ask is not None else None,
        fill_source=fill_source,
        opened_at=opened_at,
    )


def _climatology_temps(db: Database, city: str, target_date: str, window_days: int = 14) -> list[float]:
    target = date.fromisoformat(target_date)
    rows = db.conn.execute(
        """
        SELECT target_date, settlement_temp_f
        FROM settlement_records
        WHERE city = ? AND settlement_temp_f IS NOT NULL AND target_date != ?
        """,
        (city, target_date),
    ).fetchall()
    temps = []
    for row in rows:
        try:
            other = date.fromisoformat(row["target_date"])
        except ValueError:
            continue
        # Calendar-neighbor climatology. Use month/day distance, ignoring year.
        pseudo_target = date(2001, target.month, target.day)
        pseudo_other = date(2001, other.month, other.day)
        delta = abs((pseudo_other - pseudo_target).days)
        delta = min(delta, 365 - delta)
        if delta <= window_days:
            temps.append(float(row["settlement_temp_f"]))
    return temps


def _load_metar_records(db: Database, city: str, target_date: str) -> list[tuple[datetime, float]]:
    rows = db.conn.execute(
        """
        SELECT obs_time, timestamp, temp_f
        FROM metar_records
        WHERE city = ? AND DATE(COALESCE(obs_time, timestamp)) <= ?
        ORDER BY COALESCE(obs_time, timestamp)
        """,
        (city, target_date),
    ).fetchall()
    records = []
    for row in rows:
        ts = _parse_iso_timestamp(row["obs_time"]) or _parse_iso_timestamp(row["timestamp"])
        if ts is not None and row["temp_f"] is not None:
            records.append((ts, float(row["temp_f"])))
    return records


def _metar_max_before(records: list[tuple[datetime, float]], evaluation_time: datetime | None) -> float | None:
    if evaluation_time is None:
        return None
    vals = [temp for ts, temp in records if ts <= evaluation_time]
    return max(vals) if vals else None


def _metar_prefix_max(records: list[tuple[datetime, float]]) -> tuple[list[datetime], list[float]]:
    times: list[datetime] = []
    maxes: list[float] = []
    current_max: float | None = None
    for ts, temp in records:
        times.append(ts)
        current_max = temp if current_max is None else max(current_max, temp)
        maxes.append(current_max)
    return times, maxes


def _metar_max_from_prefix(
    times: list[datetime],
    maxes: list[float],
    evaluation_time: datetime | None,
) -> float | None:
    if evaluation_time is None or not times:
        return None
    idx = bisect.bisect_right(times, evaluation_time) - 1
    if idx < 0:
        return None
    return maxes[idx]


def _candidate_pnl(candidate: StrategyCandidate, winning_bin: str) -> tuple[float, bool]:
    cost = entry_cost(candidate.effective_ask, candidate.fee_rate)
    if candidate.side in {"YES", "NO"}:
        return settle_single(candidate.side, candidate.bin_label, winning_bin, candidate.shares, cost)
    if candidate.side in {"YES_CORRIDOR", "YES_LOW_TAIL", "YES_HIGH_TAIL", "YES_BASKET"}:
        bins = candidate.metadata.get("underlying_bins", [])
        return settle_union(winning_bin, list(bins), candidate.shares, candidate.effective_ask)
    return settle_single(candidate.side, candidate.bin_label, winning_bin, candidate.shares, cost)


def _diagnostic_counts(diagnostics: list[StrategyDiagnostic]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for diagnostic in diagnostics:
        key = f"{diagnostic.strategy_name}:{diagnostic.severity}"
        counts[key] = counts.get(key, 0) + 1
    return counts


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
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city_filter: Optional[str],
) -> tuple[str, str, dict]:
    backtest_id = str(uuid.uuid4())
    created_at = datetime.now(timezone.utc).isoformat()
    run_params = dict(params)
    if city_filter:
        run_params["city"] = city_filter
    run_params["replay_mode"] = "interval"
    run_params["interval_minutes"] = DEFAULT_INTERVAL_MINUTES

    return backtest_id, created_at, run_params


def _backtest_trade_row(
    backtest_id: str,
    strategy_name: str,
    created_at: str,
    settlement_city: str,
    target_date: str,
    trade: dict,
) -> dict[str, Any]:
    opened_at = trade.get("opened_at", created_at)
    settled_at = trade.get("settled_at", created_at)
    token_id = trade.get("token_id")
    if not token_id:
        opened_marker = str(opened_at).replace("-", "").replace(":", "").replace("+", "").replace(".", "").replace("T", "t")
        token_id = f"backtest-{backtest_id[:8]}-{settlement_city}-{target_date}-{trade['bin_label']}-{opened_marker}"

    rationale = trade.get("trade_rationale")
    if rationale is not None and not isinstance(rationale, str):
        rationale = json.dumps(rationale)

    return {
        "backtest_id": backtest_id,
        "strategy_name": strategy_name,
        "target_date": target_date,
        "event_slug": trade.get("event_slug", f"highest-temperature-in-{settlement_city}-on-{target_date}"),
        "bin_label": trade["bin_label"],
        "entry_price": trade["effective_ask"],
        "fee_rate": trade.get("fee_rate"),
        "shares": trade["shares"],
        "stake_usd": trade["stake_usd"],
        "pnl": round(trade["pnl"], 4),
        "status": "closed",
        "opened_at": opened_at,
        "settled_at": settled_at,
        "token_id": token_id,
        "market_id": trade.get("market_id", f"backtest-{settlement_city}-{target_date}"),
        "trade_rationale": rationale,
    }


def _finalize_backtest_run(
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
    days_considered = int(extra_summary.get("days_considered") or 0)
    trade_days = {t["target_date"] for t in trades}
    active_trade_days = len(trade_days)
    max_daily_stake = run_params.get("max_stake_usd")
    daily_cap_return = None
    try:
        max_daily_stake_float = float(max_daily_stake)
        if max_daily_stake_float > 0:
            daily_cap_return = total_pnl / max_daily_stake_float
    except (TypeError, ValueError):
        daily_cap_return = None

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
        "turnover_yield": round(total_pnl / total_stake, 6) if total_stake > 0 else None,
        "pnl_per_calendar_day": round(total_pnl / days_considered, 4) if days_considered > 0 else None,
        "pnl_per_active_day": round(total_pnl / active_trade_days, 4) if active_trade_days > 0 else None,
        "pnl_per_trade": round(total_pnl / len(trades), 4) if trades else None,
        "active_trade_days": active_trade_days,
        "return_on_daily_cap": round(daily_cap_return, 6) if daily_cap_return is not None else None,
        "result_storage": "files",
    }
    summary.update(extra_summary)
    log.info("Backtest complete: %s", summary)
    return summary


def _jsonish(value: Any) -> Any:
    if isinstance(value, (dict, list, tuple)):
        return json.dumps(value, sort_keys=True)
    return value


def _flatten_csv_row(row: dict[str, Any], prefix: str = "") -> dict[str, Any]:
    flat: dict[str, Any] = {}
    for key, value in row.items():
        if key.startswith("_"):
            continue
        csv_key = f"{prefix}{key}" if not prefix else f"{prefix}.{key}"
        if isinstance(value, dict):
            flat.update(_flatten_csv_row(value, csv_key))
        else:
            flat[csv_key] = _jsonish(value)
    return flat


def _write_summary_csv(path: Path, summaries: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    rows = [_flatten_csv_row(summary) for summary in summaries]
    preferred = [
        "backtest_id",
        "strategy",
        "strategy_plugin",
        "city",
        "date_from",
        "date_to",
        "total_trades",
        "wins",
        "win_rate",
        "total_pnl",
        "sharpe",
        "total_stake",
        "turnover_yield",
        "pnl_per_calendar_day",
        "pnl_per_active_day",
        "pnl_per_trade",
        "active_trade_days",
        "return_on_daily_cap",
        "result_storage",
        "days_considered",
        "days_with_forecast",
        "days_with_price_history",
        "days_with_minute_price_history",
        "days_with_snapshot_fills",
        "days_missing_forecast",
        "days_missing_price_history",
        "interval_minutes",
        "interval_bars_considered",
        "intervals_with_trades",
        "forecast_switches",
        "params.strategy_id",
        "params.min_ev_threshold",
        "params.desired_shares",
        "params.funding_pct",
        "params.max_stake_usd",
        "params.fee_rate",
        "params.replay_mode",
    ]
    remaining = sorted({key for row in rows for key in row} - set(preferred))
    fieldnames = [key for key in preferred if any(key in row for row in rows)] + remaining
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def _write_trades_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = [
        "backtest_id",
        "strategy_name",
        "target_date",
        "event_slug",
        "bin_label",
        "entry_price",
        "fee_rate",
        "shares",
        "stake_usd",
        "pnl",
        "status",
        "opened_at",
        "settled_at",
        "token_id",
        "market_id",
        "trade_rationale",
    ]
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(
            sorted(
                rows,
                key=lambda row: (
                    str(row.get("strategy_name") or ""),
                    str(row.get("target_date") or ""),
                    str(row.get("opened_at") or ""),
                    str(row.get("bin_label") or ""),
                ),
            )
        )


def _fmt_num(value: Any, digits: int = 4) -> str:
    if value is None:
        return ""
    try:
        return f"{float(value):.{digits}f}"
    except (TypeError, ValueError):
        return str(value)


def _fmt_pct(value: Any) -> str:
    if value is None:
        return ""
    try:
        return f"{float(value) * 100:.2f}%"
    except (TypeError, ValueError):
        return str(value)


def _turnover_yield(summary: dict[str, Any]) -> float | None:
    if summary.get("turnover_yield") is not None:
        return float(summary["turnover_yield"])
    stake = float(summary.get("total_stake") or 0.0)
    if stake <= 0:
        return None
    return float(summary.get("total_pnl") or 0.0) / stake


def _diagnostics_text(summary: dict[str, Any]) -> str:
    diagnostics = summary.get("diagnostics") or {}
    if not diagnostics:
        return ""
    return ", ".join(f"{key}={value}" for key, value in sorted(diagnostics.items()))


def _write_summary_report(
    path: Path,
    summaries: list[dict[str, Any]],
    summary_csv: str | None = None,
    trades_csv: str | None = None,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    ranked = sorted(
        summaries,
        key=lambda summary: (
            float(summary.get("total_pnl") or 0.0),
            _turnover_yield(summary) if _turnover_yield(summary) is not None else float("-inf"),
            int(summary.get("total_trades") or 0),
        ),
        reverse=True,
    )
    traded = [summary for summary in summaries if int(summary.get("total_trades") or 0) > 0]
    best_pnl = ranked[0] if ranked else None
    best_turnover_yield = max(
        (summary for summary in summaries if _turnover_yield(summary) is not None),
        key=lambda summary: _turnover_yield(summary) or float("-inf"),
        default=None,
    )
    total_trades = sum(int(summary.get("total_trades") or 0) for summary in summaries)
    total_pnl = sum(float(summary.get("total_pnl") or 0.0) for summary in summaries)

    first = summaries[0] if summaries else {}
    lines = [
        f"# Strategy Backtest Report: {first.get('city') or 'All Cities'}",
        "",
        "## Scope",
        "",
        f"- Date range: `{first.get('date_from', '')}` to `{first.get('date_to', '')}`",
        "- Replay: `1-minute interval`",
        f"- Strategies run: `{len(summaries)}`",
        f"- Strategies with trades: `{len(traded)}`",
        f"- Total simulated trades: `{total_trades}`",
        f"- Combined PnL: `{_fmt_num(total_pnl)}`",
    ]
    if summary_csv:
        lines.append(f"- Summary CSV: `{summary_csv}`")
    if trades_csv:
        lines.append(f"- Trades CSV: `{trades_csv}`")

    lines.extend(["", "## Highlights", ""])
    if best_pnl:
        lines.append(
            f"- Best total PnL: `{best_pnl.get('strategy')}` with `{_fmt_num(best_pnl.get('total_pnl'))}` "
            f"over `{best_pnl.get('total_trades')}` trades."
        )
    if best_turnover_yield:
        lines.append(
            f"- Best turnover yield: `{best_turnover_yield.get('strategy')}` with "
            f"`{_fmt_pct(_turnover_yield(best_turnover_yield))}` "
            f"on `{_fmt_num(best_turnover_yield.get('total_stake'))}` simulated stake turnover."
        )
    if not traded:
        lines.append("- No strategy produced trades in this run. Check `days_considered`, price history, forecasts, and diagnostics.")

    lines.extend(
        [
            "",
            "## Strategy Comparison",
            "",
            "| Rank | Strategy | Plugin | Trades | Win% | PnL | Stake Turnover | Turnover Yield | PnL/Day | PnL/Trade | Return on Daily Cap | Sharpe | Days | Forecast Days | Price Days | Snapshot Days | Diagnostics |",
            "|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|",
        ]
    )
    for rank, summary in enumerate(ranked, start=1):
        lines.append(
            "| {rank} | `{strategy}` | `{plugin}` | {trades} | {win_rate} | {pnl} | {stake} | {turnover_yield} | {pnl_per_day} | {pnl_per_trade} | {daily_cap_return} | {sharpe} | {days} | {forecast_days} | {price_days} | {snapshot_days} | {diagnostics} |".format(
                rank=rank,
                strategy=summary.get("strategy") or "",
                plugin=summary.get("strategy_plugin") or "",
                trades=summary.get("total_trades") or 0,
                win_rate=_fmt_pct(summary.get("win_rate")),
                pnl=_fmt_num(summary.get("total_pnl")),
                stake=_fmt_num(summary.get("total_stake")),
                turnover_yield=_fmt_pct(_turnover_yield(summary)),
                pnl_per_day=_fmt_num(summary.get("pnl_per_calendar_day")),
                pnl_per_trade=_fmt_num(summary.get("pnl_per_trade")),
                daily_cap_return=_fmt_pct(summary.get("return_on_daily_cap")),
                sharpe=_fmt_num(summary.get("sharpe")),
                days=summary.get("days_considered") or 0,
                forecast_days=summary.get("days_with_forecast") or 0,
                price_days=summary.get("days_with_price_history") or 0,
                snapshot_days=summary.get("days_with_snapshot_fills") or 0,
                diagnostics=_diagnostics_text(summary),
            )
        )

    lines.extend(
        [
            "",
            "## Notes",
            "",
            "- This report compares the strategies exactly as configured for this run.",
            "- `Stake Turnover` is the sum of simulated trade stakes, not starting capital.",
            "- `Turnover Yield` is `total_pnl / stake_turnover`; it is not portfolio ROI.",
            "- `PnL/Day` is `total_pnl / days_considered`.",
            "- `Return on Daily Cap` is `total_pnl / max_stake_usd` for this run's daily stake cap.",
            "- Diagnostics are emitted by strategy plugins when required data is missing or a research strategy is not yet fully supported.",
            "- Use the trade-level CSV to inspect individual bins, prices, rationale JSON, and PnL.",
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


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
        SELECT source, temp_max_gfs_f, temp_max_ecmwf_f, model_spread_c, forecast_lead_hours, timestamp, raw_json
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
            "gfs_f": float(row["temp_max_gfs_f"]) if row["temp_max_gfs_f"] is not None else None,
            "ecmwf_f": float(row["temp_max_ecmwf_f"]) if row["temp_max_ecmwf_f"] is not None else None,
            "model_spread_c": float(row["model_spread_c"]) if row["model_spread_c"] is not None else None,
            "forecast_lead_hours": float(row["forecast_lead_hours"]) if row["forecast_lead_hours"] is not None else None,
            "available_at": available_at or replay_open,
            "priority": priority,
        })

    return snapshots


def _station_for_city(city: str) -> str:
    _slug, contract = contract_for_city(city)
    if contract:
        return contract["station_id"]
    return city


def _default_event_slug(city: str, target_date: str) -> str:
    _slug, contract = contract_for_city(city)
    target = date.fromisoformat(target_date)
    if contract:
        return build_daily_high_slug(_slug, target)
    fallback_slug = re.sub(r"[^a-z0-9]+", "-", city.strip().lower()).strip("-")
    return build_daily_high_slug(fallback_slug, target)


def _known_city_names() -> list[str]:
    names = {cfg["display_city"] for cfg in enabled_daily_high_contracts().values()}
    return sorted(names)


def _normalize_city_filter(raw: str | None) -> str | None:
    if not raw:
        return None
    _slug, contract = contract_for_city(raw)
    if contract:
        return contract["display_city"]
    for name in _known_city_names():
        if name.lower() == raw.lower():
            return name
    return raw


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
        SELECT timestamp, bin_label, hours_to_close, no_asks_json, fee_rate, no_vwap_10, no_best_bid, no_best_ask
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
            "no_best_bid": float(row["no_best_bid"]) if row["no_best_bid"] is not None else None,
            "no_best_ask": float(row["no_best_ask"]) if row["no_best_ask"] is not None else None,
        }

    return snapshot_fills


def _run_interval_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: Optional[str] = None,
    calibration_table: Optional[dict] = None,
    include_trades: bool = True,
) -> dict:
    city_filter = city
    backtest_id, created_at, run_params = _start_backtest_run(
        strategy_name, params, date_from, date_to, city_filter
    )
    interval_minutes = DEFAULT_INTERVAL_MINUTES

    min_ev = params.get("min_ev_threshold", 0.05)
    desired_shares = params.get("desired_shares", 10.0)
    max_stake_raw = params.get("max_stake_usd", 50.0)
    max_stake = float("inf") if max_stake_raw is None else float(max_stake_raw)
    max_total_stake = params.get("max_total_stake_usd")
    funding_pct = params.get("funding_pct", 1.0)
    fee_rate = params.get("fee_rate", 0.05)
    strategy = get_strategy(strategy_name, params)
    resolved_strategy = resolve_strategy_name(strategy_name, params)
    calibration_table = calibration_table or {"cells": {}, "_defaults": {}}

    settlement_sql = """
        SELECT city, target_date, winning_bin, event_slug FROM settlement_records
        WHERE target_date >= ? AND target_date <= ?
    """
    settlement_params: list[object] = [date_from.isoformat(), date_to.isoformat()]
    if city_filter:
        settlement_sql += " AND city = ?"
        settlement_params.append(city_filter)
    settlement_sql += " ORDER BY target_date, city"
    settlements = db.conn.execute(settlement_sql, settlement_params).fetchall()

    trades = []
    trade_rows: list[dict[str, Any]] = []
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
    diagnostics: list[StrategyDiagnostic] = []

    shares = desired_shares * funding_pct

    for row in settlements:
        settlement_city = row["city"]
        target_date = row["target_date"]
        winning_bin = row["winning_bin"]
        event_slug = row["event_slug"] or _default_event_slug(settlement_city, target_date)
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
        station_code = _station_for_city(settlement_city)
        target_dt = date.fromisoformat(target_date)
        metar_records = _load_metar_records(db, settlement_city, target_date)
        metar_times, metar_maxes = _metar_prefix_max(metar_records)
        climatology_temps = _climatology_temps(db, settlement_city, target_date)

        day_stake = 0.0
        opened_bins: set[str] = set()
        used_snapshot_fill = False
        last_forecast_key = None

        for bucket_ts, bars in sorted(price_bars.items()):
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

            market_bins: list[MarketBin] = []
            for bin_label, market in sorted(bars.items(), key=lambda item: item[1]["bin_index"]):
                if f"{strategy.name}:{bin_label}" in opened_bins:
                    continue

                snapshot_fill = snapshot_fills.get((bucket_ts, bin_label))
                if snapshot_fill is not None:
                    fill_tuple = (
                        snapshot_fill["effective_ask"],
                        snapshot_fill["fee_rate"],
                        snapshot_fill["hours_to_close"],
                    )
                    used_snapshot_fill = True
                else:
                    fill_tuple = None

                market_bins.append(
                    _build_market_bin(
                        settlement_city,
                        target_date,
                        bin_label,
                        int(market["bin_index"] or 0),
                        float(market["yes_price"]),
                        current_lead_hours,
                        shares,
                        fee_rate,
                        fill_tuple,
                        snapshot_row=snapshot_fill,
                        opened_at=bucket_ts.isoformat(),
                        token_id_yes=market["token_id_yes"],
                    )
                )

            forecast_payload = {
                "source": active_forecast["source"],
                "forecast_f": active_forecast["forecast_f"],
                "forecast_lead_hours": current_lead_hours,
                "gfs_f": active_forecast.get("gfs_f"),
                "ecmwf_f": active_forecast.get("ecmwf_f"),
                "model_spread_c": active_forecast.get("model_spread_c"),
            }
            context = StrategyContext(
                city=settlement_city,
                target_date=target_date,
                winning_bin=winning_bin,
                bins=market_bins,
                forecast=_forecast_state(forecast_payload, beta_f, sigma_f),
                params=params,
                replay_mode="interval",
                evaluation_time=bucket_ts,
                event_slug=event_slug,
                settlement_temp_f=None,
                metar_max_so_far_f=_metar_max_from_prefix(metar_times, metar_maxes, bucket_ts),
                climatology_temps_f=climatology_temps,
            )
            output = strategy.generate_candidates(context)
            diagnostics.extend(output.diagnostics)

            trades_this_interval = 0
            for candidate in output.candidates:
                opened_key = f"{candidate.strategy_name}:{candidate.side}:{candidate.bin_label}"
                if opened_key in opened_bins:
                    continue
                if day_stake + candidate.stake_usd > max_stake:
                    continue
                if max_total_stake is not None and total_stake + candidate.stake_usd > float(max_total_stake):
                    continue

                candidate.settled_at = _market_close_utc(target_date).isoformat()
                pnl, won = _candidate_pnl(candidate, winning_bin)

                day_stake += candidate.stake_usd
                total_stake += candidate.stake_usd
                opened_bins.add(opened_key)
                trade = {
                    "city": settlement_city,
                    "target_date": target_date,
                    "bin_label": candidate.bin_label,
                    "winning_bin": winning_bin,
                    "forecast_source": active_forecast["source"],
                    "forecast_f": active_forecast["forecast_f"],
                    "forecast_lead_hours": current_lead_hours,
                    "beta_f": beta_f,
                    "mu_f": mu_f,
                    "sigma_f": sigma_f,
                    "p_model": candidate.p_model,
                    "effective_ask": candidate.effective_ask,
                    "fill_source": candidate.fill_source,
                    "fee_rate": candidate.fee_rate,
                    "ev_no": candidate.edge,
                    "hours_to_close": current_lead_hours,
                    "shares": candidate.shares,
                    "stake_usd": candidate.stake_usd,
                    "pnl": pnl,
                    "won": won,
                    "opened_at": candidate.opened_at,
                    "settled_at": candidate.settled_at,
                }
                trades.append(trade)
                if include_trades:
                    trade_rows.append(
                        _backtest_trade_row(
                            backtest_id,
                            strategy_name,
                            created_at,
                            settlement_city,
                            target_date,
                            candidate.to_trade(pnl, won),
                        )
                    )
                trades_this_interval += 1

            if trades_this_interval > 0:
                intervals_with_trades += 1

        if used_snapshot_fill:
            days_with_snapshot_fills += 1

    summary = _finalize_backtest_run(
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
            "strategy_plugin": resolved_strategy,
            "diagnostics": _diagnostic_counts(diagnostics),
        },
    )
    if include_trades:
        summary["_trades"] = trade_rows
    return summary


def run_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: Optional[str] = None,
    calibration_table: Optional[dict] = None,
    include_trades: bool = True,
) -> dict:
    return _run_interval_backtest(
        db=db,
        strategy_name=strategy_name,
        params=params,
        date_from=date_from,
        date_to=date_to,
        city=city,
        calibration_table=calibration_table,
        include_trades=include_trades,
    )


def _load_strategy_params(strategy_name: str, strategies_toml: Optional[Path]) -> dict:
    """Load params from the Python strategy config, JSON, or legacy TOML."""
    if strategies_toml and strategies_toml.exists() and strategies_toml.suffix.lower() == ".json":
        try:
            return load_strategy_params(strategy_name, strategies_toml)
        except Exception as e:
            log.warning("Strategy '%s' not found in %s: %s; using defaults", strategy_name, strategies_toml, e)

    if strategies_toml and strategies_toml.exists() and strategies_toml.suffix.lower() == ".toml":
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
                    "max_stake_usd": s.get("max_stake_usd", 10.0),
                    "funding_pct": s.get("funding_pct", 1.0),
                    "fixed_trade_stake_usd": s.get("fixed_trade_stake_usd", 10.0),
                    "fee_rate": 0.05,
                }
        log.warning("Strategy '%s' not found in %s, using defaults", strategy_name, strategies_toml)
    try:
        return load_strategy_params(strategy_name, DEFAULT_STRATEGIES_PATH)
    except Exception:
        pass
    return {
        "min_ev_threshold": 0.05,
        "desired_shares": 10.0,
        "max_stake_usd": 10.0,
        "funding_pct": 1.0,
        "fixed_trade_stake_usd": 10.0,
        "fee_rate": 0.05,
        "strategy_id": resolve_strategy_name(strategy_name),
    }


def main():
    parser = argparse.ArgumentParser(description="Run a strategy backtest")
    parser.add_argument("--strategy", default="base_ev", help="Strategy name")
    parser.add_argument("--strategy-group", help="Run every strategy in a configured group")
    parser.add_argument("--all-strategies", action="store_true", help="Run every configured strategy")
    parser.add_argument("--list-strategies", action="store_true", help="Print configured strategy metadata and exit")
    parser.add_argument("--days", type=int, default=60, help="Days of history to backtest")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD (overrides --days)")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD (default: yesterday)")
    parser.add_argument("--city", help="Optional city filter, e.g. NYC")
    parser.add_argument("--strategies-file", default=str(DEFAULT_STRATEGIES_PATH),
                        help="Path to Python strategy JSON config")
    parser.add_argument("--calibration-file", default=str(DEFAULT_CALIBRATION_PATH),
                        help="Path to calibration.json")
    parser.add_argument("--db", default=None, help="Path to data.db")
    parser.add_argument("--csv", dest="summary_csv", help="Custom path for per-strategy summary CSV")
    parser.add_argument("--trades-csv", help="Custom path for trade-level CSV")
    parser.add_argument("--report", help="Custom path for Markdown strategy comparison report")
    parser.add_argument("--json", dest="result_json", help="Structured result JSON, including simulated trades")
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
    city = _normalize_city_filter(args.city)
    known_cities = _known_city_names()
    if city and city not in known_cities:
        parser.error(f"--city must be one of: {', '.join(known_cities)}")
    strategies_file = Path(args.strategies_file) if args.strategies_file else None
    if args.list_strategies:
        print(json.dumps(list_strategy_configs(strategies_file or DEFAULT_STRATEGIES_PATH), indent=2))
        return
    calibration_file = Path(args.calibration_file)
    calibration_table = _load_calibration_table(calibration_file)

    from data.db import Database
    db = Database(read_only=True) if not args.db else Database(Path(args.db), read_only=True)

    if args.all_strategies:
        strategy_names = group_members("all", strategies_file or DEFAULT_STRATEGIES_PATH)
    elif args.strategy_group:
        strategy_names = group_members(args.strategy_group, strategies_file or DEFAULT_STRATEGIES_PATH)
    else:
        strategy_names = [args.strategy]

    auto_paths = _auto_artifact_paths(strategy_names, city, date_from, date_to)
    summary_csv_path = Path(args.summary_csv) if args.summary_csv else auto_paths["summary_csv"]
    trades_csv_path = Path(args.trades_csv) if args.trades_csv else auto_paths["trades_csv"]
    report_path = Path(args.report) if args.report else auto_paths["report"]

    summaries = []
    try:
        for strategy_name in strategy_names:
            params = _load_strategy_params(strategy_name, strategies_file)
            summary = run_backtest(
                db=db,
                strategy_name=strategy_name,
                params=params,
                date_from=date_from,
                date_to=date_to,
                city=city,
                calibration_table=calibration_table,
            )
            summaries.append(summary)
        trade_rows = [trade for summary in summaries for trade in summary.pop("_trades", [])]
        _write_summary_csv(summary_csv_path, summaries)
        log.info("Wrote summary CSV: %s", summary_csv_path)
        _write_trades_csv(trades_csv_path, trade_rows)
        log.info("Wrote trades CSV: %s", trades_csv_path)
        _write_summary_report(report_path, summaries, str(summary_csv_path), str(trades_csv_path))
        log.info("Wrote strategy report: %s", report_path)
    finally:
        db.close()

    payload = summaries[0] if len(summaries) == 1 else {"runs": summaries}
    payload["artifacts"] = {
        "summary_csv": str(summary_csv_path),
        "trades_csv": str(trades_csv_path),
        "report": str(report_path),
    }
    if args.result_json:
        result_path = Path(args.result_json)
        result_path.parent.mkdir(parents=True, exist_ok=True)
        result_path.write_text(json.dumps({**payload, "trades": trade_rows}, indent=2), encoding="utf-8")
    print(json.dumps(payload, indent=2))


if __name__ == "__main__":
    main()
