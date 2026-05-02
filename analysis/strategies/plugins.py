"""Concrete strategy plugins."""

from __future__ import annotations

from itertools import combinations

from .core import BaseStrategy, MarketBin, StrategyCandidate, StrategyContext, StrategyDiagnostic, StrategyOutput
from .utils import bucket_probability, candidate_size, entry_cost, min_edge, parse_temperature_range


def _event_slug(context: StrategyContext) -> str:
    return context.event_slug or f"highest-temperature-in-{context.city}-on-{context.target_date}"


def _market_id(context: StrategyContext, label: str) -> str:
    return f"backtest-{context.city}-{context.target_date}-{label}"


def _token(context: StrategyContext, side: str, label: str) -> str:
    safe = label.replace(" ", "_").replace("/", "_")
    return f"backtest-{context.city}-{context.target_date}-{side.lower()}-{safe}"


def _single_candidate(
    strategy: str,
    context: StrategyContext,
    market: MarketBin,
    side: str,
    p_model: float,
    edge: float,
    price: float,
    fee_rate: float,
    metadata: dict,
) -> StrategyCandidate:
    shares, stake_usd = candidate_size(price, fee_rate, context.params)
    cost = entry_cost(price, fee_rate)
    token_id = market.token_id_no if side == "NO" else market.token_id_yes
    return StrategyCandidate(
        strategy_name=strategy,
        side=side,
        bin_label=market.bin_label,
        effective_ask=price,
        fee_rate=fee_rate,
        shares=shares,
        stake_usd=stake_usd,
        p_model=p_model,
        edge=edge,
        fill_source=market.fill_source,
        market_id=_market_id(context, market.bin_label),
        token_id=token_id or _token(context, side, market.bin_label),
        event_slug=_event_slug(context),
        opened_at=market.opened_at,
        metadata=metadata,
    )


class BaseNoEvStrategy(BaseStrategy):
    name = "base_no_ev"
    required_data = ("forecast_records", "price_history_or_market_snapshots", "settlement_records")

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        out = StrategyOutput()
        threshold = min_edge(context.params)
        for market in context.bins:
            p_yes = bucket_probability(market.bin_label, context.forecast.mu_f, context.forecast.sigma_f)
            if p_yes is None:
                out.diagnostics.append(StrategyDiagnostic(self.name, "skip", f"unparseable bin {market.bin_label}"))
                continue
            cost = entry_cost(market.effective_ask_no, market.fee_rate)
            edge = (1.0 - p_yes) - cost
            if edge >= threshold:
                out.candidates.append(
                    _single_candidate(
                        self.name,
                        context,
                        market,
                        "NO",
                        p_yes,
                        edge,
                        market.effective_ask_no,
                        market.fee_rate,
                        {
                            "mu_f": context.forecast.mu_f,
                            "sigma_f": context.forecast.sigma_f,
                            "forecast_source": context.forecast.source,
                        },
                    )
                )
        return out


class BaseYesNoEvStrategy(BaseStrategy):
    name = "base_yes_no_ev"
    required_data = BaseNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        out = StrategyOutput()
        threshold = min_edge(context.params)
        for market in context.bins:
            p_yes = bucket_probability(market.bin_label, context.forecast.mu_f, context.forecast.sigma_f)
            if p_yes is None:
                continue
            yes_cost = entry_cost(market.yes_price, market.fee_rate)
            yes_edge = p_yes - yes_cost
            if yes_edge >= threshold:
                out.candidates.append(
                    _single_candidate(
                        self.name, context, market, "YES", p_yes, yes_edge, market.yes_price, market.fee_rate,
                        {"mu_f": context.forecast.mu_f, "sigma_f": context.forecast.sigma_f}
                    )
                )
            no_cost = entry_cost(market.effective_ask_no, market.fee_rate)
            no_edge = (1.0 - p_yes) - no_cost
            if no_edge >= threshold:
                out.candidates.append(
                    _single_candidate(
                        self.name, context, market, "NO", p_yes, no_edge, market.effective_ask_no, market.fee_rate,
                        {"mu_f": context.forecast.mu_f, "sigma_f": context.forecast.sigma_f}
                    )
                )
        return out


