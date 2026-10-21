from __future__ import annotations

import unittest

from analysis.strategies.core import ForecastState, MarketBin, StrategyContext
from analysis.strategies.plugins import BasketArbStrategy, CorridorLadderStrategy, ThresholdUnionStrategy
from analysis.strategies.utils import bucket_probability, entry_cost, parse_temperature_range


def make_context() -> StrategyContext:
    bins = [
        MarketBin("48F or below", 0, 0.05, 0.95, 0.95, 0.05, 24),
        MarketBin("49F to 51F", 1, 0.35, 0.65, 0.65, 0.05, 24),
        MarketBin("52F to 54F", 2, 0.40, 0.60, 0.60, 0.05, 24),
        MarketBin("55F or above", 3, 0.05, 0.95, 0.95, 0.05, 24),
    ]
    return StrategyContext(
        city="NYC",
        target_date="2026-04-20",
        winning_bin="52F to 54F",
        bins=bins,
        forecast=ForecastState(
            source="test",
            forecast_f=52.0,
            lead_hours=24.0,
            beta_f=0.0,
            sigma_f=2.0,
            mu_f=52.0,
            gfs_f=52.0,
            ecmwf_f=53.0,
        ),
        params={"desired_shares": 10.0, "funding_pct": 1.0, "min_ev_threshold": 0.01, "max_stake_usd": 50.0},
        replay_mode="interval",
    )


class StrategyUtilsTests(unittest.TestCase):
    def test_parse_temperature_ranges(self):
        self.assertEqual(parse_temperature_range("48F or below").kind, "below_or_equal")
        self.assertEqual(parse_temperature_range("49F to 51F").kind, "between")
        self.assertEqual(parse_temperature_range("55F or above").kind, "above_or_equal")

    def test_bucket_probability_is_reasonable(self):
        p_center = bucket_probability("49F to 51F", 50.0, 2.0)
        p_tail = bucket_probability("60F or above", 50.0, 2.0)
        self.assertGreater(p_center, 0.35)
        self.assertLess(p_tail, 0.01)

    def test_fee_cost_formula(self):
        self.assertAlmostEqual(entry_cost(0.5, 0.05), 0.5125)


class StrategyPluginTests(unittest.TestCase):
    def test_corridor_strategy_emits_adjacent_group(self):
        ctx = make_context()
        out = CorridorLadderStrategy().generate_candidates(ctx)
        self.assertTrue(any(c.side == "YES_CORRIDOR" for c in out.candidates))
        self.assertTrue(all("underlying_bins" in c.metadata for c in out.candidates))

    def test_basket_arb_requires_positive_full_basket_edge(self):
        ctx = make_context()
        ctx.bins[:] = [
            MarketBin("48F or below", 0, 0.40, 0.60, 0.60, 0.05, 24),
            MarketBin("49F to 51F", 1, 0.40, 0.60, 0.60, 0.05, 24),
            MarketBin("52F to 54F", 2, 0.40, 0.60, 0.60, 0.05, 24),
        ]
        out = BasketArbStrategy().generate_candidates(ctx)
        self.assertEqual(out.candidates, [])

    def test_threshold_union_can_synthesize_tail(self):
        ctx = make_context()
        out = ThresholdUnionStrategy().generate_candidates(ctx)
        self.assertTrue(out.candidates)
        self.assertTrue(any(c.side in {"YES_LOW_TAIL", "YES_HIGH_TAIL"} for c in out.candidates))


if __name__ == "__main__":
    unittest.main()
