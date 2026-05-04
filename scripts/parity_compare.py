#!/usr/bin/env python3
"""
Run Python and Rust parity checks against the same SQLite DB.

Artifacts are written under poly-weather-rust/parity_runs. The Python repo is
treated as read-only except for commands that inherently use its default DB;
for those, pass a copied fixture DB explicitly where the Python command allows.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
PY_REPO = ROOT.parent / "poly-weather"
OUT = ROOT / "parity_runs"
CARGO = os.environ.get("CARGO", "cargo")

FLOAT_KEYS = {
    "total_pnl",
    "total_stake",
    "win_rate",
    "sharpe",
    "roi",
    "final_equity",
    "ending_cash",
    "portfolio_return",
    "realized_pnl",
    "turnover_yield",
}
EXACT_KEYS = {
    "total_trades",
    "wins",
    "trades_opened",
    "accepted_trades",
    "skipped_trades",
    "candidate_count",
}


def run_json(cmd: list[str], cwd: Path) -> dict[str, Any]:
    proc = subprocess.run(cmd, cwd=cwd, text=True, capture_output=True)
    if proc.returncode != 0:
        raise SystemExit(
            f"command failed: {' '.join(cmd)}\nSTDOUT:\n{proc.stdout[-2000:]}\nSTDERR:\n{proc.stderr[-4000:]}"
        )
    text = proc.stdout.strip()
    for start in range(len(text)):
        if text[start] not in "[{":
            continue
        try:
            return json.loads(text[start:])
        except json.JSONDecodeError:
            continue
    raise SystemExit(f"no JSON payload found from: {' '.join(cmd)}\nSTDOUT:\n{proc.stdout[-4000:]}")


def compare_dicts(name: str, left: dict[str, Any], right: dict[str, Any], tolerance: float) -> list[dict[str, Any]]:
    failures: list[dict[str, Any]] = []
    for key in sorted(EXACT_KEYS | FLOAT_KEYS):
        if key not in left and key not in right:
            continue
        lval = left.get(key)
        rval = right.get(key)
        if key in FLOAT_KEYS and isinstance(lval, (int, float)) and isinstance(rval, (int, float)):
            if abs(float(lval) - float(rval)) > tolerance:
                failures.append({"check": name, "key": key, "python": lval, "rust": rval})
        elif lval != rval:
            failures.append({"check": name, "key": key, "python": lval, "rust": rval})
    return failures


def csv_header(path: Path) -> list[str]:
    if not path.exists():
        return []
    with path.open(newline="") as fh:
        reader = csv.reader(fh)
        return next(reader, [])


def compare_headers(name: str, left: Path, right: Path) -> list[dict[str, Any]]:
    lhead = csv_header(left)
    rhead = csv_header(right)
    if lhead != rhead:
        return [{"check": name, "key": "csv_header", "python": lhead, "rust": rhead}]
    return []


def csv_row_count(path: Path) -> int | None:
    if not path.exists():
        return None
    with path.open(newline="") as fh:
        reader = csv.reader(fh)
        next(reader, None)
        return sum(1 for _ in reader)


def backtest(args: argparse.Namespace, strategy: str, suffix: str, extra_py: list[str] | None = None, extra_rust: list[str] | None = None) -> list[dict[str, Any]]:
    py_csv = OUT / f"python_{suffix}_summary.csv"
    py_trades = OUT / f"python_{suffix}_trades.csv"
    rust_csv = OUT / f"rust_{suffix}_summary.csv"
    rust_trades = OUT / f"rust_{suffix}_trades.csv"
    py_cmd = [
        sys.executable,
        "backtest.py",
        "--strategy",
        strategy,
        "--city",
        args.city,
        "--date-from",
        args.date_from,
        "--date-to",
        args.date_to,
        "--db",
        args.db,
        "--csv",
        str(py_csv),
        "--trades-csv",
        str(py_trades),
        "--report",
        str(OUT / f"python_{suffix}.md"),
    ] + (extra_py or [])
    rust_cmd = [
        CARGO,
        "run",
        "--quiet",
        "--",
        "backtest",
        "--strategy",
        strategy,
        "--city",
        args.city,
        "--date-from",
        args.date_from,
        "--date-to",
        args.date_to,
        "--db",
        args.db,
        "--csv",
        str(rust_csv),
        "--trades-csv",
        str(rust_trades),
        "--report",
        str(OUT / f"rust_{suffix}.md"),
    ] + (extra_rust or [])
    py = run_json(py_cmd, PY_REPO)
    rust = run_json(rust_cmd, ROOT)
    failures = compare_dicts(suffix, py, rust, args.tolerance)
    failures += compare_headers(f"{suffix}_summary", py_csv, rust_csv)
    failures += compare_headers(f"{suffix}_trades", py_trades, rust_trades)
    return failures


def bankroll(args: argparse.Namespace) -> list[dict[str, Any]]:
    py_json = OUT / "python_bankroll.json"
    rust_json = OUT / "rust_bankroll.json"
    py = run_json(
        [
            sys.executable,
            "backtest_bankroll.py",
            "--strategy",
            args.strategy,
            "--city",
            args.city,
            "--date-from",
            args.date_from,
            "--date-to",
            args.date_to,
            "--db",
            args.db,
            "--json",
            str(py_json),
            "--csv",
            str(OUT / "python_bankroll.csv"),
            "--trades-csv",
            str(OUT / "python_bankroll_trades.csv"),
            "--skips-csv",
            str(OUT / "python_bankroll_skips.csv"),
            "--equity-csv",
            str(OUT / "python_bankroll_equity.csv"),
            "--report",
            str(OUT / "python_bankroll.md"),
        ],
        PY_REPO,
    )
    rust = run_json(
        [
            CARGO,
            "run",
            "--quiet",
            "--",
            "bankroll-backtest",
            "--strategy",
            args.strategy,
            "--city",
            args.city,
            "--date-from",
            args.date_from,
            "--date-to",
            args.date_to,
            "--db",
            args.db,
            "--json",
            str(rust_json),
            "--csv",
            str(OUT / "rust_bankroll.csv"),
            "--trades-csv",
            str(OUT / "rust_bankroll_trades.csv"),
            "--skips-csv",
            str(OUT / "rust_bankroll_skips.csv"),
            "--equity-csv",
            str(OUT / "rust_bankroll_equity.csv"),
            "--report",
            str(OUT / "rust_bankroll.md"),
        ],
        ROOT,
    )
    py_summary = py.get("summary", py)
    rust_summary = rust.get("summary", rust)
    return compare_dicts("bankroll", py_summary, rust_summary, args.tolerance)


def sweep(args: argparse.Namespace) -> list[dict[str, Any]]:
    py = run_json(
        [
            sys.executable,
            "backtest_param_finetune/param_sweep.py",
            "--strategy",
            args.strategy,
            "--city",
            args.city,
            "--date-from",
            args.date_from,
            "--date-to",
            args.date_to,
            "--db",
            args.db,
            "--preset",
            "quick",
            "--out-dir",
            str(OUT / "python_sweep"),
            "--sweep-name",
            "parity_quick",
        ],
        PY_REPO,
    )
    rust = run_json(
        [
            CARGO,
            "run",
            "--quiet",
            "--",
            "param-sweep",
            "--strategy",
            args.strategy,
            "--city",
            args.city,
            "--date-from",
            args.date_from,
            "--date-to",
            args.date_to,
            "--db",
            args.db,
            "--preset",
            "quick",
            "--out-dir",
            str(OUT / "rust_sweep"),
            "--sweep-name",
            "parity_quick",
        ],
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    py_csv = OUT / "python_sweep" / "parity_quick.csv"
    rust_csv = OUT / "rust_sweep" / "parity_quick.csv"
    py_rows = py.get("rows", csv_row_count(py_csv))
    rust_rows = rust.get("rows", csv_row_count(rust_csv))
    if py_rows != rust_rows:
        failures.append({"check": "sweep", "key": "rows", "python": py_rows, "rust": rust_rows})
    failures += compare_headers("sweep_csv", py_csv, rust_csv)
    return failures


def calibration(args: argparse.Namespace) -> list[dict[str, Any]]:
    py_out = OUT / "python_calibration.json"
    rust_out = OUT / "rust_calibration.json"
    proc = subprocess.run(
        [sys.executable, "calibrate.py", "--db", args.db, "--out", str(py_out)],
        cwd=PY_REPO,
        text=True,
        capture_output=True,
    )
    if proc.returncode != 0:
        raise SystemExit(f"python calibration failed\nSTDERR:\n{proc.stderr[-4000:]}")
    py = json.loads(py_out.read_text())
    rust = run_json(
        [CARGO, "run", "--quiet", "--", "calibrate", "--db", args.db, "--out", str(rust_out)],
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    for key in ["cells", "_defaults"]:
        if key not in py or key not in rust:
            failures.append({"check": "calibration", "key": key, "python_has": key in py, "rust_has": key in rust})
    return failures


def lifecycle(args: argparse.Namespace) -> list[dict[str, Any]]:
    fixture_db = OUT / "lifecycle.db"
    if fixture_db.exists():
        fixture_db.unlink()
    import sqlite3

    conn = sqlite3.connect(fixture_db)
    conn.executescript((ROOT / "db" / "schema.sql").read_text())
    conn.close()
    plan = OUT / "execution_plan.json"
    plan.write_text(
        json.dumps(
            {
                "event_slug": "highest-temperature-in-nyc-on-april-1-2026",
                "target_date": args.date_to,
                "dry_run": True,
                "trades": [
                    {
                        "no_token_id": "no-token",
                        "market_id": "market",
                        "bin_label": "70-71°F",
                        "effective_ask": 0.25,
                        "fee_rate": 0.05,
                        "shares": 10,
                        "stake_usd": 2.5,
                        "strategy_name": args.strategy,
                        "ev_no": 0.1,
                        "p_model": 0.2,
                    }
                ],
            },
            indent=2,
        )
    )
    rust = run_json(
        [
            CARGO,
            "run",
            "--quiet",
            "--",
            "execute",
            "--db",
            str(fixture_db),
            "--plan",
            str(plan),
            "--positions",
            str(OUT / "rust_open_positions.json"),
            "--dry-run",
        ],
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    if rust.get("trades_recorded") != 1:
        failures.append({"check": "lifecycle", "key": "trades_recorded", "rust": rust.get("trades_recorded")})
    return failures


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--strategy", default="base_no_ev")
    parser.add_argument("--city", default="NYC")
    parser.add_argument("--date-from", required=True)
    parser.add_argument("--date-to", required=True)
    parser.add_argument("--db", default=str(PY_REPO / "db" / "data.db"))
    parser.add_argument("--tolerance", type=float, default=1e-4)
    parser.add_argument(
        "--check",
        action="append",
        choices=("backtest", "group", "bankroll", "sweep", "calibration", "lifecycle", "all"),
        default=None,
    )
    args = parser.parse_args()

    OUT.mkdir(exist_ok=True)
    checks = args.check or ["backtest"]
    if "all" in checks:
        checks = ["backtest", "group", "bankroll", "sweep", "calibration", "lifecycle"]

    failures: list[dict[str, Any]] = []
    if "backtest" in checks:
        failures += backtest(args, args.strategy, "backtest")
    if "group" in checks:
        failures += backtest(
            args,
            args.strategy,
            "group_core",
            extra_py=["--strategy-group", "core"],
            extra_rust=["--strategy-group", "core"],
        )
    if "bankroll" in checks:
        failures += bankroll(args)
    if "sweep" in checks:
        failures += sweep(args)
    if "calibration" in checks:
        failures += calibration(args)
    if "lifecycle" in checks:
        failures += lifecycle(args)

    result = {"checks": checks, "failures": failures, "out_dir": str(OUT)}
    print(json.dumps(result, indent=2))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