class EnsembleBucketEvStrategy(BaseStrategy):
    name = "ensemble_bucket_ev"
    required_data = ("forecast_records.gfs_f", "forecast_records.ecmwf_f", "price_history_or_market_snapshots")

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        members = [v for v in (context.forecast.gfs_f, context.forecast.ecmwf_f) if v is not None]
        if len(members) < 2:
            return self.missing("requires at least two model forecasts", *self.required_data)
        out = StrategyOutput()
        threshold = min_edge(context.params)
        sigma = max(context.forecast.sigma_f, float(context.params.get("ensemble_sigma_floor", 1.2)))
        for market in context.bins:
            probs = [bucket_probability(market.bin_label, member + context.forecast.beta_f, sigma) for member in members]
            probs = [p for p in probs if p is not None]
            if not probs:
                continue
            p_yes = sum(probs) / len(probs)
            for side, price, payoff_prob in (
                ("YES", market.yes_price, p_yes),
                ("NO", market.effective_ask_no, 1.0 - p_yes),
            ):
                cost = entry_cost(price, market.fee_rate)
                edge = payoff_prob - cost
                if edge >= threshold:
                    out.candidates.append(
                        _single_candidate(
                            self.name, context, market, side, p_yes, edge, price, market.fee_rate,
                            {"members_f": members, "member_count": len(members), "sigma_f": sigma}
                        )
                    )
        return out


class DynamicSigmaEvStrategy(BaseYesNoEvStrategy):
    name = "dynamic_sigma_ev"
    required_data = BaseYesNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        spread = context.forecast.model_spread_f or 0.0
        lead_component = max(0.0, context.forecast.lead_hours - 6.0) / 36.0
        dynamic_sigma = max(
            context.forecast.sigma_f,
            float(context.params.get("dynamic_sigma_min", 0.8)) + lead_component,
            spread * float(context.params.get("spread_sigma_multiplier", 0.5)),
        )
        adjusted = context.forecast.__class__(
            **{**context.forecast.__dict__, "sigma_f": min(dynamic_sigma, float(context.params.get("dynamic_sigma_max", 5.5)))}
        )
        return super().generate_candidates(context.__class__(**{**context.__dict__, "forecast": adjusted}))


class BayesianClimatologyEvStrategy(BaseYesNoEvStrategy):
    name = "bayesian_climatology_ev"
    required_data = ("settlement_records.settlement_temp_f",) + BaseYesNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        if not context.climatology_temps_f:
            return self.missing("no climatology settlement temperatures available", "settlement_records")
        out = StrategyOutput()
        threshold = min_edge(context.params)
        model_weight = float(context.params.get("model_weight", 0.7))
        for market in context.bins:
            model_p = bucket_probability(market.bin_label, context.forecast.mu_f, context.forecast.sigma_f)
            if model_p is None:
                continue
            clim_hits = 0
            for temp in context.climatology_temps_f:
                parsed = parse_temperature_range(market.bin_label)
                if parsed is None:
                    continue
                if parsed.kind == "below_or_equal" and parsed.high_f is not None and temp <= parsed.high_f:
                    clim_hits += 1
                elif parsed.kind == "above_or_equal" and parsed.low_f is not None and temp >= parsed.low_f:
                    clim_hits += 1
                elif parsed.kind == "between" and parsed.low_f is not None and parsed.high_f is not None and parsed.low_f <= temp <= parsed.high_f:
                    clim_hits += 1
            clim_p = clim_hits / len(context.climatology_temps_f)
            p_yes = model_weight * model_p + (1.0 - model_weight) * clim_p
            for side, price, payoff_prob in (
                ("YES", market.yes_price, p_yes),
                ("NO", market.effective_ask_no, 1.0 - p_yes),
            ):
                cost = entry_cost(price, market.fee_rate)
                edge = payoff_prob - cost
                if edge >= threshold:
                    out.candidates.append(
                        _single_candidate(
                            self.name, context, market, side, p_yes, edge, price, market.fee_rate,
                            {"model_p": model_p, "climatology_p": clim_p, "model_weight": model_weight}
                        )
                    )
        return out


