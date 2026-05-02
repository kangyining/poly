import unittest
from datetime import datetime, timezone, timedelta

from analysis.bankroll_backtest import ReplayCandidate, apply_bankroll_ledger


def candidate(
    opened_at: datetime,
    resolved_at: datetime,
    stake: float = 10.0,
    pnl: float = 2.0,
    city: str = "NYC",
    label: str = "70-71°F",
) -> ReplayCandidate:
    return ReplayCandidate(
        strategy_name="base_ev",
        strategy_plugin="base_no_ev",
        city=city,
        target_date=opened_at.date().isoformat(),
        event_slug=f"highest-temperature-in-nyc-on-{opened_at.date().isoformat()}",
        bin_label=label,
        bin_index=0,
        side="NO",
        opened_at=opened_at,
        resolved_at=resolved_at,
        desired_stake_usd=stake,
        desired_shares=10.0,
        pnl=pnl,
        won=pnl > 0,
        effective_ask=0.5,
        fee_rate=0.05,
        p_model=0.1,
        edge=0.1,
        fill_source="price_history_proxy",
        token_id="token",
        market_id="market",
        metadata={},
    )


class BankrollLedgerTests(unittest.TestCase):
    def test_full_size_trade_reserves_and_releases_cash(self):
        opened = datetime(2026, 4, 1, 12, tzinfo=timezone.utc)
        resolved = opened + timedelta(days=1)
        result = apply_bankroll_ledger([candidate(opened, resolved, stake=10.0, pnl=3.0)], initial_bankroll=100.0)

        self.assertEqual(result["summary"]["trades_opened"], 1)
        self.assertEqual(result["summary"]["realized_pnl"], 3.0)
        self.assertEqual(result["summary"]["final_equity"], 103.0)
        self.assertEqual(result["trades"][0]["cash_before"], 100.0)
        self.assertEqual(result["trades"][0]["cash_after"], 90.0)

    def test_insufficient_cash_below_minimum_skips_candidate(self):
        opened = datetime(2026, 4, 1, 12, tzinfo=timezone.utc)
        result = apply_bankroll_ledger(
            [candidate(opened, opened + timedelta(days=1), stake=10.0, pnl=3.0)],
            initial_bankroll=4.0,
            min_trade_stake_usd=5.0,
        )

        self.assertEqual(result["summary"]["trades_opened"], 0)
        self.assertEqual(result["summary"]["trades_skipped_insufficient_cash"], 1)
        self.assertEqual(result["summary"]["final_equity"], 4.0)
        self.assertEqual(result["skips"][0]["reason"], "insufficient_cash")

    def test_cash_between_minimum_and_desired_scales_trade(self):
        opened = datetime(2026, 4, 1, 12, tzinfo=timezone.utc)
        result = apply_bankroll_ledger(
            [candidate(opened, opened + timedelta(days=1), stake=10.0, pnl=4.0)],
            initial_bankroll=6.0,
            min_trade_stake_usd=5.0,
            allow_scale_down=True,
        )

        self.assertEqual(result["summary"]["trades_opened"], 1)
        self.assertEqual(result["summary"]["trades_scaled"], 1)
        self.assertAlmostEqual(result["trades"][0]["scale_factor"], 0.6)
        self.assertAlmostEqual(result["trades"][0]["allocated_stake_usd"], 6.0)
        self.assertAlmostEqual(result["trades"][0]["pnl"], 2.4)
        self.assertAlmostEqual(result["summary"]["final_equity"], 8.4)

    def test_scale_down_can_be_disabled(self):
        opened = datetime(2026, 4, 1, 12, tzinfo=timezone.utc)
        result = apply_bankroll_ledger(
            [candidate(opened, opened + timedelta(days=1), stake=10.0, pnl=4.0)],
            initial_bankroll=6.0,
            min_trade_stake_usd=5.0,
            allow_scale_down=False,
        )

        self.assertEqual(result["summary"]["trades_opened"], 0)
        self.assertEqual(result["summary"]["trades_skipped_insufficient_cash"], 1)

    def test_drawdown_and_utilization_are_recorded(self):
        opened = datetime(2026, 4, 1, 12, tzinfo=timezone.utc)
        losing = candidate(opened, opened + timedelta(hours=12), stake=10.0, pnl=-10.0)
        winning = candidate(opened + timedelta(days=1), opened + timedelta(days=2), stake=10.0, pnl=5.0)

        result = apply_bankroll_ledger([losing, winning], initial_bankroll=100.0)

        self.assertGreater(result["summary"]["max_drawdown"], 0.0)
        self.assertGreater(result["summary"]["max_locked_capital"], 0.0)
        self.assertGreater(result["summary"]["max_capital_utilization"], 0.0)
        self.assertGreater(result["summary"]["avg_capital_utilization"], 0.0)


if __name__ == "__main__":
    unittest.main()
