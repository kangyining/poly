"""
Phase-1 backtest: replay price history against settled outcomes.

For each (city, date) with a known settlement, pick the NO price at a given
lead-hours window, apply the strategy filter, simulate PnL, and persist results.

Usage (via backtest.py wrapper):
    python backtest.py --strategy base_ev --days 60
    python backtest.py --strategy base_ev --date-from 2026-01-01 --date-to 2026-04-01
"""

import argparse
import json
import logging
import sys
import uuid
from datetime import datetime, timezone, date, timedelta
from pathlib import Path
from typing import Optional

sys.path.insert(0, str(Path(__file__).parent.parent))

from data.db import Database

log = logging.getLogger(__name__)

# Default entry window: between 20–28 hours before close (one snapshot per bin)
DEFAULT_LEAD_MIN = 20.0
DEFAULT_LEAD_MAX = 28.0


def _pick_entry_price(db: Database, city: str, target_date: str, bin_label: str,
                      lead_min: float, lead_max: float) -> Optional[tuple[float, float]]:
    """Return (yes_price, hours_to_close) for the snapshot closest to lead_min."""
    row = db.conn.execute("""
        SELECT yes_price, hours_to_close FROM price_history
        WHERE city=? AND target_date=? AND bin_label=?
          AND hours_to_close >= ? AND hours_to_close <= ?
        ORDER BY ABS(hours_to_close - ?) ASC
        LIMIT 1
    """, (city, target_date, bin_label, lead_min, lead_max,
          (lead_min + lead_max) / 2)).fetchone()
    return (row["yes_price"], row["hours_to_close"]) if row else None


def run_backtest(
    db: Database,
    strategy_name: str,
    params: dict,
    date_from: date,
    date_to: date,
    lead_min: float = DEFAULT_LEAD_MIN,
    lead_max: float = DEFAULT_LEAD_MAX,
) -> dict:
    """
    Simulate trades for all settled (city, date) pairs in [date_from, date_to].
    Persists a backtests row and tagged positions rows. Returns a summary dict.
    """
    backtest_id = str(uuid.uuid4())
    created_at = datetime.now(timezone.utc).isoformat()

    # Insert backtest metadata (running)
    db.conn.execute("""
        INSERT INTO backtests (id, strategy, params, date_from, date_to, created_at, status)
        VALUES (?,?,?,?,?,?,'running')
    """, (backtest_id, strategy_name, json.dumps(params),
          date_from.isoformat(), date_to.isoformat(), created_at))
    db.conn.commit()

    min_ev = params.get("min_ev_threshold", 0.05)
    desired_shares = params.get("desired_shares", 10.0)
    max_stake = params.get("max_stake_usd", 50.0)
    funding_pct = params.get("funding_pct", 1.0)
    fee_rate = params.get("fee_rate", 0.10)

    # Fetch all settlements in range
    settlements = db.conn.execute("""
        SELECT city, target_date, winning_bin FROM settlement_records
        WHERE target_date >= ? AND target_date <= ?
        ORDER BY target_date, city
    """, (date_from.isoformat(), date_to.isoformat())).fetchall()

    trades = []
    total_stake = 0.0

    for row in settlements:
        city = row["city"]
        target_date = row["target_date"]
        winning_bin = row["winning_bin"]

        if not winning_bin:
            continue

        # Get all bins with price history for this city/date
        bins = db.conn.execute("""
            SELECT DISTINCT bin_label FROM price_history
            WHERE city=? AND target_date=?
        """, (city, target_date)).fetchall()

        day_stake = 0.0

        for bin_row in bins:
            bin_label = bin_row["bin_label"]

            entry = _pick_entry_price(db, city, target_date, bin_label, lead_min, lead_max)
            if entry is None:
                continue

            yes_price, hours_to_close = entry
            no_price = 1.0 - yes_price

            # EV of buying NO: (1 - p_market) × (1 - fee×p_market) - no_price
            # Simplified: edge = no_price - (implied NO fair value)
            # Strategy filter: if NO price has positive EV given model probs
            # Phase-1 simple: use market-implied probability as-is
            # EV_NO ≈ (1 - yes_price) - no_price × (1 + fee_rate)  [simplified]
            effective_ask = no_price * (1.0 + fee_rate)
            ev_no = (1.0 - yes_price) - effective_ask

            if ev_no < min_ev:
                continue

            shares = desired_shares
            stake = effective_ask * shares * funding_pct
            if day_stake + stake > max_stake or total_stake + stake > max_stake * 10:
                continue

            # Settle PnL
            if bin_label == winning_bin:
                pnl = -stake
            else:
                pnl = shares * (1.0 - effective_ask)

            day_stake += stake
            total_stake += stake

            trade = {
                "city": city,
                "target_date": target_date,
                "bin_label": bin_label,
                "winning_bin": winning_bin,
                "yes_price": yes_price,
                "no_price": no_price,
                "effective_ask": effective_ask,
                "ev_no": ev_no,
                "hours_to_close": hours_to_close,
                "shares": shares,
                "stake_usd": stake,
                "pnl": pnl,
                "won": bin_label != winning_bin,
            }
            trades.append(trade)

            # Persist simulated position
            db.conn.execute("""
                INSERT OR IGNORE INTO positions (
                    token_id, market_id, bin_label, event_slug, target_date,
                    entry_price, shares, stake_usd, order_id,
                    status, pnl, opened_at, settled_at, is_paper,
                    strategy_name, backtest_id, trade_rationale
                ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            """, (
                f"backtest-{backtest_id[:8]}-{city}-{target_date}-{bin_label}",
                f"backtest-{city}-{target_date}",
                bin_label,
                f"highest-temperature-in-{city}-on-{target_date}",
                target_date,
                effective_ask, shares, stake,
                None,
                "closed", round(pnl, 4),
                created_at, created_at,
                0,
                strategy_name, backtest_id,
                json.dumps({"ev_no": ev_no, "yes_price": yes_price,
                            "fee_rate": fee_rate, "hours_to_close": hours_to_close}),
            ))

    db.conn.commit()

    # Calculate metrics
    total_pnl = sum(t["pnl"] for t in trades)
    wins = sum(1 for t in trades if t["won"])
    win_rate = wins / len(trades) if trades else 0.0

    # Sharpe: daily PnL series
    from collections import defaultdict
    daily = defaultdict(float)
    for t in trades:
        daily[t["target_date"]] += t["pnl"]
    daily_pnls = list(daily.values())
    if len(daily_pnls) > 1:
        import statistics
        mean_pnl = statistics.mean(daily_pnls)
        std_pnl = statistics.stdev(daily_pnls)
        sharpe = (mean_pnl / std_pnl * (252 ** 0.5)) if std_pnl > 0 else 0.0
    else:
        sharpe = 0.0

    # Update backtests row
    db.conn.execute("""
        UPDATE backtests SET
            total_trades=?, win_rate=?, total_pnl=?, sharpe=?, status='done'
        WHERE id=?
    """, (len(trades), win_rate, round(total_pnl, 4), round(sharpe, 4), backtest_id))
    db.conn.commit()

    summary = {
        "backtest_id": backtest_id,
        "strategy": strategy_name,
        "params": params,
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
        "total_trades": len(trades),
        "wins": wins,
        "win_rate": round(win_rate, 4),
        "total_pnl": round(total_pnl, 4),
        "sharpe": round(sharpe, 4),
        "total_stake": round(total_stake, 4),
    }
    log.info("Backtest complete: %s", summary)
    return summary


