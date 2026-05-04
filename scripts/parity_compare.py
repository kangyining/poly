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
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
PY_REPO = ROOT.parent / "poly-weather"
OUT = ROOT / "parity_runs"
CARGO = os.environ.get("CARGO", "cargo")
COMMAND_TIMEOUT_SECONDS = int(os.environ.get("POLY_WEATHER_PARITY_TIMEOUT", "1200"))

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
    print(f"[parity] running in {cwd.name}: {' '.join(cmd)}", file=sys.stderr, flush=True)
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            text=True,
            capture_output=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout or ""
        stderr = exc.stderr or ""
        raise SystemExit(
            f"command timed out after {COMMAND_TIMEOUT_SECONDS}s: {' '.join(cmd)}\n"
            f"STDOUT:\n{stdout[-2000:]}\nSTDERR:\n{stderr[-4000:]}"
        )
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


def csv_rows(path: Path) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as fh:
        return list(csv.DictReader(fh))


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
    return sweep_preset(args, "quick", "sweep", "parity_quick")


def sweep_preset(
    args: argparse.Namespace,
    preset: str,
    check_name: str,
    sweep_name: str,
    extra: list[str] | None = None,
) -> list[dict[str, Any]]:
    py_dir = OUT / f"python_{check_name}"
    rust_dir = OUT / f"rust_{check_name}"
    extra = extra or []
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
            preset,
            "--out-dir",
            str(py_dir),
            "--sweep-name",
            sweep_name,
        ]
        + extra,
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
            preset,
            "--out-dir",
            str(rust_dir),
            "--sweep-name",
            sweep_name,
        ]
        + extra,
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    py_csv = py_dir / f"{sweep_name}.csv"
    rust_csv = rust_dir / f"{sweep_name}.csv"
    py_rows = py.get("rows", csv_row_count(py_csv))
    rust_rows = rust.get("rows", csv_row_count(rust_csv))
    if py_rows != rust_rows:
        failures.append({"check": check_name, "key": "rows", "python": py_rows, "rust": rust_rows})
    failures += compare_headers(f"{check_name}_csv", py_csv, rust_csv)
    failures += compare_sweep_artifacts(check_name, py, rust, py_dir, rust_dir, sweep_name)
    return failures


def sweep_full(args: argparse.Namespace) -> list[dict[str, Any]]:
    return sweep_preset(args, "full", "sweep_full", "parity_full")


def sweep_exhaustive(args: argparse.Namespace) -> list[dict[str, Any]]:
    return sweep_preset(args, "exhaustive", "sweep_exhaustive", "parity_exhaustive")


def sweep_all_available(args: argparse.Namespace) -> list[dict[str, Any]]:
    return sweep_preset(
        args,
        "quick",
        "sweep_all_available",
        "parity_all_available",
        extra=[
            "--all-available",
            "--min-ev-thresholds",
            "0.04",
            "--desired-shares",
            "10",
        ],
    )


def compare_sweep_artifacts(
    check_name: str,
    py_payload: dict[str, Any],
    rust_payload: dict[str, Any],
    py_dir: Path,
    rust_dir: Path,
    sweep_name: str,
) -> list[dict[str, Any]]:
    failures: list[dict[str, Any]] = []
    py_csv = py_dir / f"{sweep_name}.csv"
    rust_csv = rust_dir / f"{sweep_name}.csv"
    py_report = py_dir / f"{sweep_name}.md"
    rust_report = rust_dir / f"{sweep_name}.md"
    py_final = py_dir / f"{sweep_name}__best_by_strategy.md"
    rust_final = rust_dir / f"{sweep_name}__best_by_strategy.md"
    for label, py_path, rust_path in [
        ("csv", py_csv, rust_csv),
        ("report", py_report, rust_report),
        ("best_by_strategy_report", py_final, rust_final),
    ]:
        if py_path.exists() != rust_path.exists():
            failures.append(
                {
                    "check": check_name,
                    "key": f"{label}_exists",
                    "python": py_path.exists(),
                    "rust": rust_path.exists(),
                }
            )
    for label, path in [("python_report", py_report), ("rust_report", rust_report)]:
        text = path.read_text() if path.exists() else ""
        for marker in ["# Parameter Sweep Report", "## Top Results"]:
            if marker not in text:
                failures.append({"check": check_name, "key": label, "missing": marker})
    for label, path in [("python_best_report", py_final), ("rust_best_report", rust_final)]:
        text = path.read_text() if path.exists() else ""
        if "Strategy" not in text:
            failures.append({"check": check_name, "key": label, "missing": "Strategy"})
    py_rows = csv_rows(py_csv)
    rust_rows = csv_rows(rust_csv)
    if py_rows and rust_rows:
        for key in ["strategy", "strategy_id", "total_trades", "wins"]:
            if py_rows[0].get(key) != rust_rows[0].get(key):
                failures.append(
                    {
                        "check": check_name,
                        "key": f"top.{key}",
                        "python": py_rows[0].get(key),
                        "rust": rust_rows[0].get(key),
                    }
                )
    py_artifacts = py_payload.get("strategy_artifacts", {})
    rust_artifacts = rust_payload.get("strategy_artifacts", {})
    if sorted(py_artifacts) != sorted(rust_artifacts):
        failures.append(
            {
                "check": check_name,
                "key": "strategy_artifacts.keys",
                "python": sorted(py_artifacts),
                "rust": sorted(rust_artifacts),
            }
        )
    for strategy in sorted(set(py_artifacts) & set(rust_artifacts)):
        for suffix in [".csv", ".md"]:
            py_path = py_dir / f"{sweep_name}__{strategy}{suffix}"
            rust_path = rust_dir / f"{sweep_name}__{strategy}{suffix}"
            if py_path.exists() != rust_path.exists():
                failures.append(
                    {
                        "check": check_name,
                        "key": f"{strategy}{suffix}.exists",
                        "python": py_path.exists(),
                        "rust": rust_path.exists(),
                    }
                )
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


