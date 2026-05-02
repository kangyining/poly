"""
Initial-deposit bankroll backtest runner.

This entrypoint keeps the existing turnover-style `backtest.py` separate while
using the same underlying replay data and strategy plugins.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from analysis.bankroll_backtest import main


if __name__ == "__main__":
    main()