def _load_strategy_params(strategy_name: str, strategies_toml: Optional[Path]) -> dict:
    """Load params from strategies.toml if present, else return defaults."""
    if strategies_toml and strategies_toml.exists():
        try:
            import tomllib  # Python 3.11+
        except ImportError:
            try:
                import tomli as tomllib
            except ImportError:
                log.warning("tomllib not available, using default strategy params")
                return {}

        with open(strategies_toml, "rb") as f:
            data = tomllib.load(f)
        for s in data.get("strategy", []):
            if s["name"] == strategy_name:
                return {
                    "min_ev_threshold": s.get("min_ev_threshold", 0.05),
                    "desired_shares": s.get("desired_shares", 10.0),
                    "max_stake_usd": s.get("max_stake_usd", 50.0),
                    "funding_pct": s.get("funding_pct", 1.0),
                    "fee_rate": 0.10,
                }
        log.warning("Strategy '%s' not found in %s, using defaults", strategy_name, strategies_toml)
    return {
        "min_ev_threshold": 0.05,
        "desired_shares": 10.0,
        "max_stake_usd": 50.0,
        "funding_pct": 1.0,
        "fee_rate": 0.10,
    }


def main():
    parser = argparse.ArgumentParser(description="Run a strategy backtest")
    parser.add_argument("--strategy", default="base_ev", help="Strategy name")
    parser.add_argument("--days", type=int, default=60, help="Days of history to backtest")
    parser.add_argument("--date-from", help="Start date YYYY-MM-DD (overrides --days)")
    parser.add_argument("--date-to", help="End date YYYY-MM-DD (default: yesterday)")
    parser.add_argument("--lead-min", type=float, default=DEFAULT_LEAD_MIN,
                        help="Min hours before close to use as entry price")
    parser.add_argument("--lead-max", type=float, default=DEFAULT_LEAD_MAX,
                        help="Max hours before close to use as entry price")
    parser.add_argument("--strategies-file", default="../poly-weather-bot/strategies.toml",
                        help="Path to strategies.toml")
    parser.add_argument("--db", default=None, help="Path to data.db")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s — %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=[logging.StreamHandler(sys.stdout)],
    )

    today = datetime.now(timezone.utc).date()
    date_to = date.fromisoformat(args.date_to) if args.date_to else today - timedelta(days=1)
    if args.date_from:
        date_from = date.fromisoformat(args.date_from)
    else:
        date_from = date_to - timedelta(days=args.days - 1)

    strategies_file = Path(args.strategies_file) if args.strategies_file else None
    params = _load_strategy_params(args.strategy, strategies_file)

    from data.db import Database
    db = Database() if not args.db else Database(Path(args.db))

    summary = run_backtest(
        db=db,
        strategy_name=args.strategy,
        params=params,
        date_from=date_from,
        date_to=date_to,
        lead_min=args.lead_min,
        lead_max=args.lead_max,
    )

    print(json.dumps(summary, indent=2))
    db.close()


if __name__ == "__main__":
    main()
