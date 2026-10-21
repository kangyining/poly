"""
Position and PnL status viewer.

Shows open positions, closed positions, and overall PnL summary.

Usage:
    python status.py              # show everything
    python status.py --open       # open positions only
    python status.py --closed     # closed positions only
    python status.py --date 2026-04-19
"""

import argparse
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from data.db import Database


def fmt(val, prefix="$"):
    if val is None:
        return "—"
    return f"{prefix}{val:+.4f}" if prefix == "$" else f"{val:.4f}"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--open",   action="store_true", help="Show only open positions")
    parser.add_argument("--closed", action="store_true", help="Show only closed positions")
    parser.add_argument("--date",   help="Filter by target_date (YYYY-MM-DD)")
    args = parser.parse_args()

    logging.basicConfig(level=logging.WARNING)

    db = Database()

    where_clauses = []
    params = []

    if args.open and not args.closed:
        where_clauses.append("status='open'")
    elif args.closed and not args.open:
        where_clauses.append("status='closed'")

    if args.date:
        where_clauses.append("target_date=?")
        params.append(args.date)

    where = ("WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
    rows = db.conn.execute(
        f"SELECT bin_label, target_date, entry_price, shares, stake_usd, pnl, status, is_paper, opened_at "
        f"FROM positions {where} ORDER BY opened_at",
        params,
    ).fetchall()

    if not rows:
        print("No positions found.")
        db.close()
        return

    # Header
    print()
    print(f"  {'Bin':<18} {'Date':<12} {'Ask':>6} {'Shares':>6} {'Stake':>7} {'PnL':>9} {'Status':<8} {'Type'}")
    print("  " + "-" * 80)

    for r in rows:
        pnl_str = f"${r[5]:+.4f}" if r[5] is not None else "     —  "
        kind = "paper" if r[7] else "live"
        print(
            f"  {r[0]:<18} {r[1]:<12} {r[2]:>6.4f} {r[3]:>6.1f} "
            f"${r[4]:>6.2f} {pnl_str:>9} {r[6]:<8} {kind}"
        )

    # Summary
    summary = db.conn.execute(
        f"SELECT COUNT(*), SUM(stake_usd), SUM(CASE WHEN pnl IS NOT NULL THEN pnl ELSE 0 END) "
        f"FROM positions {where}",
        params,
    ).fetchone()

    total_pnl = summary[2] or 0.0
    total_stake = summary[1] or 0.0
    roi = (total_pnl / total_stake * 100) if total_stake else 0.0

    print("  " + "-" * 80)
    print(f"  {'TOTAL':<18} {'':12} {'':>6} {'':>6} ${total_stake:>6.2f} ${total_pnl:>+8.4f}   ROI: {roi:+.1f}%")
    print()

    db.close()


if __name__ == "__main__":
    main()