class CorridorLadderStrategy(BaseStrategy):
    name = "corridor_ladder"
    required_data = BaseNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        out = StrategyOutput()
        bins = sorted(context.bins, key=lambda b: b.bin_index)
        if len(bins) < 2:
            return self.missing("needs multiple bucket markets", "price_history")
        threshold = float(context.params.get("corridor_min_edge", min_edge(context.params)))
        max_width = int(context.params.get("max_corridor_width", 3))
        for width in range(2, max_width + 1):
            for start in range(0, len(bins) - width + 1):
                group = bins[start:start + width]
                probs = [bucket_probability(m.bin_label, context.forecast.mu_f, context.forecast.sigma_f) for m in group]
                if any(p is None for p in probs):
                    continue
                p_yes = sum(p for p in probs if p is not None)
                total_cost = sum(entry_cost(m.yes_price, m.fee_rate) for m in group)
                edge = p_yes - total_cost
                if edge >= threshold:
                    shares, stake_usd = candidate_size(total_cost, 0.0, context.params)
                    labels = [m.bin_label for m in group]
                    label = "corridor:" + "|".join(labels)
                    out.candidates.append(
                        StrategyCandidate(
                            strategy_name=self.name,
                            side="YES_CORRIDOR",
                            bin_label=label,
                            effective_ask=total_cost,
                            fee_rate=max(m.fee_rate for m in group),
                            shares=shares,
                            stake_usd=stake_usd,
                            p_model=p_yes,
                            edge=edge,
                            fill_source="corridor_sum",
                            market_id=_market_id(context, label),
                            token_id=_token(context, "corridor", label),
                            event_slug=_event_slug(context),
                            opened_at=group[0].opened_at,
                            metadata={"underlying_bins": labels, "width": width},
                        )
                    )
        return out


class ThresholdUnionStrategy(CorridorLadderStrategy):
    name = "threshold_union"
    required_data = ("market_relationships_or_bucket_ladder",) + BaseNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        out = StrategyOutput()
        bins = sorted(context.bins, key=lambda b: b.bin_index)
        if len(bins) < 3:
            return self.missing("needs bucket ladder to synthesize thresholds", "price_history")
        threshold = min_edge(context.params)
        for cut in range(1, len(bins)):
            for side_name, group in (("LOW_TAIL", bins[:cut]), ("HIGH_TAIL", bins[cut:])):
                probs = [bucket_probability(m.bin_label, context.forecast.mu_f, context.forecast.sigma_f) for m in group]
                if any(p is None for p in probs):
                    continue
                p_yes = sum(p for p in probs if p is not None)
                total_cost = sum(entry_cost(m.yes_price, m.fee_rate) for m in group)
                edge = p_yes - total_cost
                if edge >= threshold:
                    shares, stake_usd = candidate_size(total_cost, 0.0, context.params)
                    labels = [m.bin_label for m in group]
                    label = f"threshold_{side_name.lower()}:" + "|".join(labels)
                    out.candidates.append(
                        StrategyCandidate(
                            strategy_name=self.name,
                            side=f"YES_{side_name}",
                            bin_label=label,
                            effective_ask=total_cost,
                            fee_rate=max(m.fee_rate for m in group),
                            shares=shares,
                            stake_usd=stake_usd,
                            p_model=p_yes,
                            edge=edge,
                            fill_source="threshold_union_sum",
                            market_id=_market_id(context, label),
                            token_id=_token(context, side_name, label),
                            event_slug=_event_slug(context),
                            opened_at=group[0].opened_at,
                            metadata={"underlying_bins": labels, "cut_index": cut},
                        )
                    )
        return out


class BasketArbStrategy(BaseStrategy):
    name = "basket_arb"
    required_data = ("complete_bucket_ladder", "executable_yes_prices")

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        bins = sorted(context.bins, key=lambda b: b.bin_index)
        if len(bins) < 3:
            return self.missing("needs full multi-bucket event", "price_history")
        total_cost = sum(entry_cost(m.yes_price, m.fee_rate) for m in bins)
        edge = 1.0 - total_cost
        if edge < float(context.params.get("arb_buffer", min_edge(context.params, 0.01))):
            return StrategyOutput()
        shares, stake_usd = candidate_size(total_cost, 0.0, context.params)
        labels = [m.bin_label for m in bins]
        return StrategyOutput(
            candidates=[
                StrategyCandidate(
                    strategy_name=self.name,
                    side="YES_BASKET",
                    bin_label="basket:" + "|".join(labels),
                    effective_ask=total_cost,
                    fee_rate=max(m.fee_rate for m in bins),
                    shares=shares,
                    stake_usd=stake_usd,
                    p_model=1.0,
                    edge=edge,
                    fill_source="basket_sum",
                    market_id=_market_id(context, "basket"),
                    token_id=_token(context, "basket", "all"),
                    event_slug=_event_slug(context),
                    opened_at=bins[0].opened_at,
                    metadata={"underlying_bins": labels},
                )
            ]
        )


