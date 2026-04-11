"""
Phase 1 Backtest — Distant-Bin NO Strategy

Strategy:
  For each settled date, use the best model forecast (GFS for NYC).
  Any bin whose midpoint is ≥ EXCLUSION_RANGE away from the forecast is a candidate.
  Buy NO at the price recorded at a fixed lead time before close.
  Win if that bin does NOT settle as the winner.

Usage:
  source venv/bin/activate
  python backtest/phase1_backtest.py
  python backtest/phase1_backtest.py --city NYC --entry-lead 24 --exclusion-range 4.0
  python backtest/phase1_backtest.py --sweep   # sweep param grid
"""

import argparse
import re
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# ── Config ────────────────────────────────────────────────────────────────────

DB_PATH = Path(__file__).parent.parent / "db" / "data.db"

BEST_MODEL = {
    "Chicago": "gfs",
    "Dallas":  "ecmwf",
    "NYC":     "gfs",
}

TAKER_FEE_RATE = 0.05   # fee per share = rate × p_yes × p_no

# ── Helpers ───────────────────────────────────────────────────────────────────

def parse_bin_midpoint(label: str) -> Optional[float]:
    """
    Extract numeric midpoint from a bin label.
    Examples:
      "35°F or below"   → 34.5 (midpoint of open-ended low; use lower edge - 1)
      "50°F or higher"  → 50.5 (midpoint of open-ended high; use upper edge + 1)
      "36-37°F"         → 36.5
      "42-43°F"         → 42.5
    """
    label = label.strip()
    # Open-ended high: "50°F or higher", "≥50°F"
    m = re.match(r'(\d+)°?F? or higher', label, re.I)
    if m:
        return float(m.group(1)) + 0.5
    m = re.match(r'[≥>=]+\s*(\d+)', label)
    if m:
        return float(m.group(1)) + 0.5

    # Open-ended low: "35°F or below", "≤35°F"
    m = re.match(r'(\d+)°?F? or below', label, re.I)
    if m:
        return float(m.group(1)) - 0.5
    m = re.match(r'[≤<=]+\s*(\d+)', label)
    if m:
        return float(m.group(1)) - 0.5

    # Range: "36-37°F", "36–37°F"
    m = re.match(r'(\d+)\s*[-–]\s*(\d+)', label)
    if m:
        lo, hi = float(m.group(1)), float(m.group(2))
        return (lo + hi) / 2.0

    return None


def taker_fee(shares: float, yes_price: float) -> float:
    """Taker fee in USDC for buying `shares` of a token priced at yes_price."""
    return shares * TAKER_FEE_RATE * yes_price * (1 - yes_price)


def is_winning_bin(label: str, temp_f: float) -> bool:
    """Return True if the given temperature falls into this bin."""
    label = label.strip()

    # Open-ended high
    m = re.match(r'(\d+)°?F? or higher', label, re.I)
    if m:
        return temp_f >= float(m.group(1))
    m = re.match(r'[≥>=]+\s*(\d+)', label)
    if m:
        return temp_f >= float(m.group(1))

    # Open-ended low
    m = re.match(r'(\d+)°?F? or below', label, re.I)
    if m:
        return temp_f <= float(m.group(1))
    m = re.match(r'[≤<=]+\s*(\d+)', label)
    if m:
        return temp_f <= float(m.group(1))

    # Range: "36-37°F"
    m = re.match(r'(\d+)\s*[-–]\s*(\d+)', label)
    if m:
        lo, hi = float(m.group(1)), float(m.group(2))
        return lo <= temp_f <= hi

    return False


# ── Core backtest ─────────────────────────────────────────────────────────────

@dataclass
class Trade:
    date: str
    bin_label: str
    bin_index: int
    gfs_forecast_f: float
    bin_midpoint: float
    distance_f: float          # |bin_midpoint - forecast|
    entry_lead_h: float        # hours before close at entry
    no_price_entry: float      # NO price at entry
    position_usdc: float
    shares: float
    fee_usdc: float
    won: bool                  # True if NO wins (this bin did NOT settle)
    pnl_usdc: float            # net P&L in USDC


FORECAST_BIAS_WARNING = """
⚠️  预报来源说明（重要）
    当前使用：GFS 再分析数据（lead=-1），事后拉取，非真实 24h 提前预报
    影响：胜率和 ROI 偏乐观，真实预报误差更大（再分析 MAE ~1.7°F vs 真实24h ~3.3°F）
    建议：加 --true-forecast 参数使用真实 24h GFS 预报重跑回测
"""


