"""
Parameter sweep runner for the current weather NO-EV backtest.

This script intentionally reuses analysis.phase1_backtest.run_backtest so every
combo exercises the same trading logic as ordinary backtests. Results are kept
out of SQLite and compact CSV/Markdown summaries are written for ranking.
"""

from __future__ import annotations

import argparse
import csv
import itertools
import json
import logging
import os
import sys
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from typing import Iterable


POLY_WEATHER = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(POLY_WEATHER))

from analysis.phase1_backtest import (  # noqa: E402
    DEFAULT_CALIBRATION_PATH,
    DEFAULT_INTERVAL_MINUTES,
    DEFAULT_STRATEGIES_PATH,
    _load_calibration_table,
    run_backtest,
)
from analysis.strategies import group_members, load_strategy_params  # noqa: E402
from config.cities import CITIES  # noqa: E402
from data.db import Database  # noqa: E402


log = logging.getLogger(__name__)

_WORKER_DB: Database | None = None
_WORKER_CALIBRATION_TABLE: dict | None = None
_WORKER_DATE_FROM: date | None = None
_WORKER_DATE_TO: date | None = None
_WORKER_CITY: str | None = None
_WORKER_STRATEGY_PREFIX: str | None = None
_WORKER_RANK_BY: str | None = None
_WORKER_MIN_TRADES: int | None = None

DEFAULT_OUT_DIR = Path(__file__).resolve().parent / "results"
FORECAST_SOURCES_SQL = "'gfs-global-24h','open-meteo','open-meteo-historical'"

PRESETS = {
    "quick": {
        "min_ev_thresholds": "0.02:0.08:0.02",
        "desired_shares": "1:10:1",
    },
    "full": {
        "min_ev_thresholds": "0.01:0.10:0.01",
        "desired_shares": "1:10:1",
    },
    "exhaustive": {
        "min_ev_thresholds": "0.00:0.15:0.01",
        "desired_shares": "1:10:1",
    },
}

BACKTEST_READY_STRATEGIES = [
    "base_no_ev",
    "base_yes_no_ev",
    "ensemble_bucket_ev",
    "dynamic_sigma_ev",
    "bayesian_climatology_ev",
    "corridor_ladder",
    "nowcast_metar",
    "threshold_union",
    "basket_arb",
    "maker_quote_sim",
]

