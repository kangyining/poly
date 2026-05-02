"""
Calibration table builder.

Joins forecast_records (gfs-global-24h) with settlement_records to fit
(beta_f, sigma_f) per (station_code, month, lead_bin).

  beta_f  = mean forecast error  (settlement_temp_f - forecast_temp_f)
  sigma_f = std  forecast error

Exports calibration.json consumed by the Rust bot at startup.

Usage:
    python analysis/calibration.py            # fit + export
    python analysis/calibration.py --show     # print table and exit
"""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import statistics
from collections import defaultdict
from datetime import datetime
from pathlib import Path

DB_PATH   = Path(__file__).parent.parent / "db" / "data.db"
OUT_PATH  = Path(__file__).parent.parent / "calibration.json"

# Minimum samples required to use a fitted cell; fall back to station default otherwise
MIN_SAMPLES = 10

DEFAULT_BETA_F  = 0.0
DEFAULT_SIGMA_F = 2.0
SIGMA_MIN       = 0.8
SIGMA_MAX       = 4.0


def lead_bin(hours: float) -> str:
    if hours < 6:   return "lt6"
    if hours < 12:  return "6_12"
    if hours < 24:  return "12_24"
    if hours < 36:  return "24_36"
    if hours < 48:  return "36_48"
    if hours < 72:  return "48_72"
    return "72_plus"


def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def fit(errors: list[float]) -> tuple[float, float]:
    """Return (beta_f, sigma_f) for a list of error values."""
    n = len(errors)
    if n == 0:
        return DEFAULT_BETA_F, DEFAULT_SIGMA_F
    beta = statistics.mean(errors)
    sigma = statistics.stdev(errors) if n > 1 else DEFAULT_SIGMA_F
    return beta, clamp(sigma, SIGMA_MIN, SIGMA_MAX)


def _ensure_column(conn: sqlite3.Connection, table: str, name: str, ddl: str) -> None:
    existing = {row["name"] for row in conn.execute(f"PRAGMA table_info({table})")}
    if name not in existing:
        conn.execute(f"ALTER TABLE {table} ADD COLUMN {name} {ddl}")


def build_table() -> dict:
    """
    Returns nested dict:
      table[station][month_str][lead_bin] = {"beta_f": ..., "sigma_f": ..., "n": ...}

    Plus a "_defaults" key with per-station and global fallbacks.
    """
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row
    _ensure_column(conn, "forecast_records", "station_id", "TEXT")
    _ensure_column(conn, "settlement_records", "station_id", "TEXT")
    _ensure_column(conn, "settlement_records", "settlement_temp_station", "TEXT")

    rows = conn.execute("""
        SELECT
            f.city,
            f.target_date,
            f.temp_max_gfs_f     AS forecast_f,
            f.forecast_lead_hours,
            f.station_id          AS forecast_station,
            s.settlement_temp_f,
            s.station_id          AS settlement_station,
            s.settlement_temp_station,
            s.wunderground_station
        FROM forecast_records f
        JOIN settlement_records s
          ON f.city = s.city AND f.target_date = s.target_date
        WHERE f.source = 'gfs-global-24h'
          AND f.temp_max_gfs_f IS NOT NULL
          AND s.settlement_temp_f IS NOT NULL
        ORDER BY f.city, f.target_date
    """).fetchall()

    conn.close()

    # Bucket errors by (station, month, lead_bin)
    cells: dict[tuple, list[float]] = defaultdict(list)
    station_errors: dict[str, list[float]] = defaultdict(list)
    all_errors: list[float] = []

    for row in rows:
        station = (
            row["settlement_station"]
            or row["settlement_temp_station"]
            or row["wunderground_station"]
            or row["forecast_station"]
            or row["city"]
        )
        month   = str(datetime.fromisoformat(row["target_date"]).month)
        lb      = lead_bin(row["forecast_lead_hours"] or 26.0)
        error   = row["settlement_temp_f"] - row["forecast_f"]

        cells[(station, month, lb)].append(error)
        station_errors[station].append(error)
        all_errors.append(error)

    # Build output structure
    table: dict = {}

    for (station, month, lb), errors in cells.items():
        if station not in table:
            table[station] = {}
        if month not in table[station]:
            table[station][month] = {}

        n = len(errors)
        if n >= MIN_SAMPLES:
            beta, sigma = fit(errors)
        else:
            # Not enough data — mark as low-confidence but still record
            beta, sigma = fit(errors)

        table[station][month][lb] = {
            "beta_f":  round(beta,  4),
            "sigma_f": round(sigma, 4),
            "n":       n,
        }

    # Per-station and global defaults
    defaults: dict = {}
    for station, errors in station_errors.items():
        beta, sigma = fit(errors)
        defaults[station] = {
            "beta_f":  round(beta,  4),
            "sigma_f": round(sigma, 4),
            "n":       len(errors),
        }

    global_beta, global_sigma = fit(all_errors)
    defaults["_global"] = {
        "beta_f":  round(global_beta,  4),
        "sigma_f": round(global_sigma, 4),
        "n":       len(all_errors),
    }

    return {"cells": table, "_defaults": defaults}