def run_backtest(
    city: str = "NYC",
    entry_lead_h: float = 24.0,
    exclusion_range_f: float = 4.0,
    min_no_price: float = 0.90,
    max_no_price: float = 0.99,
    position_usdc: float = 5.0,
    use_true_forecast: bool = False,
    verbose: bool = False,
) -> list[Trade]:
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row

    # Pick the best model column for this city (ECMWF for Dallas, GFS elsewhere)
    model_key = BEST_MODEL.get(city, "gfs")
    col = "temp_max_ecmwf_f" if model_key == "ecmwf" else "temp_max_gfs_f"

    # True 24h forecast (gfs_global) or retroactive reanalysis (gfs_seamless)
    # Note: true-forecast backfill only stores gfs_global, so Dallas uses GFS
    # when use_true_forecast=True (no ECMWF equivalent available for that source).
    if use_true_forecast:
        forecast_query = f"""
            SELECT temp_max_gfs_f FROM forecast_records
            WHERE city = ? AND target_date = ? AND source = 'gfs-global-24h'
            LIMIT 1
        """
    else:
        forecast_query = f"""
            SELECT {col} FROM forecast_records
            WHERE city = ? AND target_date = ?
            ORDER BY ABS(forecast_lead_hours - (-1)) ASC
            LIMIT 1
        """

    # Pull all settlements with temperature
    settlements = conn.execute("""
        SELECT target_date, winning_bin, settlement_temp_f
        FROM settlement_records
        WHERE city = ? AND settlement_temp_f IS NOT NULL
        ORDER BY target_date
    """, (city,)).fetchall()

    trades: list[Trade] = []

    for s in settlements:
        date = s["target_date"]
        actual_temp = s["settlement_temp_f"]
        winning_bin_label = s["winning_bin"]

        fc_row = conn.execute(forecast_query, (city, date)).fetchone()

        if not fc_row or fc_row[0] is None:
            if verbose:
                src = "gfs-global-24h" if use_true_forecast else f"{model_key} reanalysis"
                print(f"  {date}: no {src} forecast, skipping")
            continue

        forecast_f = fc_row[0]

        # Get all bins with price at the target lead time
        # Find the closest available hour to `entry_lead_h`
        price_rows = conn.execute("""
            SELECT bin_label, bin_index, yes_price, hours_to_close
            FROM price_history
            WHERE city = ? AND target_date = ?
            ORDER BY ABS(hours_to_close - ?) ASC
        """, (city, date, entry_lead_h)).fetchall()

        # Group by bin: take the closest lead time per bin
        bin_prices: dict[int, dict] = {}
        for row in price_rows:
            bi = row["bin_index"]
            if bi not in bin_prices:
                bin_prices[bi] = dict(row)

        for bi, row in bin_prices.items():
            label = row["bin_label"]
            yes_price = row["yes_price"]
            actual_lead = row["hours_to_close"]
            no_price = 1.0 - yes_price

            midpoint = parse_bin_midpoint(label)
            if midpoint is None:
                continue

            distance = abs(midpoint - forecast_f)

            # Strategy filter
            if distance < exclusion_range_f:
                continue
            if not (min_no_price <= no_price <= max_no_price):
                continue

            # Size the position
            # shares = USDC / no_price
            shares = position_usdc / no_price
            fee = taker_fee(shares, yes_price)

            # Outcome
            won = not is_winning_bin(label, actual_temp)
            if won:
                pnl = shares * 1.0 - position_usdc - fee  # settle at $1
            else:
                pnl = -position_usdc - fee                 # settle at $0

            t = Trade(
                date=date,
                bin_label=label,
                bin_index=bi,
                gfs_forecast_f=forecast_f,
                bin_midpoint=midpoint,
                distance_f=distance,
                entry_lead_h=actual_lead,
                no_price_entry=no_price,
                position_usdc=position_usdc,
                shares=shares,
                fee_usdc=fee,
                won=won,
                pnl_usdc=pnl,
            )
            trades.append(t)

            if verbose:
                result = "WIN" if won else "LOSS"
                print(f"  {date} | bin {bi:2d} {label:18s} | dist={distance:.1f}°F "
                      f"| NO={no_price:.3f} | {result} | P&L={pnl:+.2f}")

    conn.close()
    return trades


def summarize(trades: list[Trade], label: str = "") -> dict:
    if not trades:
        return {"n_trades": 0, "win_rate": 0, "total_pnl": 0, "roi": 0}

    n = len(trades)
    wins = sum(1 for t in trades if t.won)
    total_pnl = sum(t.pnl_usdc for t in trades)
    capital = sum(t.position_usdc for t in trades)
    roi = total_pnl / capital if capital else 0

    dates = sorted(set(t.date for t in trades))

    # P&L by date
    daily_pnl: dict[str, float] = {}
    for t in trades:
        daily_pnl[t.date] = daily_pnl.get(t.date, 0) + t.pnl_usdc
    max_dd = min(daily_pnl.values()) if daily_pnl else 0
    avg_no = sum(t.no_price_entry for t in trades) / n

    result = {
        "n_trades": n,
        "n_dates": len(dates),
        "n_wins": wins,
        "win_rate": wins / n,
        "total_pnl": total_pnl,
        "capital_deployed": capital,
        "roi": roi,
        "worst_day_pnl": max_dd,
        "avg_no_price": avg_no,
    }

    if label:
        print(f"\n{'─'*50}")
        print(f"  {label}")
        print(f"{'─'*50}")
        print(f"  Trades:         {n} across {len(dates)} dates")
        print(f"  Win rate:       {wins}/{n} = {wins/n:.1%}")
        print(f"  Total P&L:      ${total_pnl:+.2f}")
        print(f"  Capital used:   ${capital:.2f}")
        print(f"  ROI:            {roi:.1%}")
        print(f"  Worst day:      ${max_dd:+.2f}")
        print(f"  Avg NO price:   {avg_no:.3f}")

    return result