STRATEGY_EXTRA_GRIDS = {
    "quick": {
        "ensemble_bucket_ev": {
            "ensemble_sigma_floor": [0.8, 1.2, 1.6],
        },
        "dynamic_sigma_ev": {
            "dynamic_sigma_min": [0.8, 1.0],
            "spread_sigma_multiplier": [0.5],
        },
        "bayesian_climatology_ev": {
            "model_weight": [0.6, 0.8],
        },
        "corridor_ladder": {
            "corridor_min_edge": [0.02, 0.05],
            "max_corridor_width": [2, 3],
        },
        "threshold_union": {
        },
        "basket_arb": {
            "arb_buffer": [0.0, 0.005, 0.01],
        },
        "maker_quote_sim": {
            "tick_size": [0.01, 0.02],
            "maker_proxy_from_price_history": [1.0],
        },
    },
    "full": {
        "ensemble_bucket_ev": {
            "ensemble_sigma_floor": [0.6, 0.8, 1.0, 1.2, 1.4, 1.6, 2.0],
        },
        "dynamic_sigma_ev": {
            "dynamic_sigma_min": [0.6, 0.8, 1.0, 1.2],
            "dynamic_sigma_max": [4.0, 5.0, 6.0],
            "spread_sigma_multiplier": [0.25, 0.5, 0.75, 1.0],
        },
        "bayesian_climatology_ev": {
            "model_weight": [0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
        },
        "corridor_ladder": {
            "corridor_min_edge": [0.0, 0.01, 0.02, 0.03, 0.04, 0.05],
            "max_corridor_width": [2, 3, 4, 5],
        },
        "threshold_union": {
        },
        "basket_arb": {
            "arb_buffer": [0.0, 0.0025, 0.005, 0.0075, 0.01, 0.02],
        },
        "maker_quote_sim": {
            "tick_size": [0.005, 0.01, 0.015, 0.02, 0.03],
            "maker_proxy_from_price_history": [0.0, 1.0],
        },
    },
    "exhaustive": {
        "ensemble_bucket_ev": {
            "ensemble_sigma_floor": [0.4, 0.6, 0.8, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0],
        },
        "dynamic_sigma_ev": {
            "dynamic_sigma_min": [0.4, 0.6, 0.8, 1.0, 1.2, 1.4, 1.6],
            "dynamic_sigma_max": [3.0, 4.0, 5.0, 6.0, 7.0],
            "spread_sigma_multiplier": [0.0, 0.25, 0.5, 0.75, 1.0, 1.25],
        },
        "bayesian_climatology_ev": {
            "model_weight": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
        },
        "corridor_ladder": {
            "corridor_min_edge": [0.0, 0.01, 0.02, 0.03, 0.04, 0.05, 0.075, 0.10],
            "max_corridor_width": [2, 3, 4, 5, 6],
        },
        "threshold_union": {
        },
        "basket_arb": {
            "arb_buffer": [0.0, 0.0025, 0.005, 0.0075, 0.01, 0.015, 0.02, 0.03],
        },
        "maker_quote_sim": {
            "tick_size": [0.005, 0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.05],
            "maker_proxy_from_price_history": [0.0, 1.0],
        },
    },
}


def parse_float_list(raw: str) -> list[float]:
    """Parse comma values and inclusive ranges like 0.01:0.10:0.01."""
    values: list[float] = []
    for item in raw.split(","):
        item = item.strip()
        if not item:
            continue
        if ":" in item:
            parts = item.split(":")
            if len(parts) != 3:
                raise argparse.ArgumentTypeError("ranges must be START:STOP:STEP")
            start, stop, step = (float(part) for part in parts)
            if step == 0:
                raise argparse.ArgumentTypeError("range step cannot be zero")
            current = start
            epsilon = abs(step) / 1_000_000
            if step > 0:
                while current <= stop + epsilon:
                    values.append(round(current, 10))
                    current += step
            else:
                while current >= stop - epsilon:
                    values.append(round(current, 10))
                    current += step
        else:
            values.append(float(item))
    if not values:
        raise argparse.ArgumentTypeError("expected at least one comma-separated float")
    deduped = []
    seen = set()
    for value in values:
        key = round(value, 10)
        if key not in seen:
            seen.add(key)
            deduped.append(value)
    return deduped


def parse_key_float_list(raw: str) -> tuple[str, list[float]]:
    if "=" not in raw:
        raise argparse.ArgumentTypeError("expected KEY=v1,v2")
    key, values = raw.split("=", 1)
    key = key.strip()
    if not key:
        raise argparse.ArgumentTypeError("parameter key cannot be empty")
    return key, parse_float_list(values)


def parse_strategy_key_float_list(raw: str) -> tuple[str, str, list[float]]:
    if ":" not in raw or "=" not in raw:
        raise argparse.ArgumentTypeError("expected STRATEGY:KEY=v1,v2 or STRATEGY:KEY=start:stop:step")
    strategy, rest = raw.split(":", 1)
    key, values = parse_key_float_list(rest)
    strategy = strategy.strip()
    if not strategy:
        raise argparse.ArgumentTypeError("strategy cannot be empty")
    return strategy, key, values


def strategy_names(args: argparse.Namespace) -> list[str]:
    strategies_file = Path(args.strategies_file)
    if args.all_strategies:
        return group_members("all", strategies_file)
    if args.backtest_ready_strategies:
        return list(BACKTEST_READY_STRATEGIES)
    if args.strategy_group:
        return group_members(args.strategy_group, strategies_file)
    return args.strategies


def _extra_grid_for(strategy_name: str, args: argparse.Namespace) -> dict[str, list[float]]:
    grid = dict(STRATEGY_EXTRA_GRIDS.get(args.preset, {}).get(strategy_name, {}))
    for key, values in args.param_grid:
        grid[key] = values
    for target_strategy, key, values in args.strategy_param_grid:
        if target_strategy == strategy_name:
            grid[key] = values
    return grid


def iter_param_grid(strategy_name: str, args: argparse.Namespace) -> Iterable[dict]:
    common_grid = {
        "min_ev_threshold": args.min_ev_thresholds,
        "desired_shares": args.desired_shares,
    }
    extra_grid = _extra_grid_for(strategy_name, args)
    for key in extra_grid:
        common_grid.pop(key, None)

    grid = {**common_grid, **extra_grid}
    keys = list(grid)
    value_sets = [grid[key] for key in keys]
    base_params = load_strategy_params(strategy_name, Path(args.strategies_file))

    for values in itertools.product(*value_sets):
        params = dict(base_params)
        params.pop("funding_pct", None)
        params.update(dict(zip(keys, values, strict=True)))
        fixed_trade_stake = args.fixed_trade_stake_usd
        params.update(
            {
                "strategy_id": strategy_name,
                "max_stake_usd": args.daily_max_stake_usd,
                "fixed_trade_stake_usd": fixed_trade_stake,
                "fee_rate": args.fee_rate,
                "sweep_name": args.sweep_name,
            }
        )
        yield params


def apply_preset(args: argparse.Namespace) -> None:
    preset = PRESETS[args.preset]
    if args.min_ev_thresholds is None:
        args.min_ev_thresholds = parse_float_list(preset["min_ev_thresholds"])
    if args.desired_shares is None:
        args.desired_shares = parse_float_list(preset["desired_shares"])
    if args.max_stake_usd is not None and args.daily_max_stake_usd is None:
        args.daily_max_stake_usd = args.max_stake_usd[0]


def available_dates(
    db: Database,
    city: str,
) -> list[str]:
    date_rows = db.conn.execute(
        """
        SELECT target_date
        FROM settlement_records
        WHERE city = ? AND winning_bin IS NOT NULL
        ORDER BY target_date
        """,
        (city,),
    ).fetchall()

    dates = []
    for row in date_rows:
        target_date = row["target_date"]
        forecast_count = db.conn.execute(
            f"""
            SELECT COUNT(*) AS c
            FROM forecast_records
            WHERE city = ? AND target_date = ?
              AND source IN ({FORECAST_SOURCES_SQL})
              AND (temp_max_gfs_f IS NOT NULL OR temp_max_ecmwf_f IS NOT NULL)
            """,
            (city, target_date),
        ).fetchone()["c"]
        if forecast_count == 0:
            continue

        price_bins = db.conn.execute(
            """
            SELECT COUNT(DISTINCT bin_label) AS c
            FROM price_history_1m
            WHERE city = ? AND target_date = ?
              AND hours_to_close > 0 AND hours_to_close <= 36
            """,
            (city, target_date),
        ).fetchone()["c"]

        if price_bins > 0:
            dates.append(target_date)

    return dates


def metric_value(row: dict, metric: str, min_trades: int) -> float | None:
    trades = int(row.get("total_trades") or 0)
    if trades < min_trades:
        return None
    if metric == "roi":
        stake = float(row.get("total_stake") or 0.0)
        if stake <= 0:
            return None
        return float(row.get("total_pnl") or 0.0) / stake
    return float(row.get(metric) or 0.0)


def sort_score(row: dict) -> float:
    score = row.get("rank_score")
    return float(score) if score is not None else float("-inf")


def enrich_summary(summary: dict, combo_index: int, params: dict, metric: str, min_trades: int) -> dict:
    total_pnl = float(summary.get("total_pnl") or 0.0)
    total_stake = float(summary.get("total_stake") or 0.0)
    roi = total_pnl / total_stake if total_stake > 0 else None

    row = {
        "combo_index": combo_index,
        "backtest_id": summary.get("backtest_id"),
        "strategy": summary.get("strategy"),
        "strategy_id": params.get("strategy_id"),
        "strategy_plugin": summary.get("strategy_plugin"),
        "city": summary.get("city"),
        "date_from": summary.get("date_from"),
        "date_to": summary.get("date_to"),
        "replay_mode": params.get("replay_mode"),
        "interval_minutes": params.get("interval_minutes"),
        "min_ev_threshold": params.get("min_ev_threshold"),
        "desired_shares": params.get("desired_shares"),
        "max_stake_usd": params.get("max_stake_usd"),
        "fixed_trade_stake_usd": params.get("fixed_trade_stake_usd"),
        "fee_rate": params.get("fee_rate"),
        "total_trades": summary.get("total_trades"),
        "wins": summary.get("wins"),
        "win_rate": summary.get("win_rate"),
        "total_pnl": total_pnl,
        "total_stake": total_stake,
        "roi": roi,
        "sharpe": summary.get("sharpe"),
        "diagnostics": json.dumps(summary.get("diagnostics") or {}, sort_keys=True),
        "rank_metric": metric,
        "rank_score": None,
        "params_json": json.dumps(params, sort_keys=True),
    }
    for key, value in sorted(params.items()):
        if isinstance(value, (str, int, float, bool)) or value is None:
            row[f"param.{key}"] = value
    row["rank_score"] = metric_value(row, metric, min_trades)
    return row


def _init_worker(
    db_path: str | None,
    calibration_table: dict,
    date_from_iso: str,
    date_to_iso: str,
    city: str,
    strategy_prefix: str,
    rank_by: str,
    min_trades: int,
) -> None:
    global _WORKER_DB
    global _WORKER_CALIBRATION_TABLE
    global _WORKER_DATE_FROM
    global _WORKER_DATE_TO
    global _WORKER_CITY
    global _WORKER_STRATEGY_PREFIX
    global _WORKER_RANK_BY
    global _WORKER_MIN_TRADES

    logging.getLogger("analysis.phase1_backtest").setLevel(logging.WARNING)
    logging.getLogger("data.db").setLevel(logging.WARNING)
    _WORKER_DB = Database(read_only=True) if db_path is None else Database(Path(db_path), read_only=True)
    _WORKER_CALIBRATION_TABLE = calibration_table
    _WORKER_DATE_FROM = date.fromisoformat(date_from_iso)
    _WORKER_DATE_TO = date.fromisoformat(date_to_iso)
    _WORKER_CITY = city
    _WORKER_STRATEGY_PREFIX = strategy_prefix
    _WORKER_RANK_BY = rank_by
    _WORKER_MIN_TRADES = min_trades


def _run_combo_worker(task: tuple[int, int, str, dict]) -> dict:
    combo_index, combo_count, strategy_id, params = task
    if (
        _WORKER_DB is None
        or _WORKER_CALIBRATION_TABLE is None
        or _WORKER_DATE_FROM is None
        or _WORKER_DATE_TO is None
        or _WORKER_CITY is None
        or _WORKER_STRATEGY_PREFIX is None
        or _WORKER_RANK_BY is None
        or _WORKER_MIN_TRADES is None
    ):
        raise RuntimeError("parameter sweep worker was not initialized")

    params = dict(params)
    params.update(
        {
            "replay_mode": "interval",
            "interval_minutes": DEFAULT_INTERVAL_MINUTES,
        }
    )
    strategy_name = f"{_WORKER_STRATEGY_PREFIX}_{strategy_id}_{combo_index:04d}"
    summary = run_backtest(
        db=_WORKER_DB,
        strategy_name=strategy_name,
        params=params,
        date_from=_WORKER_DATE_FROM,
        date_to=_WORKER_DATE_TO,
        city=_WORKER_CITY,
        calibration_table=_WORKER_CALIBRATION_TABLE,
        include_trades=False,
    )
    row = enrich_summary(summary, combo_index, params, _WORKER_RANK_BY, _WORKER_MIN_TRADES)
    row["combo_count"] = combo_count
    return row


def write_csv(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if not rows:
        path.write_text("")
        return
    preferred = list(rows[0].keys())
    remaining = sorted({key for row in rows for key in row} - set(preferred))
    fieldnames = preferred + remaining
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def format_pct(value: object) -> str:
    if value is None:
        return ""
    return f"{float(value) * 100:.2f}%"


def format_float(value: object, digits: int = 4) -> str:
    if value is None:
        return ""
    return f"{float(value):.{digits}f}"


def compact_params(row: dict, max_items: int = 5) -> str:
    raw = row.get("params_json")
    if not raw:
        return ""
    try:
        params = json.loads(raw)
    except (TypeError, ValueError, json.JSONDecodeError):
        return str(raw)
    skip = {"strategy_id", "sweep_name", "replay_mode", "interval_minutes", "fee_rate", "funding_pct"}
    items = []
    for key, value in sorted(params.items()):
        if key in skip or value is None:
            continue
        items.append(f"{key}={value}")
    return ", ".join(items[:max_items])


def write_report(path: Path, rows: list[dict], meta: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    top = rows[:20]
    lines = [
        f"# Parameter Sweep Report: {meta['sweep_name']}",
        "",
        "## Scope",
        "",
        f"- City: `{meta['city']}`",
        f"- Date range: `{meta['date_from']}` to `{meta['date_to']}`",
        f"- Available dates used: `{meta['available_date_count']}`",
        f"- Replay: `1-minute interval`",
        f"- Strategies: `{meta['strategies']}`",
        f"- Parameter combinations: `{meta['combos']}`",
        f"- Parallel jobs: `{meta['jobs']}`",
        f"- Rank metric: `{meta['rank_by']}`",
        f"- Minimum trades for ranking: `{meta['min_trades']}`",
        f"- CSV: `{meta['csv']}`",
        "",
        "## Grid",
        "",
        f"- `min_ev_threshold`: `{meta['min_ev_thresholds']}`",
        f"- `desired_shares`: `{meta['desired_shares']}`",
        f"- `funding_pct`: not used; `desired_shares` controls size directly",
        f"- `max_stake_usd` daily cap: `{meta['daily_max_stake_usd']}`",
        f"- `fixed_trade_stake_usd`: `{meta['fixed_trade_stake_usd']}`",
        f"- `fee_rate`: `{meta['fee_rate']}`",
        "",
        "## Top Results",
        "",
        "| Rank | Strategy | Backtest ID | ROI | PnL | Stake | Trades | Win% | Sharpe | Key Params |",
        "|---:|---|---|---:|---:|---:|---:|---:|---:|---|",
    ]

    for rank, row in enumerate(top, start=1):
        lines.append(
            "| {rank} | `{strategy}` | `{backtest_id}` | {roi} | {pnl} | {stake} | {trades} | {win_rate} | {sharpe} | {params} |".format(
                rank=rank,
                strategy=row.get("strategy_id") or row.get("strategy") or "",
                backtest_id=row.get("backtest_id") or "",
                roi=format_pct(row.get("roi")),
                pnl=format_float(row.get("total_pnl")),
                stake=format_float(row.get("total_stake")),
                trades=row.get("total_trades") or 0,
                win_rate=format_pct(row.get("win_rate")),
                sharpe=format_float(row.get("sharpe")),
                params=compact_params(row),
            )
        )

    lines.extend(
        [
            "",
            "## Notes",
            "",
            "- Rows below the minimum trade count receive a blank `rank_score` and sort below eligible rows.",
            "- This report ranks the tested period only. Keep a separate validation period before promoting parameters.",
            "- Backtest results are file-based; SQLite is used only as the historical data source.",
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


def best_row(rows: list[dict]) -> dict | None:
    eligible = [row for row in rows if row.get("rank_score") is not None]
    if eligible:
        return max(eligible, key=sort_score)
    if not rows:
        return None
    return max(
        rows,
        key=lambda row: (
            float(row.get("total_pnl") or 0.0),
            float(row.get("total_stake") or 0.0),
            int(row.get("total_trades") or 0),
        ),
    )


def write_strategy_artifacts(out_dir: Path, sweep_name: str, rows: list[dict], meta: dict) -> dict[str, dict[str, str]]:
    grouped: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        grouped[row.get("strategy_id") or row.get("strategy") or "unknown"].append(row)

    artifacts: dict[str, dict[str, str]] = {}
    for strategy_id, strategy_rows in sorted(grouped.items()):
        ranked = sorted(strategy_rows, key=sort_score, reverse=True)
        strategy_stem = f"{sweep_name}__{strategy_id}"
        csv_path = out_dir / f"{strategy_stem}.csv"
        report_path = out_dir / f"{strategy_stem}.md"
        write_csv(csv_path, ranked)
        strategy_meta = dict(meta)
        strategy_meta.update(
            {
                "sweep_name": f"{sweep_name} / {strategy_id}",
                "strategies": strategy_id,
                "combos": len(strategy_rows),
                "csv": str(csv_path),
                "report": str(report_path),
            }
        )
        write_report(report_path, ranked, strategy_meta)
        artifacts[strategy_id] = {"csv": str(csv_path), "report": str(report_path)}
    return artifacts


def write_final_strategy_report(path: Path, best_rows: list[dict], meta: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    ranked = sorted(best_rows, key=sort_score, reverse=True)
    lines = [
        f"# Final Strategy Comparison: {meta['sweep_name']}",
        "",
        "## Scope",
        "",
        f"- City: `{meta['city']}`",
        f"- Date range: `{meta['date_from']}` to `{meta['date_to']}`",
        f"- Replay: `1-minute interval`",
        f"- Strategies compared: `{len(best_rows)}`",
        f"- Parallel jobs: `{meta['jobs']}`",
        f"- Ranking metric: `{meta['rank_by']}`",
        f"- Minimum trades for ranking: `{meta['min_trades']}`",
        f"- Full sweep CSV: `{meta['csv']}`",
        "",
        "## Best Parameters By Strategy",
        "",
        "| Rank | Strategy | ROI | PnL | Stake | Trades | Win% | Sharpe | Backtest ID | Best Params |",
        "|---:|---|---:|---:|---:|---:|---:|---:|---|---|",
    ]
    for rank, row in enumerate(ranked, start=1):
        lines.append(
            "| {rank} | `{strategy}` | {roi} | {pnl} | {stake} | {trades} | {win_rate} | {sharpe} | `{backtest_id}` | {params} |".format(
                rank=rank,
                strategy=row.get("strategy_id") or row.get("strategy") or "",
                roi=format_pct(row.get("roi")),
                pnl=format_float(row.get("total_pnl")),
                stake=format_float(row.get("total_stake")),
                trades=row.get("total_trades") or 0,
                win_rate=format_pct(row.get("win_rate")),
                sharpe=format_float(row.get("sharpe")),
                backtest_id=row.get("backtest_id") or "",
                params=compact_params(row, max_items=10),
            )
        )
    lines.extend(
        [
            "",
            "## Notes",
            "",
            "- This report keeps only the best-ranked parameter set from each strategy, then compares those winners.",
            "- Strategies with no row meeting `min_trades` are included with blank rank scores and sort below eligible strategies.",
            "- Per-strategy CSV and Markdown reports are written next to this file.",
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


def positive_int(raw: str) -> int:
    value = int(raw)
    if value <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Grid-search current backtest strategy parameters and rank the results."
    )
    parser.add_argument("--sweep-name", default=None, help="Name stored in params and used in output filename")
    parser.add_argument("--strategy", dest="strategies", action="append", default=None,
                        help="Strategy plugin to sweep; repeat for multiple strategies")
    parser.add_argument("--strategy-group", help="Sweep every strategy in a configured group")
    parser.add_argument("--all-strategies", action="store_true",
                        help="Sweep every configured strategy plugin, including research placeholders")
    parser.add_argument("--backtest-ready-strategies", action="store_true",
                        help="Sweep only plugins with executable backtest logic")
    parser.add_argument("--strategies-file", default=str(DEFAULT_STRATEGIES_PATH),
                        help="Path to strategy JSON config")
    parser.add_argument("--strategy-prefix", default="sweep", help="Strategy name prefix stored per run")
    parser.add_argument("--city", default="NYC", help="City filter, e.g. NYC")
    parser.add_argument(
        "--nyc-all-available",
        action="store_true",
        help="Shortcut: use NYC, all currently backtest-ready NYC dates, and the full preset",
    )
    parser.add_argument("--all-available", action="store_true", help="Use all backtest-ready dates for --city")
    parser.add_argument("--days", type=positive_int, default=60, help="Days of history when --date-from is omitted")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD, default yesterday UTC")
    parser.add_argument("--calibration-file", default=str(DEFAULT_CALIBRATION_PATH))
    parser.add_argument("--db", default=None, help="Path to SQLite DB")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR), help="Directory for CSV summaries")
    parser.add_argument("--rank-by", choices=("roi", "total_pnl", "sharpe", "win_rate"), default="roi")
    parser.add_argument("--min-trades", type=int, default=5, help="Minimum trades required for ranking")
    parser.add_argument("--preset", choices=tuple(PRESETS), default="quick", help="Default parameter grid preset")
    parser.add_argument("--report", action="store_true", help="Deprecated; Markdown reports are always written next to the CSV")
    parser.add_argument("--jobs", type=positive_int, default=1,
                        help="Parallel worker processes for parameter combinations")

    parser.add_argument("--min-ev-thresholds", type=parse_float_list, default=None)
    parser.add_argument("--desired-shares", type=parse_float_list, default=None)
    parser.add_argument("--funding-pcts", type=parse_float_list, default=None,
                        help="Deprecated; ignored. Finetune does not use funding_pct")
    parser.add_argument("--max-stake-usd", type=parse_float_list, default=None,
                        help="Deprecated alias for --daily-max-stake-usd; omitted means uncapped")
    parser.add_argument("--fee-rate", type=float, default=0.05)
    parser.add_argument("--fixed-trade-stake-usd", type=float, default=None,
                        help="Optional fixed stake per candidate; omit to sweep desired_shares directly")
    parser.add_argument("--daily-max-stake-usd", type=float, default=None,
                        help="Optional per-day stake cap; omitted means uncapped")
    parser.add_argument("--param-grid", type=parse_key_float_list, action="append", default=[],
                        help="Extra or override grid for every selected strategy, e.g. --param-grid tick_size=0.01,0.02")
    parser.add_argument("--strategy-param-grid", type=parse_strategy_key_float_list, action="append", default=[],
                        help="Strategy-specific grid override, e.g. --strategy-param-grid dynamic_sigma_ev:dynamic_sigma_min=0.4:1.2:0.2")

    args = parser.parse_args()
    if args.nyc_all_available:
        args.city = "NYC"
        args.all_available = True
        args.preset = "full"
    explicit_strategies = args.strategies is not None
    if not args.strategies:
        args.strategies = ["base_no_ev"]
    selected_modes = sum(bool(v) for v in (args.all_strategies, args.backtest_ready_strategies, args.strategy_group, explicit_strategies))
    if selected_modes > 1:
        parser.error("use only one of --strategy, --strategy-group, --all-strategies, or --backtest-ready-strategies")
    city = args.city.upper()
    if city and city not in CITIES:
        parser.error(f"--city must be one of: {', '.join(sorted(CITIES))}")
    args.city = city
    apply_preset(args)
    if args.daily_max_stake_usd is not None and args.daily_max_stake_usd <= 0:
        args.daily_max_stake_usd = None
    if args.fixed_trade_stake_usd is not None and args.fixed_trade_stake_usd <= 0:
        parser.error("--fixed-trade-stake-usd must be positive")
    if not args.sweep_name:
        args.sweep_name = f"{args.city.lower()}_{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')}"
    cpu_count = os.cpu_count() or 1
    if args.jobs > cpu_count:
        args.jobs = cpu_count
    return args


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s - %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )
    logging.getLogger("analysis.phase1_backtest").setLevel(logging.WARNING)
    args = parse_args()

    calibration_table = _load_calibration_table(Path(args.calibration_file))
    db = Database(read_only=True) if not args.db else Database(Path(args.db), read_only=True)
    available: list[str] = []
    today = datetime.now(timezone.utc).date()
    date_to = date.fromisoformat(args.date_to) if args.date_to else today - timedelta(days=1)
    date_from = date.fromisoformat(args.date_from) if args.date_from else date_to - timedelta(days=args.days - 1)

    if args.all_available:
        available = available_dates(db, args.city)
        if not available:
            db.close()
            raise SystemExit(f"No backtest-ready dates found for {args.city}")
        date_from = date.fromisoformat(available[0])
        date_to = date.fromisoformat(available[-1])
        log.info(
            "Using all available %s dates: %s to %s (%d dates)",
            args.city,
            date_from,
            date_to,
            len(available),
        )

    selected_strategies = strategy_names(args)
    raw_combos = [
        (strategy_id, params)
        for strategy_id in selected_strategies
        for params in iter_param_grid(strategy_id, args)
    ]
    combos = [
        (combo_index, 0, strategy_id, params)
        for combo_index, (strategy_id, params) in enumerate(raw_combos, start=1)
    ]
    combo_count = len(combos)
    combos = [
        (combo_index, combo_count, strategy_id, params)
        for combo_index, _, strategy_id, params in combos
    ]
    log.info(
        "Running %d parameter combinations across %d strategies for sweep '%s' with %d job(s)",
        combo_count,
        len(selected_strategies),
        args.sweep_name,
        args.jobs,
    )

    rows: list[dict] = []
    db.close()
    db = None

    if args.jobs == 1:
        _init_worker(
            args.db,
            calibration_table,
            date_from.isoformat(),
            date_to.isoformat(),
            args.city,
            args.strategy_prefix,
            args.rank_by,
            args.min_trades,
        )
        try:
            for task in combos:
                combo_index, _, strategy_id, params = task
                log.info("Combo %d/%d (%s): %s", combo_index, combo_count, strategy_id, params)
                rows.append(_run_combo_worker(task))
        finally:
            if _WORKER_DB is not None:
                _WORKER_DB.close()
    else:
        with ProcessPoolExecutor(
            max_workers=args.jobs,
            initializer=_init_worker,
            initargs=(
                args.db,
                calibration_table,
                date_from.isoformat(),
                date_to.isoformat(),
                args.city,
                args.strategy_prefix,
                args.rank_by,
                args.min_trades,
            ),
        ) as executor:
            futures = {
                executor.submit(_run_combo_worker, task): task
                for task in combos
            }
            completed = 0
            for future in as_completed(futures):
                combo_index, _, strategy_id, _ = futures[future]
                rows.append(future.result())
                completed += 1
                if completed == 1 or completed % 25 == 0 or completed == combo_count:
                    log.info(
                        "Completed %d/%d combinations; latest combo %d (%s)",
                        completed,
                        combo_count,
                        combo_index,
                        strategy_id,
                    )

    ranked = sorted(rows, key=sort_score, reverse=True)
    out_dir = Path(args.out_dir)
    out_path = out_dir / f"{args.sweep_name}.csv"
    report_path = out_dir / f"{args.sweep_name}.md"
    final_report_path = out_dir / f"{args.sweep_name}__best_by_strategy.md"
    write_csv(out_path, ranked)
    meta = {
        "sweep_name": args.sweep_name,
        "city": args.city,
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "available_date_count": len(available) if args.all_available else None,
        "replay_mode": "interval",
        "strategies": ",".join(selected_strategies),
        "combos": len(rows),
        "jobs": args.jobs,
        "rank_by": args.rank_by,
        "min_trades": args.min_trades,
        "csv": str(out_path),
        "report": str(report_path),
        "min_ev_thresholds": ",".join(str(v) for v in args.min_ev_thresholds),
        "desired_shares": ",".join(str(v) for v in args.desired_shares),
        "max_stake_usd": "" if args.max_stake_usd is None else ",".join(str(v) for v in args.max_stake_usd),
        "daily_max_stake_usd": args.daily_max_stake_usd,
        "fixed_trade_stake_usd": args.fixed_trade_stake_usd,
        "fee_rate": args.fee_rate,
    }
    write_report(report_path, ranked, meta)
    strategy_artifacts = write_strategy_artifacts(out_dir, args.sweep_name, rows, meta)
    grouped: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        grouped[row.get("strategy_id") or row.get("strategy") or "unknown"].append(row)
    best_by_strategy = [row for row in (best_row(strategy_rows) for strategy_rows in grouped.values()) if row is not None]
    write_final_strategy_report(final_report_path, best_by_strategy, meta)

    print(json.dumps({
        "sweep_name": args.sweep_name,
        "combos": len(rows),
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "available_date_count": meta["available_date_count"],
        "strategies": selected_strategies,
        "rank_by": args.rank_by,
        "min_trades": args.min_trades,
        "csv": str(out_path),
        "report": str(report_path),
        "strategy_artifacts": strategy_artifacts,
        "final_report": str(final_report_path),
        "best_by_strategy": sorted(best_by_strategy, key=sort_score, reverse=True),
        "top": ranked[:10],
    }, indent=2))


if __name__ == "__main__":
    main()