def live_recorder(args: argparse.Namespace) -> list[dict[str, Any]]:
    if not args.live:
        return [
            {
                "check": "live_recorder",
                "key": "live",
                "message": "pass --live to run the opt-in network smoke check",
            }
        ]
    fixture_db = OUT / "live_recorder.db"
    if fixture_db.exists():
        fixture_db.unlink()
    rust = run_json(
        [
            CARGO,
            "run",
            "--quiet",
            "--",
            "recorder",
            "--db",
            str(fixture_db),
            "--discover-contracts",
            "--broad-gamma-discovery",
            "--city",
            args.city,
        ],
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    summary = rust.get("contract_discovery", {})
    if "contracts_seen" not in summary:
        failures.append({"check": "live_recorder", "key": "contracts_seen", "rust": summary})
    if not fixture_db.exists():
        failures.append({"check": "live_recorder", "key": "db_exists", "rust": False})
    else:
        conn = sqlite3.connect(fixture_db)
        try:
            columns = [
                row[1]
                for row in conn.execute("PRAGMA table_info(weather_contracts)").fetchall()
            ]
        finally:
            conn.close()
        for column in ["event_slug", "target_date", "city_slug", "validation_status", "raw_json"]:
            if column not in columns:
                failures.append({"check": "live_recorder", "key": "weather_contracts.column", "missing": column})
    return failures


def live_recorder_once(args: argparse.Namespace) -> list[dict[str, Any]]:
    if not args.live:
        return [
            {
                "check": "live_recorder_once",
                "key": "live",
                "message": "pass --live to run the opt-in recorder --once network smoke check",
            }
        ]
    fixture_db = OUT / "live_recorder_once.db"
    positions = OUT / "live_recorder_once_open_positions.json"
    for path in [fixture_db, positions]:
        if path.exists():
            path.unlink()
    rust = run_json(
        [
            CARGO,
            "run",
            "--quiet",
            "--",
            "recorder",
            "--db",
            str(fixture_db),
            "--positions",
            str(positions),
            "--once",
            "--broad-gamma-discovery",
        ],
        ROOT,
    )
    failures: list[dict[str, Any]] = []
    if "run" not in rust:
        failures.append({"check": "live_recorder_once", "key": "run", "rust": rust})
    if not fixture_db.exists():
        failures.append({"check": "live_recorder_once", "key": "db_exists", "rust": False})
    else:
        conn = sqlite3.connect(fixture_db)
        try:
            expected_columns = {
                "market_snapshots": ["timestamp", "city", "target_date", "bin_label", "hours_to_close", "fee_rate"],
                "metar_records": ["timestamp", "icao", "city", "temp_f", "obs_time", "raw_metar"],
                "taf_records": ["timestamp", "icao", "city", "issue_time", "valid_from", "valid_to", "raw_taf"],
                "nws_forecasts": ["timestamp", "city", "target_date", "temp_f", "forecast_lead_hours"],
                "forecast_records": ["timestamp", "city", "target_date", "source", "temp_max_gfs_f", "event_slug"],
                "settlement_records": ["city", "target_date", "event_slug", "winning_bin", "settlement_temp_source"],
            }
            for table, columns in expected_columns.items():
                actual = [row[1] for row in conn.execute(f"PRAGMA table_info({table})").fetchall()]
                if not actual:
                    failures.append({"check": "live_recorder_once", "key": f"{table}.exists", "rust": False})
                    continue
                for column in columns:
                    if column not in actual:
                        failures.append({"check": "live_recorder_once", "key": f"{table}.column", "missing": column})
        finally:
            conn.close()
    if not positions.exists():
        failures.append({"check": "live_recorder_once", "key": "positions_path", "rust": False})
    else:
        try:
            json.loads(positions.read_text())
        except json.JSONDecodeError as exc:
            failures.append({"check": "live_recorder_once", "key": "positions_json", "rust": str(exc)})
    return failures


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--strategy", default="base_no_ev")
    parser.add_argument("--city", default="NYC")
    parser.add_argument("--date-from", default="2026-04-01")
    parser.add_argument("--date-to", default="2026-04-02")
    parser.add_argument("--db", default=str(PY_REPO / "db" / "data.db"))
    parser.add_argument("--tolerance", type=float, default=1e-4)
    parser.add_argument("--live", action="store_true")
    parser.add_argument(
        "--check",
        action="append",
        choices=(
            "backtest",
            "group",
            "bankroll",
            "sweep",
            "sweep-full",
            "sweep-exhaustive",
            "sweep-all-available",
            "calibration",
            "lifecycle",
            "live-recorder",
            "live-recorder-once",
            "all",
        ),
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
    if "sweep-full" in checks:
        failures += sweep_full(args)
    if "sweep-exhaustive" in checks:
        failures += sweep_exhaustive(args)
    if "sweep-all-available" in checks:
        failures += sweep_all_available(args)
    if "calibration" in checks:
        failures += calibration(args)
    if "lifecycle" in checks:
        failures += lifecycle(args)
    if "live-recorder" in checks:
        failures += live_recorder(args)
    if "live-recorder-once" in checks:
        failures += live_recorder_once(args)

    result = {"checks": checks, "failures": failures, "out_dir": str(OUT)}
    print(json.dumps(result, indent=2))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
