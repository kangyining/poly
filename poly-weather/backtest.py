"""
Backtest runner — the supported entry point for historical backtests.

This wrapper delegates to `analysis/phase1_backtest.py`, which owns the current
backtest flow and writes result artifacts to CSV/Markdown files.

Usage:
    python backtest.py                                    # base_ev, last 60 days, 1-minute interval replay
    python backtest.py --strategy base_ev --days 90
    python backtest.py --strategy aggressive_tail --date-from 2026-01-01
    python backtest.py --strategy base_ev --date-from 2026-01-01 --date-to 2026-03-31
    python backtest.py --strategy base_ev --days 60 --city NYC
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from analysis.phase1_backtest import main

if __name__ == "__main__":
    main()
