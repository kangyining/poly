"""
Parameter sweep runner for the current weather NO-EV backtest.

This script intentionally reuses analysis.phase1_backtest.run_backtest so every
combo exercises the same trading logic as ordinary backtests. Results are still
persisted to SQLite's backtests/positions tables, and compact CSV/Markdown
summaries are written for ranking.
"""

from __future__ import annotations

import argparse
import csv
import itertools
import json
import logging
import sys
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from typing import Iterable


POLY_WEATHER = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(POLY_WEATHER))

from analysis.phase1_backtest import (  # noqa: E402
    DEFAULT_CALIBRATION_PATH,
    DEFAULT_INTERVAL_MINUTES,
    DEFAULT_LEAD_MAX,
    DEFAULT_LEAD_MIN,
    DEFAULT_REPLAY_MODE,
    SUPPORTED_REPLAY_MODES,
    _load_calibration_table,
    run_backtest,
)
from config.cities import CITIES  # noqa: E402
from data.db import Database  # noqa: E402


log = logging.getLogger(__name__)

DEFAULT_OUT_DIR = Path(__file__).resolve().parent / "results"
FORECAST_SOURCES_SQL = "'gfs-global-24h','open-meteo','open-meteo-historical'"

PRESETS = {
    "quick": {
        "min_ev_thresholds": "0.02,0.03,0.05",
        "desired_shares": "5,10",
        "funding_pcts": "1.0",
        "max_stake_usd": "25,50",
    },
    "full": {
        "min_ev_thresholds": "0.01,0.02,0.03,0.04,0.05,0.06,0.08,0.10",
        "desired_shares": "2.5,5,10,15",
        "funding_pcts": "0.5,1.0",
        "max_stake_usd": "25,50,100,200",
    },
}


def parse_float_list(raw: str) -> list[float]:
    values = []
    for item in raw.split(","):
        item = item.strip()
        if not item:
            continue
        values.append(float(item))
    if not values:
        raise argparse.ArgumentTypeError("expected at least one comma-separated float")
    return values


def iter_param_grid(args: argparse.Namespace) -> Iterable[dict]:
    keys = (
        "min_ev_threshold",
        "desired_shares",
        "funding_pct",
        "max_stake_usd",
    )
    value_sets = (
        args.min_ev_thresholds,
        args.desired_shares,
        args.funding_pcts,
        args.max_stake_usd,
    )

    for values in itertools.product(*value_sets):
        params = dict(zip(keys, values, strict=True))
        params["fee_rate"] = args.fee_rate
        params["sweep_name"] = args.sweep_name
        yield params


def apply_preset(args: argparse.Namespace) -> None:
    preset = PRESETS[args.preset]
    if args.min_ev_thresholds is None:
        args.min_ev_thresholds = parse_float_list(preset["min_ev_thresholds"])
    if args.desired_shares is None:
        args.desired_shares = parse_float_list(preset["desired_shares"])
    if args.funding_pcts is None:
        args.funding_pcts = parse_float_list(preset["funding_pcts"])
    if args.max_stake_usd is None:
        args.max_stake_usd = parse_float_list(preset["max_stake_usd"])


