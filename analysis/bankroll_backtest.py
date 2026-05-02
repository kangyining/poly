"""Initial-deposit bankroll backtest model.

This module reuses the existing strategy/data replay helpers from
analysis.phase1_backtest, but applies cash, locked-capital, settlement, and
portfolio accounting instead of the turnover model's daily stake cap.
"""

from __future__ import annotations

import argparse
import csv
import json
import logging
import sys
import uuid
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Optional

sys.path.insert(0, str(Path(__file__).parent.parent))

from analysis.calibration import lookup as calibration_lookup
from analysis.phase1_backtest import (
    DEFAULT_CALIBRATION_PATH,
    DEFAULT_INTERVAL_MINUTES,
    DEFAULT_OUTPUT_DIR,
    DEFAULT_STRATEGIES_PATH,
    _backtest_trade_row,
    _build_market_bin,
    _candidate_pnl,
    _climatology_temps,
    _default_event_slug,
    _diagnostic_counts,
    _effective_ask_from_levels,
    _flatten_csv_row,
    _floor_timestamp,
    _forecast_state,
    _load_calibration_table,
    _load_forecast_snapshots,
    _load_interval_price_bars,
    _load_metar_records,
    _load_snapshot_fill_bars,
    _market_close_utc,
    _metar_max_from_prefix,
    _metar_prefix_max,
    _normalize_city_filter,
    _parse_iso_timestamp,
    _select_active_forecast,
    _slug,
    _station_for_city,
)
from analysis.strategies import MarketBin, StrategyContext, get_strategy, load_strategy_params
from analysis.strategies.core import StrategyCandidate, StrategyDiagnostic
from analysis.strategies.registry import resolve_strategy_name
from data.db import Database

log = logging.getLogger(__name__)

DEFAULT_INITIAL_BANKROLL = 500.0
DEFAULT_MIN_TRADE_STAKE = 5.0
BANKROLL_OUTPUT_DIR = DEFAULT_OUTPUT_DIR / "bankroll"


@dataclass(frozen=True)
class ReplayCandidate:
    strategy_name: str
    strategy_plugin: str
    city: str
    target_date: str
    event_slug: str
    bin_label: str
    bin_index: int
    side: str
    opened_at: datetime
    resolved_at: datetime
    desired_stake_usd: float
    desired_shares: float
    pnl: float
    won: bool
    effective_ask: float
    fee_rate: float
    p_model: float | None
    edge: float
    fill_source: str
    token_id: str
    market_id: str
    metadata: dict[str, Any]


@dataclass
class OpenPosition:
    event: ReplayCandidate
    allocated_stake_usd: float
    allocated_shares: float
    pnl: float
    scale_factor: float


def _fallback_resolved_at(target_date: str) -> datetime:
    target = date.fromisoformat(target_date)
    return datetime(target.year, target.month, target.day, 12, 0, tzinfo=timezone.utc) + timedelta(days=1)


def _settlement_resolved_at(row) -> datetime:
    parsed = _parse_iso_timestamp(row["resolved_at"] if "resolved_at" in row.keys() else None)
    return parsed or _fallback_resolved_at(row["target_date"])


def _settlement_rows(
    db: Database,
    date_from: date,
    date_to: date,
    city_filter: str | None,
) -> list:
    sql = """
        SELECT city, target_date, winning_bin, event_slug, resolved_at
        FROM settlement_records
        WHERE target_date >= ? AND target_date <= ?
    """
    params: list[object] = [date_from.isoformat(), date_to.isoformat()]
    if city_filter:
        sql += " AND city = ?"
        params.append(city_filter)
    sql += " ORDER BY target_date, city"
    return db.conn.execute(sql, params).fetchall()