# ── Sweep ─────────────────────────────────────────────────────────────────────

def sweep(city: str = "NYC", use_true_forecast: bool = True):
    """Sweep exclusion range and entry lead time, find best params."""
    print(f"\n{'='*60}")
    print(f"  PARAM SWEEP — {city}")
    print(f"{'='*60}")
    print(f"  {'excl':>6} {'lead':>6} {'trades':>8} {'wins':>6} {'win%':>7} {'P&L':>10} {'ROI':>8}")
    print(f"  {'-'*6} {'-'*6} {'-'*8} {'-'*6} {'-'*7} {'-'*10} {'-'*8}")

    best_roi = -999
    best_params = {}

    for excl in [3.0, 4.0, 5.0, 6.0, 7.0, 8.0]:
        for lead in [6, 12, 24, 36, 48]:
            trades = run_backtest(city=city, entry_lead_h=lead,
                                  exclusion_range_f=excl, verbose=False,
                                  use_true_forecast=use_true_forecast)
            if not trades:
                continue
            s = summarize(trades)
            roi = s["roi"]
            label = f"  {excl:>6.1f} {lead:>6} {s['n_trades']:>8} {s['n_wins']:>6} {s['win_rate']:>7.1%} ${s['total_pnl']:>9.2f} {roi:>7.1%}"
            print(label)

            if roi > best_roi:
                best_roi = roi
                best_params = {"exclusion_range": excl, "entry_lead": lead,
                               "roi": roi, "win_rate": s["win_rate"],
                               "n_trades": s["n_trades"], "total_pnl": s["total_pnl"]}

    print(f"\n  Best params: exclusion={best_params.get('exclusion_range')}°F "
          f"lead={best_params.get('entry_lead')}h "
          f"ROI={best_params.get('roi', 0):.1%} "
          f"win_rate={best_params.get('win_rate', 0):.1%}")
    return best_params


# ── CLI ───────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Phase 1 backtest — distant-bin NO strategy")
    parser.add_argument("--city", default="NYC", help="City to backtest (default: NYC)")
    parser.add_argument("--entry-lead", type=float, default=24.0,
                        help="Hours before close to enter (default: 24)")
    parser.add_argument("--exclusion-range", type=float, default=4.0,
                        help="Min distance from forecast to trade (default: 4.0°F)")
    parser.add_argument("--min-no-price", type=float, default=0.90)
    parser.add_argument("--max-no-price", type=float, default=0.99)
    parser.add_argument("--position", type=float, default=5.0,
                        help="USDC per bin (default: 5.0)")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--no-true-forecast", action="store_true",
                        help="Use reanalysis data instead of true 24h GFS forecasts")
    parser.add_argument("--sweep", action="store_true",
                        help="Sweep parameter grid and find best settings")
    args = parser.parse_args()

    use_true = not args.no_true_forecast

    if not use_true:
        print(FORECAST_BIAS_WARNING)

    forecast_label = "gfs_global 真实24h" if use_true else "gfs_seamless 再分析"

    if args.sweep:
        print(f"预报来源: {forecast_label}")
        best = sweep(args.city, use_true_forecast=use_true)
        return

    print(f"预报来源: {forecast_label}")
    print(f"Backtest: {args.city} | lead={args.entry_lead}h | excl={args.exclusion_range}°F "
          f"| NO=[{args.min_no_price}, {args.max_no_price}] | pos=${args.position}")

    trades = run_backtest(
        city=args.city,
        entry_lead_h=args.entry_lead,
        exclusion_range_f=args.exclusion_range,
        min_no_price=args.min_no_price,
        max_no_price=args.max_no_price,
        position_usdc=args.position,
        use_true_forecast=use_true,
        verbose=args.verbose,
    )

    label = (f"{args.city} | excl={args.exclusion_range}°F | lead={args.entry_lead}h "
             f"| pos=${args.position}")
    summarize(trades, label=label)

    # Per-bin breakdown
    if trades:
        print(f"\n  Per-bin breakdown:")
        bin_stats: dict[int, dict] = {}
        for t in trades:
            bi = t.bin_index
            if bi not in bin_stats:
                bin_stats[bi] = {"label": t.bin_label, "n": 0, "wins": 0, "pnl": 0}
            bin_stats[bi]["n"] += 1
            bin_stats[bi]["wins"] += int(t.won)
            bin_stats[bi]["pnl"] += t.pnl_usdc
        print(f"  {'Bin':>4} {'Label':20s} {'N':>5} {'Win%':>7} {'P&L':>10}")
        for bi in sorted(bin_stats):
            s = bin_stats[bi]
            print(f"  {bi:>4} {s['label']:20s} {s['n']:>5} "
                  f"{s['wins']/s['n']:>7.1%} ${s['pnl']:>9.2f}")


if __name__ == "__main__":
    main()