def available_dates(
    db: Database,
    city: str,
    replay_mode: str,
    lead_min: float,
    lead_max: float,
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

        if replay_mode == "interval":
            price_bins = db.conn.execute(
                """
                SELECT COUNT(DISTINCT bin_label) AS c
                FROM price_history_1m
                WHERE city = ? AND target_date = ?
                  AND hours_to_close > 0 AND hours_to_close <= 36
                """,
                (city, target_date),
            ).fetchone()["c"]
        else:
            price_bins = db.conn.execute(
                """
                SELECT COUNT(DISTINCT bin_label) AS c
                FROM price_history
                WHERE city = ? AND target_date = ?
                  AND hours_to_close >= ? AND hours_to_close <= ?
                """,
                (city, target_date, lead_min, lead_max),
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
        "city": summary.get("city"),
        "date_from": summary.get("date_from"),
        "date_to": summary.get("date_to"),
        "replay_mode": params.get("replay_mode"),
        "interval_minutes": params.get("interval_minutes"),
        "lead_min": params.get("lead_min"),
        "lead_max": params.get("lead_max"),
        "min_ev_threshold": params["min_ev_threshold"],
        "desired_shares": params["desired_shares"],
        "funding_pct": params["funding_pct"],
        "max_stake_usd": params["max_stake_usd"],
        "fee_rate": params["fee_rate"],
        "total_trades": summary.get("total_trades"),
        "wins": summary.get("wins"),
        "win_rate": summary.get("win_rate"),
        "total_pnl": total_pnl,
        "total_stake": total_stake,
        "roi": roi,
        "sharpe": summary.get("sharpe"),
        "rank_metric": metric,
        "rank_score": None,
        "params_json": json.dumps(params, sort_keys=True),
    }
    row["rank_score"] = metric_value(row, metric, min_trades)
    return row


def write_csv(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if not rows:
        path.write_text("")
        return
    fieldnames = list(rows[0].keys())
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
        f"- Replay mode: `{meta['replay_mode']}`",
        f"- Parameter combinations: `{meta['combos']}`",
        f"- Rank metric: `{meta['rank_by']}`",
        f"- Minimum trades for ranking: `{meta['min_trades']}`",
        f"- CSV: `{meta['csv']}`",
        "",
        "## Grid",
        "",
        f"- `min_ev_threshold`: `{meta['min_ev_thresholds']}`",
        f"- `desired_shares`: `{meta['desired_shares']}`",
        f"- `funding_pct`: `{meta['funding_pcts']}`",
        f"- `max_stake_usd`: `{meta['max_stake_usd']}`",
        f"- `fee_rate`: `{meta['fee_rate']}`",
        "",
        "## Top Results",
        "",
        "| Rank | Backtest ID | ROI | PnL | Stake | Trades | Win% | Sharpe | min_ev | shares | funding | max_stake |",
        "|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]

    for rank, row in enumerate(top, start=1):
        lines.append(
            "| {rank} | `{backtest_id}` | {roi} | {pnl} | {stake} | {trades} | {win_rate} | {sharpe} | {min_ev} | {shares} | {funding} | {max_stake} |".format(
                rank=rank,
                backtest_id=row.get("backtest_id") or "",
                roi=format_pct(row.get("roi")),
                pnl=format_float(row.get("total_pnl")),
                stake=format_float(row.get("total_stake")),
                trades=row.get("total_trades") or 0,
                win_rate=format_pct(row.get("win_rate")),
                sharpe=format_float(row.get("sharpe")),
                min_ev=format_float(row.get("min_ev_threshold"), 3),
                shares=format_float(row.get("desired_shares"), 2),
                funding=format_float(row.get("funding_pct"), 2),
                max_stake=format_float(row.get("max_stake_usd"), 2),
            )
        )

    lines.extend(
        [
            "",
            "## Notes",
            "",
            "- Rows below the minimum trade count receive a blank `rank_score` and sort below eligible rows.",
            "- This report ranks the tested period only. Keep a separate validation period before promoting parameters.",
            "- Each row also exists in SQLite as a normal backtest run, so the dashboard can inspect the generated `backtest_id`.",
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
    parser.add_argument("--strategy-prefix", default="sweep_base_ev", help="Strategy name prefix stored per run")
    parser.add_argument("--city", default="NYC", help="City filter, e.g. NYC")
    parser.add_argument(
        "--nyc-all-available",
        action="store_true",
        help="Shortcut: use NYC, all currently backtest-ready NYC dates, the full preset, and write a Markdown report",
    )
    parser.add_argument("--all-available", action="store_true", help="Use all backtest-ready dates for --city")
    parser.add_argument("--days", type=positive_int, default=60, help="Days of history when --date-from is omitted")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD, default yesterday UTC")
    parser.add_argument(
        "--replay-mode",
        choices=SUPPORTED_REPLAY_MODES,
        default=DEFAULT_REPLAY_MODE,
        help="Backtest replay mode",
    )
    parser.add_argument(
        "--interval-minutes",
        type=positive_int,
        default=DEFAULT_INTERVAL_MINUTES,
        help="Bucket size for interval replay mode",
    )
    parser.add_argument("--lead-min", type=float, default=DEFAULT_LEAD_MIN)
    parser.add_argument("--lead-max", type=float, default=DEFAULT_LEAD_MAX)
    parser.add_argument("--calibration-file", default=str(DEFAULT_CALIBRATION_PATH))
    parser.add_argument("--db", default=None, help="Path to SQLite DB")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR), help="Directory for CSV summaries")
    parser.add_argument("--rank-by", choices=("roi", "total_pnl", "sharpe", "win_rate"), default="roi")
    parser.add_argument("--min-trades", type=int, default=5, help="Minimum trades required for ranking")
    parser.add_argument("--preset", choices=tuple(PRESETS), default="quick", help="Default parameter grid preset")
    parser.add_argument("--report", action="store_true", help="Write a Markdown report next to the CSV")

    parser.add_argument("--min-ev-thresholds", type=parse_float_list, default=None)
    parser.add_argument("--desired-shares", type=parse_float_list, default=None)
    parser.add_argument("--funding-pcts", type=parse_float_list, default=None)
    parser.add_argument("--max-stake-usd", type=parse_float_list, default=None)
    parser.add_argument("--fee-rate", type=float, default=0.05)

    args = parser.parse_args()
    if args.nyc_all_available:
        args.city = "NYC"
        args.all_available = True
        args.report = True
        args.preset = "full"
    if args.lead_min > args.lead_max:
        parser.error("--lead-min must be <= --lead-max")
    city = args.city.upper()
    if city and city not in CITIES:
        parser.error(f"--city must be one of: {', '.join(sorted(CITIES))}")
    args.city = city
    apply_preset(args)
    if not args.sweep_name:
        args.sweep_name = f"{args.city.lower()}_{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')}"
    return args


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s - %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )
    args = parse_args()

    calibration_table = _load_calibration_table(Path(args.calibration_file))
    db = Database() if not args.db else Database(Path(args.db))
    available: list[str] = []
    today = datetime.now(timezone.utc).date()
    date_to = date.fromisoformat(args.date_to) if args.date_to else today - timedelta(days=1)
    date_from = date.fromisoformat(args.date_from) if args.date_from else date_to - timedelta(days=args.days - 1)

    if args.all_available:
        available = available_dates(db, args.city, args.replay_mode, args.lead_min, args.lead_max)
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

    rows: list[dict] = []
    combos = list(iter_param_grid(args))
    log.info("Running %d parameter combinations for sweep '%s'", len(combos), args.sweep_name)

    try:
        for combo_index, params in enumerate(combos, start=1):
            params = dict(params)
            params.update(
                {
                    "replay_mode": args.replay_mode,
                    "interval_minutes": args.interval_minutes if args.replay_mode == "interval" else None,
                    "lead_min": args.lead_min,
                    "lead_max": args.lead_max,
                }
            )
            strategy_name = f"{args.strategy_prefix}_{combo_index:04d}"
            log.info("Combo %d/%d: %s", combo_index, len(combos), params)
            summary = run_backtest(
                db=db,
                strategy_name=strategy_name,
                params=params,
                date_from=date_from,
                date_to=date_to,
                city=args.city,
                lead_min=args.lead_min,
                lead_max=args.lead_max,
                calibration_table=calibration_table,
                replay_mode=args.replay_mode,
                interval_minutes=args.interval_minutes,
            )
            rows.append(enrich_summary(summary, combo_index, params, args.rank_by, args.min_trades))
    finally:
        db.close()

    ranked = sorted(rows, key=sort_score, reverse=True)
    out_path = Path(args.out_dir) / f"{args.sweep_name}.csv"
    report_path = Path(args.out_dir) / f"{args.sweep_name}.md"
    write_csv(out_path, ranked)
    meta = {
        "sweep_name": args.sweep_name,
        "city": args.city,
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "available_date_count": len(available) if args.all_available else None,
        "replay_mode": args.replay_mode,
        "combos": len(rows),
        "rank_by": args.rank_by,
        "min_trades": args.min_trades,
        "csv": str(out_path),
        "report": str(report_path) if args.report else None,
        "min_ev_thresholds": ",".join(str(v) for v in args.min_ev_thresholds),
        "desired_shares": ",".join(str(v) for v in args.desired_shares),
        "funding_pcts": ",".join(str(v) for v in args.funding_pcts),
        "max_stake_usd": ",".join(str(v) for v in args.max_stake_usd),
        "fee_rate": args.fee_rate,
    }
    if args.report:
        write_report(report_path, ranked, meta)

    print(json.dumps({
        "sweep_name": args.sweep_name,
        "combos": len(rows),
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "available_date_count": meta["available_date_count"],
        "rank_by": args.rank_by,
        "min_trades": args.min_trades,
        "csv": str(out_path),
        "report": str(report_path) if args.report else None,
        "top": ranked[:10],
    }, indent=2))


if __name__ == "__main__":
    main()
