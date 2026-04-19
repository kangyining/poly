"""
Backtest runner — replays strategy against historical price + settlement data.

Usage:
    python backtest.py                                    # base_ev, last 60 days
    python backtest.py --strategy base_ev --days 90
    python backtest.py --strategy aggressive_tail --date-from 2026-01-01
    python backtest.py --strategy base_ev --date-from 2026-01-01 --date-to 2026-03-31
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from analysis.phase1_backtest import main

if __name__ == "__main__":
    main()