class MakerQuoteSimStrategy(BaseStrategy):
    name = "maker_quote_sim"
    required_data = ("market_snapshots.no_best_bid", "forecast_records")

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        out = StrategyOutput()
        threshold = min_edge(context.params)
        tick = float(context.params.get("tick_size", 0.01))
        allow_proxy = bool(context.params.get("maker_proxy_from_price_history", True))
        for market in context.bins:
            no_best_bid = market.no_best_bid
            proxy_used = False
            if no_best_bid is None and allow_proxy:
                no_best_bid = max(0.0, market.no_price - tick)
                proxy_used = True
            if no_best_bid is None:
                continue
            p_yes = bucket_probability(market.bin_label, context.forecast.mu_f, context.forecast.sigma_f)
            if p_yes is None:
                continue
            quote = min(market.effective_ask_no, no_best_bid + tick)
            edge = (1.0 - p_yes) - quote
            if edge >= threshold:
                out.candidates.append(
                    _single_candidate(
                        self.name, context, market, "NO", p_yes, edge, quote, 0.0,
                        {
                            "maker_quote": quote,
                            "no_best_bid": no_best_bid,
                            "tick_size": tick,
                            "maker_proxy_from_price_history": proxy_used,
                        }
                    )
                )
        if not out.candidates and all(m.no_best_bid is None for m in context.bins):
            out.diagnostics.append(StrategyDiagnostic(self.name, "missing_data", "no maker-side bid snapshots", ("market_snapshots.no_best_bid",)))
        return out


class NowcastMetarStrategy(BaseYesNoEvStrategy):
    name = "nowcast_metar"
    required_data = ("metar_records",) + BaseYesNoEvStrategy.required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        if context.forecast is None:
            return self.missing("forecast unavailable", "forecast_records")
        if context.metar_max_so_far_f is None:
            return self.missing("no METAR max available before evaluation time", "metar_records")
        hours = min(max(context.bins[0].hours_to_close if context.bins else context.forecast.lead_hours, 0.0), 36.0)
        elapsed = 1.0 - (hours / 36.0)
        mu = max(context.forecast.mu_f, elapsed * context.metar_max_so_far_f + (1.0 - elapsed) * context.forecast.mu_f)
        sigma = max(0.6, context.forecast.sigma_f * max(0.35, (1.0 - elapsed) ** 0.5))
        adjusted = context.forecast.__class__(**{**context.forecast.__dict__, "mu_f": mu, "sigma_f": sigma})
        return super().generate_candidates(context.__class__(**{**context.__dict__, "forecast": adjusted}))


class UnsupportedResearchStrategy(BaseStrategy):
    def __init__(self, name: str, reason: str, required_data: tuple[str, ...]):
        self.name = name
        self.reason = reason
        self.required_data = required_data

    def generate_candidates(self, context: StrategyContext) -> StrategyOutput:
        return self.missing(self.reason, *self.required_data)


def all_strategy_instances() -> dict[str, BaseStrategy]:
    strategies: list[BaseStrategy] = [
        BaseNoEvStrategy(),
        BaseYesNoEvStrategy(),
        EnsembleBucketEvStrategy(),
        DynamicSigmaEvStrategy(),
        BayesianClimatologyEvStrategy(),
        CorridorLadderStrategy(),
        NowcastMetarStrategy(),
        ThresholdUnionStrategy(),
        BasketArbStrategy(),
        MakerQuoteSimStrategy(),
        UnsupportedResearchStrategy(
            "monthly_remainder",
            "monthly precipitation/snow remainder data is not normalized in this DB yet",
            ("monthly_realized_totals", "monthly_contract_metadata"),
        ),
        UnsupportedResearchStrategy(
            "cross_market_semantic_arb",
            "requires linked threshold/bucket markets with verified equivalent rule text",
            ("market_relationships",),
        ),
        UnsupportedResearchStrategy(
            "exit_overlay",
            "requires position lifecycle price path simulation after entry; tracked as a follow-up overlay",
            ("post_entry_price_path",),
        ),
        UnsupportedResearchStrategy(
            "sharp_wallet_copy",
            "requires configured wallet allowlist and fetched public Data API activity",
            ("wallet_signal_snapshots",),
        ),
        UnsupportedResearchStrategy(
            "resolution_lag_anomaly",
            "requires official source publication timestamps; settlement outcome alone would be lookahead",
            ("official_source_publication_times",),
        ),
    ]
    return {strategy.name: strategy for strategy in strategies}
