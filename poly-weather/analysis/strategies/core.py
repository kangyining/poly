"""Strategy plugin primitives for the weather backtest lab."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Protocol


@dataclass(frozen=True)
class ForecastState:
    source: str
    forecast_f: float
    lead_hours: float
    beta_f: float
    sigma_f: float
    mu_f: float
    gfs_f: float | None = None
    ecmwf_f: float | None = None
    model_spread_f: float | None = None


@dataclass(frozen=True)
class MarketBin:
    bin_label: str
    bin_index: int
    yes_price: float
    no_price: float
    effective_ask_no: float
    fee_rate: float
    hours_to_close: float
    token_id_yes: str | None = None
    token_id_no: str | None = None
    no_best_bid: float | None = None
    no_best_ask: float | None = None
    fill_source: str = "price_history_proxy"
    opened_at: str | None = None


@dataclass(frozen=True)
class StrategyContext:
    city: str
    target_date: str
    winning_bin: str
    bins: list[MarketBin]
    forecast: ForecastState | None
    params: dict[str, Any]
    replay_mode: str
    evaluation_time: datetime | None = None
    event_slug: str | None = None
    settlement_temp_f: float | None = None
    metar_max_so_far_f: float | None = None
    climatology_temps_f: list[float] = field(default_factory=list)


@dataclass
class StrategyCandidate:
    strategy_name: str
    side: str
    bin_label: str
    effective_ask: float
    fee_rate: float
    shares: float
    stake_usd: float
    p_model: float | None
    edge: float
    fill_source: str
    market_id: str
    token_id: str
    event_slug: str
    opened_at: str | None = None
    settled_at: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_trade(self, pnl: float, won: bool) -> dict[str, Any]:
        rationale = dict(self.metadata)
        rationale.update(
            {
                "strategy": self.strategy_name,
                "side": self.side,
                "p_model": self.p_model,
                "edge": self.edge,
                "effective_ask": self.effective_ask,
                "fee_rate": self.fee_rate,
                "fill_source": self.fill_source,
            }
        )
        return {
            "bin_label": self.bin_label,
            "effective_ask": self.effective_ask,
            "fee_rate": self.fee_rate,
            "shares": self.shares,
            "stake_usd": self.stake_usd,
            "pnl": pnl,
            "won": won,
            "token_id": self.token_id,
            "market_id": self.market_id,
            "event_slug": self.event_slug,
            "opened_at": self.opened_at,
            "settled_at": self.settled_at,
            "trade_rationale": rationale,
        }


@dataclass(frozen=True)
class StrategyDiagnostic:
    strategy_name: str
    severity: str
    reason: str
    required_data: tuple[str, ...] = ()


@dataclass
class StrategyOutput:
    candidates: list[StrategyCandidate] = field(default_factory=list)
    diagnostics: list[StrategyDiagnostic] = field(default_factory=list)


class Strategy(Protocol):
    name: str
    required_data: tuple[str, ...]

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        ...

    def explain(self, candidate: StrategyCandidate) -> dict[str, Any]:
        ...


class BaseStrategy:
    name = "base"
    required_data: tuple[str, ...] = ()

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        raise NotImplementedError

    def explain(self, candidate: StrategyCandidate) -> dict[str, Any]:
        return dict(candidate.metadata)

    def missing(self, reason: str, *required_data: str) -> StrategyOutput:
        return StrategyOutput(
            diagnostics=[
                StrategyDiagnostic(
                    strategy_name=self.name,
                    severity="missing_data",
                    reason=reason,
                    required_data=tuple(required_data),
                )
            ]
        )
