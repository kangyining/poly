"""
Data models for the Polymarket weather bot.

Units / conventions (verified Phase 0, 2026-04-06):
  - Temperatures stored in Fahrenheit (matches Polymarket settlement)
  - bin_label format: "38-39°F", "≤27°F", "≥46°F"
  - Polymarket settles on Wunderground data (whole °F)
  - Settlement stations are defined in config.weather_contracts
"""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional


@dataclass
class WeatherContract:
    """Normalized Polymarket weather contract metadata."""
    event_slug: str
    target_date: str
    market_type: str
    measure: str
    unit: str
    city_slug: str
    display_city: str
    resolution_source_kind: str
    station_id: str
    source_url: str
    timezone: str
    lat: float
    lon: float
    validation_status: str
    skip_reason: Optional[str] = None
    event_id: Optional[str] = None
    raw_json: Optional[str] = None


@dataclass
class MarketSnapshot:
    """Market price snapshot — recorded every 5 minutes per bin."""
    timestamp: datetime
    city: str
    target_date: str          # YYYY-MM-DD
    hours_to_close: float     # hours until noon UTC close for target_date
    event_id: str
    event_slug: str           # highest-temperature-in-{city-slug}-on-april-{dd}-2026
    bin_label: str            # "38-39°F", "≤27°F", "≥46°F"
    bin_index: int            # 0-10, low to high
    token_id_yes: str         # clobTokenIds[0]
    token_id_no: str          # clobTokenIds[1]
    yes_price: float          # last trade / mid price
    no_price: float
    yes_best_bid: float
    yes_best_ask: float
    no_best_bid: float
    no_best_ask: float
    book_depth_bid: float     # total USDC on bid side (NO book)
    book_depth_ask: float     # total USDC on ask side (NO book)
    volume_24h: float
    liquidity: float
    no_asks_json: Optional[str] = None
    fee_rate: Optional[float] = None
    no_vwap_10: Optional[float] = None
    neg_risk: bool = True
    market_type: Optional[str] = None
    measure: Optional[str] = None
    unit: Optional[str] = None
    city_slug: Optional[str] = None
    station_id: Optional[str] = None
    source_url: Optional[str] = None
    validation_status: Optional[str] = None
    skip_reason: Optional[str] = None


@dataclass
class METARRecord:
    """METAR observation — recorded every hour."""
    timestamp: datetime
    icao: str                 # settlement station ICAO code
    city: str
    temp_c: float
    temp_f: float             # temp_c * 9/5 + 32
    dewpoint_c: Optional[float]
    raw_metar: str
    obs_time: datetime        # actual observation time from METAR


@dataclass
class ForecastRecord:
    """Weather forecast snapshot — recorded every hour."""
    timestamp: datetime       # when we fetched
    city: str
    target_date: str          # YYYY-MM-DD
    source: str               # "open-meteo"
    temp_max_ecmwf_c: Optional[float]
    temp_max_gfs_c: Optional[float]
    temp_max_ecmwf_f: Optional[float]
    temp_max_gfs_f: Optional[float]
    model_spread_c: Optional[float]   # |ecmwf - gfs|
    forecast_lead_hours: float        # hours until target_date local midnight
    raw_json: str
    market_type: Optional[str] = None
    measure: Optional[str] = None
    unit: Optional[str] = None
    city_slug: Optional[str] = None
    station_id: Optional[str] = None
    event_slug: Optional[str] = None
    value_ecmwf: Optional[float] = None
    value_gfs: Optional[float] = None
    value_unit: Optional[str] = None


@dataclass
class SettlementRecord:
    """Settlement result — recorded after market resolves."""
    city: str
    target_date: str          # YYYY-MM-DD
    event_id: str
    event_slug: str
    winning_bin: str          # "38-39°F"
    settlement_temp_f: Optional[float]   # Wunderground daily high (whole °F)
    wunderground_station: str            # official Polymarket/Wunderground station
    resolved_at: Optional[datetime]
    raw_outcome_prices: str   # JSON snapshot of all outcomePrices at close
    market_type: Optional[str] = None
    measure: Optional[str] = None
    unit: Optional[str] = None
    city_slug: Optional[str] = None
    station_id: Optional[str] = None
    source_url: Optional[str] = None
    settlement_value: Optional[float] = None
    settlement_unit: Optional[str] = None
    settlement_temp_source: Optional[str] = None
    settlement_temp_station: Optional[str] = None
    settlement_temp_raw_json: Optional[str] = None