def lookup(
    table: dict,
    station: str,
    month: int,
    lead_hours: float,
) -> tuple[float, float]:
    """
    Look up (beta_f, sigma_f) with fallback chain:
      cell (station, month, lead_bin) → station default → global default
    """
    lb  = lead_bin(lead_hours)
    mon = str(month)

    cell = (
        table.get("cells", {})
             .get(station, {})
             .get(mon, {})
             .get(lb)
    )
    if cell and cell["n"] >= MIN_SAMPLES:
        return cell["beta_f"], cell["sigma_f"]

    station_default = table.get("_defaults", {}).get(station)
    if station_default:
        return station_default["beta_f"], station_default["sigma_f"]

    global_default = table.get("_defaults", {}).get("_global", {})
    return (
        global_default.get("beta_f",  DEFAULT_BETA_F),
        global_default.get("sigma_f", DEFAULT_SIGMA_F),
    )


def print_table(table: dict) -> None:
    print(f"\n{'Station':<8} {'Month':>5} {'Lead bin':>10} {'Beta_F':>8} {'Sigma_F':>8} {'N':>4}")
    print("-" * 55)
    for station, months in sorted(table.get("cells", {}).items()):
        for month, lead_bins in sorted(months.items(), key=lambda x: int(x[0])):
            for lb, vals in sorted(lead_bins.items()):
                flag = "" if vals["n"] >= MIN_SAMPLES else " *"
                print(f"{station:<8} {month:>5} {lb:>10} {vals['beta_f']:>8.3f} "
                      f"{vals['sigma_f']:>8.3f} {vals['n']:>4}{flag}")

    print("\n* = fewer than", MIN_SAMPLES, "samples, using station default as fallback\n")
    print("Station defaults:")
    for station, vals in sorted(table.get("_defaults", {}).items()):
        if station == "_global":
            continue
        print(f"  {station}: beta={vals['beta_f']:+.3f}F  sigma={vals['sigma_f']:.3f}F  n={vals['n']}")
    gd = table["_defaults"].get("_global", {})
    print(f"  _global: beta={gd.get('beta_f', 0):+.3f}F  sigma={gd.get('sigma_f', 2):.3f}F  n={gd.get('n', 0)}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--show", action="store_true", help="Print table and exit without writing")
    parser.add_argument("--db",   default=None, help="Path to SQLite DB")
    parser.add_argument("--out",  default=None, help="Output JSON path")
    args = parser.parse_args()

    db_path  = Path(args.db)  if args.db  else DB_PATH
    out_path = Path(args.out) if args.out else OUT_PATH

    table = build_table()
    print_table(table)

    if args.show:
        return

    out_path.write_text(json.dumps(table, indent=2))
    print(f"\nCalibration written to: {out_path}")
    total_cells = sum(
        len(lbs)
        for months in table.get("cells", {}).values()
        for lbs in months.values()
    )
    print(f"Total cells: {total_cells}  |  Total samples: {table['_defaults'].get('_global', {}).get('n', 0)}")


if __name__ == "__main__":
    main()
