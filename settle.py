"""
Daily settlement runner.

Checks yesterday's (or a specific date's) market for each city,
records the winning bin, closes matching open positions with PnL,
and refreshes open_positions.json for the Rust bot.

Usage:
    python settle.py                  # check yesterday
    python settle.py --date 2026-04-19
"""

import argparse
import logging
import sys
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from data.db import Database
from data.settlement_tracker import daily_check, backfill

log = logging.getLogger(__name__)

OPEN_POSITIONS_JSON = Path(__file__).parent / "open_positions.json"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--date", help="Specific date to settle (YYYY-MM-DD). Defaults to yesterday.")
    parser.add_argument("--backfill", type=int, metavar="DAYS",
                        help="Backfill settlements for the last N days instead of daily check.")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s — %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )

    db = Database()

    if args.backfill:
        log.info("Running settlement backfill for last %d days...", args.backfill)
        count = backfill(db, days_back=args.backfill)
        log.info("Backfill complete: %d total settlement records", count)
    else:
        if args.date:
            target = date.fromisoformat(args.date)
            log.info("Running settlement check for %s...", target)
        else:
            target = datetime.now(timezone.utc).date() - timedelta(days=1)
            log.info("Running settlement check for yesterday (%s)...", target)

        daily_check(db, open_positions_json=OPEN_POSITIONS_JSON, target_date=target)

    # Print summary
    open_pos = db.get_open_positions()
    closed = db.conn.execute(
        "SELECT COUNT(*), ROUND(SUM(pnl),4) FROM positions WHERE status='closed'"
    ).fetchone()

    print()
    print("=" * 40)
    print(f"  Open positions : {len(open_pos)}")
    print(f"  Closed positions: {closed[0]}")
    print(f"  Total PnL       : ${closed[1] or 0:.4f}")
    print("=" * 40)

    db.close()


if __name__ == "__main__":
    main()
