"""Shared strategy math and parsing helpers."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass


BELOW_PATTERN = re.compile(
    r"(?i)(?:≤|<=)?\s*(\d+(?:\.\d+)?)\s*(?:°?\s*f)?\s*(?:or below|and below|below|or lower|and lower|lower)?"
)
ABOVE_PATTERN = re.compile(
    r"(?i)(?:≥|>=)?\s*(\d+(?:\.\d+)?)\s*(?:°?\s*f)?\s*(?:or above|and above|above|or higher|and higher|higher)?"
)
BETWEEN_PATTERN = re.compile(
    r"(?i)(\d+(?:\.\d+)?)\s*(?:°?\s*f)?\s*(?:to|-|through)\s*(\d+(?:\.\d+)?)"
)


@dataclass(frozen=True)
class TemperatureRange:
    kind: str
    low_f: float | None = None
    high_f: float | None = None

    @property
    def sort_key(self) -> float:
        if self.kind == "below_or_equal":
            return float("-inf") if self.high_f is None else self.high_f
        if self.kind == "above_or_equal":
            return float("inf") if self.low_f is None else self.low_f
        return self.low_f or 0.0


def parse_temperature_range(label: str) -> TemperatureRange | None:
    normalized = label.replace("°", "").replace("F", "").replace("f", "")
    between = BETWEEN_PATTERN.search(normalized)
    if between:
        return TemperatureRange("between", float(between.group(1)), float(between.group(2)))

    lower_marked = any(token in label.lower() for token in ("below", "lower", "≤", "<="))
    upper_marked = any(token in label.lower() for token in ("above", "higher", "≥", ">="))
    if lower_marked:
        match = re.search(r"(\d+(?:\.\d+)?)", normalized)
        if match:
            return TemperatureRange("below_or_equal", high_f=float(match.group(1)))
    if upper_marked:
        match = re.search(r"(\d+(?:\.\d+)?)", normalized)
        if match:
            return TemperatureRange("above_or_equal", low_f=float(match.group(1)))

    return None


def norm_cdf(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


def bucket_probability(label: str, mu_f: float, sigma_f: float) -> float | None:
    parsed = parse_temperature_range(label)
    if parsed is None or sigma_f <= 0:
        return None
    if parsed.kind == "below_or_equal":
        assert parsed.high_f is not None
        return norm_cdf((parsed.high_f + 0.5 - mu_f) / sigma_f)
    if parsed.kind == "above_or_equal":
        assert parsed.low_f is not None
        return 1.0 - norm_cdf((parsed.low_f - 0.5 - mu_f) / sigma_f)
    assert parsed.low_f is not None and parsed.high_f is not None
    hi = norm_cdf((parsed.high_f + 0.5 - mu_f) / sigma_f)
    lo = norm_cdf((parsed.low_f - 0.5 - mu_f) / sigma_f)
    return max(0.0, hi - lo)


def entry_cost(price: float, fee_rate: float) -> float:
    return price * (1.0 + fee_rate * (1.0 - price))


def settle_single(side: str, bin_label: str, winning_bin: str, shares: float, cost_per_share: float) -> tuple[float, bool]:
    side = side.upper()
    won = (bin_label == winning_bin and side == "YES") or (bin_label != winning_bin and side == "NO")
    if won:
        return shares * (1.0 - cost_per_share), True
    return -(shares * cost_per_share), False


def settle_union(winning_bin: str, included_bins: list[str], shares: float, total_cost_per_share: float) -> tuple[float, bool]:
    won = winning_bin in included_bins
    if won:
        return shares * (1.0 - total_cost_per_share), True
    return -(shares * total_cost_per_share), False


def candidate_shares(params: dict, multiplier: float = 1.0) -> float:
    return float(params.get("desired_shares", 10.0)) * float(params.get("funding_pct", 1.0)) * multiplier


def candidate_size(price: float, fee_rate: float, params: dict, multiplier: float = 1.0) -> tuple[float, float]:
    fixed_stake = params.get("fixed_trade_stake_usd")
    cost = entry_cost(price, fee_rate)
    if fixed_stake is not None:
        stake = float(fixed_stake) * multiplier
        if cost <= 0:
            return 0.0, 0.0
        return stake / cost, stake
    shares = candidate_shares(params, multiplier)
    return shares, shares * cost


def min_edge(params: dict, default: float = 0.05) -> float:
    return float(params.get("min_ev_threshold", params.get("min_edge", default)))