def generate_replay_candidates(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: str | None = None,
    calibration_table: Optional[dict] = None,
) -> tuple[list[ReplayCandidate], dict[str, Any]]:
    """Generate strategy candidates with the same data path as interval backtests."""
    city_filter = city
    interval_minutes = DEFAULT_INTERVAL_MINUTES
    desired_shares = float(params.get("desired_shares", 10.0)) * float(params.get("funding_pct", 1.0))
    fee_rate = float(params.get("fee_rate", 0.05))
    strategy = get_strategy(strategy_name, params)
    resolved_strategy = resolve_strategy_name(strategy_name, params)
    calibration_table = calibration_table or {"cells": {}, "_defaults": {}}

    candidates: list[ReplayCandidate] = []
    days_considered = 0
    days_with_forecast = 0
    days_with_price_history = 0
    days_with_minute_price_history = 0
    days_with_snapshot_fills = 0
    days_missing_forecast = 0
    days_missing_price_history = 0
    interval_bars_considered = 0
    intervals_with_candidates = 0
    forecast_switches = 0
    diagnostics: list[StrategyDiagnostic] = []

    for row in _settlement_rows(db, date_from, date_to, city_filter):
        settlement_city = row["city"]
        target_date = row["target_date"]
        winning_bin = row["winning_bin"]
        event_slug = row["event_slug"] or _default_event_slug(settlement_city, target_date)
        resolved_at = _settlement_resolved_at(row)
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

        snapshot_fills = _load_snapshot_fill_bars(db, settlement_city, target_date, interval_minutes, desired_shares)
        station_code = _station_for_city(settlement_city)
        target_dt = date.fromisoformat(target_date)
        metar_records = _load_metar_records(db, settlement_city, target_date)
        metar_times, metar_maxes = _metar_prefix_max(metar_records)
        climatology_temps = _climatology_temps(db, settlement_city, target_date)

        opened_bins: set[str] = set()
        used_snapshot_fill = False
        last_forecast_key = None

        for bucket_ts, bars in sorted(price_bars.items()):
            if not bars:
                continue
            interval_bars_considered += 1
            reference_market = next(iter(bars.values()))
            current_lead_hours = float(reference_market["hours_to_close"])
            active_forecast = _select_active_forecast(forecast_snapshots, bucket_ts, current_lead_hours)
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

            market_bins: list[MarketBin] = []
            bin_index_by_label: dict[str, int] = {}
            for bin_label, market in sorted(bars.items(), key=lambda item: item[1]["bin_index"]):
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

                bin_index = int(market["bin_index"] or 0)
                bin_index_by_label[bin_label] = bin_index
                market_bins.append(
                    _build_market_bin(
                        settlement_city,
                        target_date,
                        bin_label,
                        bin_index,
                        float(market["yes_price"]),
                        current_lead_hours,
                        desired_shares,
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

            interval_candidates = 0
            for candidate in output.candidates:
                opened_key = f"{candidate.strategy_name}:{candidate.side}:{candidate.bin_label}"
                if opened_key in opened_bins:
                    continue
                pnl, won = _candidate_pnl(candidate, winning_bin)
                opened_bins.add(opened_key)
                candidates.append(
                    ReplayCandidate(
                        strategy_name=strategy_name,
                        strategy_plugin=resolved_strategy,
                        city=settlement_city,
                        target_date=target_date,
                        event_slug=event_slug,
                        bin_label=candidate.bin_label,
                        bin_index=bin_index_by_label.get(candidate.bin_label, 0),
                        side=candidate.side,
                        opened_at=bucket_ts,
                        resolved_at=resolved_at,
                        desired_stake_usd=float(candidate.stake_usd),
                        desired_shares=float(candidate.shares),
                        pnl=float(pnl),
                        won=won,
                        effective_ask=float(candidate.effective_ask),
                        fee_rate=float(candidate.fee_rate),
                        p_model=candidate.p_model,
                        edge=float(candidate.edge),
                        fill_source=candidate.fill_source,
                        token_id=candidate.token_id,
                        market_id=candidate.market_id,
                        metadata=dict(candidate.metadata),
                    )
                )
                interval_candidates += 1

            if interval_candidates > 0:
                intervals_with_candidates += 1

        if used_snapshot_fill:
            days_with_snapshot_fills += 1

    candidates.sort(
        key=lambda c: (
            c.opened_at,
            c.target_date,
            c.city,
            c.strategy_name,
            c.bin_index,
            c.side,
            c.bin_label,
        )
    )
    diagnostics_summary = {
        "days_considered": days_considered,
        "days_with_forecast": days_with_forecast,
        "days_with_price_history": days_with_price_history,
        "days_with_minute_price_history": days_with_minute_price_history,
        "days_with_snapshot_fills": days_with_snapshot_fills,
        "days_missing_forecast": days_missing_forecast,
        "days_missing_price_history": days_missing_price_history,
        "interval_minutes": interval_minutes,
        "interval_bars_considered": interval_bars_considered,
        "intervals_with_candidates": intervals_with_candidates,
        "forecast_switches": forecast_switches,
        "strategy_plugin": resolved_strategy,
        "diagnostics": _diagnostic_counts(diagnostics),
    }
    return candidates, diagnostics_summary


def _equity(cash: float, open_positions: list[OpenPosition]) -> float:
    return cash + sum(pos.allocated_stake_usd for pos in open_positions)


def _locked(open_positions: list[OpenPosition]) -> float:
    return sum(pos.allocated_stake_usd for pos in open_positions)


def apply_bankroll_ledger(
    candidates: list[ReplayCandidate],
    initial_bankroll: float = DEFAULT_INITIAL_BANKROLL,
    min_trade_stake_usd: float = DEFAULT_MIN_TRADE_STAKE,
    allow_scale_down: bool = True,
) -> dict[str, Any]:
    """Apply cash/locked-capital accounting to chronological replay candidates."""
    cash = float(initial_bankroll)
    open_positions: list[OpenPosition] = []
    trades: list[dict[str, Any]] = []
    skips: list[dict[str, Any]] = []
    equity_curve: list[dict[str, Any]] = []
    realized_pnl = 0.0
    total_stake = 0.0
    trades_scaled = 0
    trades_skipped_insufficient_cash = 0
    peak_equity = float(initial_bankroll)
    max_drawdown = 0.0
    max_locked_capital = 0.0
    max_capital_utilization = 0.0
    utilization_seconds = 0.0
    elapsed_seconds = 0.0
    last_ts: datetime | None = None
    last_utilization = 0.0

    def mark(ts: datetime, event_type: str, city: str = "", target_date: str = "", note: str = "") -> None:
        nonlocal peak_equity, max_drawdown, max_locked_capital, max_capital_utilization
        nonlocal utilization_seconds, elapsed_seconds, last_ts, last_utilization

        if last_ts is not None and ts > last_ts:
            delta = (ts - last_ts).total_seconds()
            utilization_seconds += last_utilization * delta
            elapsed_seconds += delta

        locked_capital = _locked(open_positions)
        eq = cash + locked_capital
        peak_equity = max(peak_equity, eq)
        drawdown = (peak_equity - eq) / peak_equity if peak_equity > 0 else 0.0
        utilization = locked_capital / eq if eq > 0 else 0.0
        max_drawdown = max(max_drawdown, drawdown)
        max_locked_capital = max(max_locked_capital, locked_capital)
        max_capital_utilization = max(max_capital_utilization, utilization)
        equity_curve.append({
            "timestamp": ts.isoformat(),
            "event_type": event_type,
            "city": city,
            "target_date": target_date,
            "cash": round(cash, 6),
            "locked_capital": round(locked_capital, 6),
            "equity": round(eq, 6),
            "drawdown": round(drawdown, 8),
            "capital_utilization": round(utilization, 8),
            "note": note,
        })
        last_ts = ts
        last_utilization = utilization

    def settle_due(ts: datetime) -> None:
        nonlocal cash, realized_pnl
        due = [pos for pos in open_positions if pos.event.resolved_at <= ts]
        for pos in sorted(due, key=lambda p: (p.event.resolved_at, p.event.city, p.event.target_date, p.event.bin_index)):
            cash += pos.allocated_stake_usd + pos.pnl
            realized_pnl += pos.pnl
            open_positions.remove(pos)
            mark(pos.event.resolved_at, "settle", pos.event.city, pos.event.target_date, pos.event.bin_label)

    for event in candidates:
        settle_due(event.opened_at)
        cash_before = cash
        desired_stake = event.desired_stake_usd

        if desired_stake <= 0:
            continue
        if cash >= desired_stake:
            allocated_stake = desired_stake
            scale_factor = 1.0
        elif allow_scale_down and cash >= min_trade_stake_usd:
            allocated_stake = cash
            scale_factor = allocated_stake / desired_stake
            trades_scaled += 1
        else:
            trades_skipped_insufficient_cash += 1
            skips.append({
                "timestamp": event.opened_at.isoformat(),
                "city": event.city,
                "target_date": event.target_date,
                "event_slug": event.event_slug,
                "bin_label": event.bin_label,
                "side": event.side,
                "desired_stake_usd": round(desired_stake, 6),
                "available_cash": round(cash, 6),
                "min_trade_stake_usd": round(min_trade_stake_usd, 6),
                "reason": "insufficient_cash",
            })
            mark(event.opened_at, "skip", event.city, event.target_date, event.bin_label)
            continue

        allocated_shares = event.desired_shares * scale_factor
        scaled_pnl = event.pnl * scale_factor
        cash -= allocated_stake
        position = OpenPosition(
            event=event,
            allocated_stake_usd=allocated_stake,
            allocated_shares=allocated_shares,
            pnl=scaled_pnl,
            scale_factor=scale_factor,
        )
        open_positions.append(position)
        total_stake += allocated_stake
        mark(event.opened_at, "open", event.city, event.target_date, event.bin_label)
        trades.append({
            "strategy_name": event.strategy_name,
            "strategy_plugin": event.strategy_plugin,
            "city": event.city,
            "target_date": event.target_date,
            "event_slug": event.event_slug,
            "bin_label": event.bin_label,
            "bin_index": event.bin_index,
            "side": event.side,
            "entry_price": event.effective_ask,
            "fee_rate": event.fee_rate,
            "desired_stake_usd": round(desired_stake, 6),
            "allocated_stake_usd": round(allocated_stake, 6),
            "stake_usd": round(allocated_stake, 6),
            "desired_shares": round(event.desired_shares, 8),
            "shares": round(allocated_shares, 8),
            "scale_factor": round(scale_factor, 8),
            "pnl": round(scaled_pnl, 6),
            "won": event.won,
            "cash_before": round(cash_before, 6),
            "cash_after": round(cash, 6),
            "equity_after": round(_equity(cash, open_positions), 6),
            "opened_at": event.opened_at.isoformat(),
            "resolved_at": event.resolved_at.isoformat(),
            "settled_at": event.resolved_at.isoformat(),
            "token_id": event.token_id,
            "market_id": event.market_id,
            "fill_source": event.fill_source,
            "p_model": event.p_model,
            "edge": event.edge,
            "metadata": json.dumps(event.metadata, sort_keys=True),
        })

    # Settle everything by the end of the simulation so final equity is realized.
    for pos in sorted(list(open_positions), key=lambda p: (p.event.resolved_at, p.event.city, p.event.target_date, p.event.bin_index)):
        settle_due(pos.event.resolved_at)

    final_locked = _locked(open_positions)
    final_equity = cash + final_locked
    avg_utilization = utilization_seconds / elapsed_seconds if elapsed_seconds > 0 else 0.0
    wins = sum(1 for trade in trades if trade["won"])
    summary = {
        "initial_bankroll": round(float(initial_bankroll), 4),
        "final_equity": round(final_equity, 4),
        "portfolio_return": round((final_equity - float(initial_bankroll)) / float(initial_bankroll), 6)
        if initial_bankroll else None,
        "realized_pnl": round(realized_pnl, 4),
        "open_locked_capital": round(final_locked, 4),
        "max_drawdown": round(max_drawdown, 6),
        "max_locked_capital": round(max_locked_capital, 4),
        "max_capital_utilization": round(max_capital_utilization, 6),
        "avg_capital_utilization": round(avg_utilization, 6),
        "trades_opened": len(trades),
        "trades_scaled": trades_scaled,
        "trades_skipped_insufficient_cash": trades_skipped_insufficient_cash,
        "wins": wins,
        "win_rate": round(wins / len(trades), 4) if trades else 0.0,
        "total_stake": round(total_stake, 4),
        "turnover_yield": round(realized_pnl / total_stake, 6) if total_stake > 0 else None,
    }
    return {
        "summary": summary,
        "trades": trades,
        "skips": skips,
        "equity_curve": equity_curve,
    }


def _auto_bankroll_paths(
    strategy_names: list[str],
    city: str | None,
    date_from: date,
    date_to: date,
) -> dict[str, Path]:
    city_part = _slug(city or "all_cities")
    strategy_part = _slug(strategy_names[0]) if len(strategy_names) == 1 else f"{len(strategy_names)}_strategies"
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    stem = f"{city_part}_{date_from.isoformat()}_{date_to.isoformat()}_bankroll_{strategy_part}_{stamp}"
    return {
        "summary_csv": BANKROLL_OUTPUT_DIR / f"{stem}_summary.csv",
        "summary_json": BANKROLL_OUTPUT_DIR / f"{stem}_summary.json",
        "trades_csv": BANKROLL_OUTPUT_DIR / f"{stem}_trades.csv",
        "skips_csv": BANKROLL_OUTPUT_DIR / f"{stem}_skips.csv",
        "equity_csv": BANKROLL_OUTPUT_DIR / f"{stem}_equity.csv",
        "report": BANKROLL_OUTPUT_DIR / f"{stem}_report.md",
    }


def _write_csv(path: Path, rows: list[dict[str, Any]], preferred: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    flattened = [_flatten_csv_row(row) for row in rows]
    keys = set(preferred)
    keys.update(key for row in flattened for key in row)
    fieldnames = [key for key in preferred if key in keys] + sorted(keys - set(preferred))
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(flattened)


def _write_report(path: Path, summary: dict[str, Any], artifacts: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        f"# Bankroll Backtest Report: {summary.get('city') or 'All Cities'}",
        "",
        "## Scope",
        "",
        f"- Date range: `{summary.get('date_from')}` to `{summary.get('date_to')}`",
        f"- Strategy: `{summary.get('strategy')}` (`{summary.get('strategy_plugin')}`)",
        f"- Initial bankroll: `{summary.get('initial_bankroll')}`",
        f"- Min trade stake: `{summary.get('min_trade_stake_usd')}`",
        f"- Scale down enabled: `{summary.get('allow_scale_down')}`",
        "",
        "## Portfolio Metrics",
        "",
        f"- Final equity: `{summary.get('final_equity')}`",
        f"- Portfolio return: `{summary.get('portfolio_return')}`",
        f"- Realized PnL: `{summary.get('realized_pnl')}`",
        f"- Avg PnL/day: `{summary.get('avg_pnl_per_day')}`",
        f"- Max drawdown: `{summary.get('max_drawdown')}`",
        f"- Max locked capital: `{summary.get('max_locked_capital')}`",
        f"- Max capital utilization: `{summary.get('max_capital_utilization')}`",
        f"- Avg capital utilization: `{summary.get('avg_capital_utilization')}`",
        "",
        "## Trading Metrics",
        "",
        f"- Trades opened: `{summary.get('trades_opened')}`",
        f"- Trades scaled: `{summary.get('trades_scaled')}`",
        f"- Trades skipped insufficient cash: `{summary.get('trades_skipped_insufficient_cash')}`",
        f"- Turnover yield: `{summary.get('turnover_yield')}`",
        "",
        "## Artifacts",
        "",
    ]
    for name, value in artifacts.items():
        lines.append(f"- {name}: `{value}`")
    lines.extend([
        "",
        "## Notes",
        "",
        "- Portfolio return is based on initial deposit and final realized equity.",
        "- Open positions are valued at locked cost basis until settlement.",
        "- Turnover yield remains a secondary efficiency metric, not ROI.",
        "",
    ])
    path.write_text("\n".join(lines), encoding="utf-8")


def run_bankroll_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    city: str | None = None,
    calibration_table: Optional[dict] = None,
    initial_bankroll: float = DEFAULT_INITIAL_BANKROLL,
    min_trade_stake_usd: float = DEFAULT_MIN_TRADE_STAKE,
    allow_scale_down: bool = True,
) -> dict[str, Any]:
    backtest_id = str(uuid.uuid4())
    backtest_days = (date_to - date_from).days + 1
    run_params = dict(params)
    run_params["replay_mode"] = "bankroll"
    run_params["interval_minutes"] = DEFAULT_INTERVAL_MINUTES
    run_params["initial_bankroll"] = initial_bankroll
    run_params["min_trade_stake_usd"] = min_trade_stake_usd
    run_params["allow_scale_down"] = allow_scale_down
    if city:
        run_params["city"] = city

    candidates, replay_summary = generate_replay_candidates(
        db=db,
        strategy_name=strategy_name,
        params=params,
        date_from=date_from,
        date_to=date_to,
        city=city,
        calibration_table=calibration_table,
    )
    ledger = apply_bankroll_ledger(
        candidates,
        initial_bankroll=initial_bankroll,
        min_trade_stake_usd=min_trade_stake_usd,
        allow_scale_down=allow_scale_down,
    )
    summary = {
        "backtest_id": backtest_id,
        "strategy": strategy_name,
        "strategy_plugin": resolve_strategy_name(strategy_name, params),
        "params": run_params,
        "city": city,
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "backtest_days": backtest_days,
        "candidate_count": len(candidates),
        **replay_summary,
        **ledger["summary"],
        "avg_pnl_per_day": round(ledger["summary"]["realized_pnl"] / backtest_days, 4)
        if backtest_days > 0 else None,
        "min_trade_stake_usd": min_trade_stake_usd,
        "allow_scale_down": allow_scale_down,
        "result_storage": "files",
    }
    return {
        "summary": summary,
        "trades": ledger["trades"],
        "skips": ledger["skips"],
        "equity_curve": ledger["equity_curve"],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Run an initial-deposit bankroll backtest")
    parser.add_argument("--strategy", default="base_ev", help="Strategy name")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD (default: yesterday)")
    parser.add_argument("--days", type=int, default=60, help="Days of history if --date-from is omitted")
    parser.add_argument("--city", help="Optional city filter, e.g. NYC or los-angeles")
    parser.add_argument("--db", default=None, help="Path to data.db")
    parser.add_argument("--strategies-file", default=str(DEFAULT_STRATEGIES_PATH), help="Path to strategy JSON config")
    parser.add_argument("--calibration-file", default=str(DEFAULT_CALIBRATION_PATH), help="Path to calibration.json")
    parser.add_argument("--initial-bankroll", type=float, default=DEFAULT_INITIAL_BANKROLL)
    parser.add_argument("--min-trade-stake-usd", type=float, default=DEFAULT_MIN_TRADE_STAKE)
    parser.add_argument("--allow-scale-down", dest="allow_scale_down", action="store_true", help="Allow cash-limited scale down")
    parser.add_argument("--no-scale-down", dest="allow_scale_down", action="store_false", help="Skip instead of scaling cash-limited trades")
    parser.set_defaults(allow_scale_down=True)
    parser.add_argument("--csv", dest="summary_csv", help="Custom path for summary CSV")
    parser.add_argument("--json", dest="summary_json", help="Custom path for summary JSON")
    parser.add_argument("--trades-csv", help="Custom path for trade-level CSV")
    parser.add_argument("--skips-csv", help="Custom path for skipped-candidates CSV")
    parser.add_argument("--equity-csv", help="Custom path for equity curve CSV")
    parser.add_argument("--report", help="Custom path for Markdown report")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s — %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )

    today = datetime.now(timezone.utc).date()
    date_to = date.fromisoformat(args.date_to) if args.date_to else today - timedelta(days=1)
    date_from = date.fromisoformat(args.date_from) if args.date_from else date_to - timedelta(days=args.days - 1)
    city = _normalize_city_filter(args.city)
    strategies_file = Path(args.strategies_file)
    params = load_strategy_params(args.strategy, strategies_file)
    calibration_table = _load_calibration_table(Path(args.calibration_file))
    db = Database(read_only=True) if not args.db else Database(Path(args.db), read_only=True)

    auto_paths = _auto_bankroll_paths([args.strategy], city, date_from, date_to)
    summary_csv = Path(args.summary_csv) if args.summary_csv else auto_paths["summary_csv"]
    summary_json = Path(args.summary_json) if args.summary_json else auto_paths["summary_json"]
    trades_csv = Path(args.trades_csv) if args.trades_csv else auto_paths["trades_csv"]
    skips_csv = Path(args.skips_csv) if args.skips_csv else auto_paths["skips_csv"]
    equity_csv = Path(args.equity_csv) if args.equity_csv else auto_paths["equity_csv"]
    report = Path(args.report) if args.report else auto_paths["report"]

    try:
        result = run_bankroll_backtest(
            db=db,
            strategy_name=args.strategy,
            params=params,
            date_from=date_from,
            date_to=date_to,
            city=city,
            calibration_table=calibration_table,
            initial_bankroll=args.initial_bankroll,
            min_trade_stake_usd=args.min_trade_stake_usd,
            allow_scale_down=args.allow_scale_down,
        )
    finally:
        db.close()

    summary = result["summary"]
    artifacts = {
        "summary_csv": str(summary_csv),
        "summary_json": str(summary_json),
        "trades_csv": str(trades_csv),
        "skips_csv": str(skips_csv),
        "equity_csv": str(equity_csv),
        "report": str(report),
    }
    summary["artifacts"] = artifacts
    _write_csv(summary_csv, [summary], [
        "backtest_id", "strategy", "strategy_plugin", "city", "date_from", "date_to",
        "backtest_days", "initial_bankroll", "final_equity", "portfolio_return",
        "realized_pnl", "avg_pnl_per_day",
        "max_drawdown", "max_locked_capital", "max_capital_utilization",
        "avg_capital_utilization", "trades_opened", "trades_scaled",
        "trades_skipped_insufficient_cash", "total_stake", "turnover_yield",
        "candidate_count", "days_considered", "days_with_forecast", "days_with_price_history",
    ])
    summary_json.parent.mkdir(parents=True, exist_ok=True)
    summary_json.write_text(json.dumps(summary, indent=2, sort_keys=True), encoding="utf-8")
    _write_csv(trades_csv, result["trades"], [
        "strategy_name", "strategy_plugin", "city", "target_date", "event_slug", "bin_label",
        "side", "desired_stake_usd", "allocated_stake_usd", "scale_factor",
        "cash_before", "cash_after", "equity_after", "pnl", "won", "opened_at", "resolved_at",
    ])
    _write_csv(skips_csv, result["skips"], [
        "timestamp", "city", "target_date", "event_slug", "bin_label", "side",
        "desired_stake_usd", "available_cash", "min_trade_stake_usd", "reason",
    ])
    _write_csv(equity_csv, result["equity_curve"], [
        "timestamp", "event_type", "city", "target_date", "cash", "locked_capital",
        "equity", "drawdown", "capital_utilization", "note",
    ])
    _write_report(report, summary, artifacts)
    log.info("Bankroll backtest complete: %s", summary)
    print(json.dumps(summary, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
