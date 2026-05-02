import unittest
from datetime import date

from data.gamma_client import normalize_fee_rate
from data.weather_contracts import (
    discover_contracts_from_events,
    parse_daily_high_slug,
    validate_weather_event,
)
from analysis.strategies.utils import parse_temperature_range


def event(slug, title, description, station="KLGA", unit="°F", city_question="New York City"):
    source = f"https://www.wunderground.com/history/daily/us/ny/new-york-city/{station}"
    return {
        "id": "1",
        "slug": slug,
        "title": title,
        "description": description,
        "resolutionSource": source,
        "markets": [
            {
                "question": f"Will the highest temperature in {city_question} be 53{unit} or below on May 1?",
                "groupItemTitle": f"53{unit} or below",
                "description": description,
                "resolutionSource": source,
            }
        ],
    }


class WeatherContractTests(unittest.TestCase):
    def test_slug_parser_accepts_daily_high(self):
        parsed = parse_daily_high_slug("highest-temperature-in-nyc-on-may-1-2026")
        self.assertEqual(parsed, ("nyc", date(2026, 5, 1)))

    def test_slug_parser_rejects_other_weather_products(self):
        self.assertIsNone(parse_daily_high_slug("lowest-temperature-in-nyc-on-may-1-2026"))
        self.assertIsNone(parse_daily_high_slug("precipitation-in-nyc-in-may"))

    def test_validator_accepts_known_fahrenheit_wunderground_event(self):
        nyc = event(
            "highest-temperature-in-nyc-on-may-1-2026",
            "Highest temperature in NYC on May 1?",
            "This market resolves in degrees Fahrenheit using Wunderground.",
        )
        result = validate_weather_event(nyc)
        self.assertTrue(result.is_valid)
        self.assertEqual(result.contract.station_id, "KLGA")
        self.assertEqual(result.contract.unit, "F")

    def test_validator_rejects_station_mismatch(self):
        nyc = event(
            "highest-temperature-in-nyc-on-may-1-2026",
            "Highest temperature in NYC on May 1?",
            "This market resolves in degrees Fahrenheit using Wunderground.",
            station="KJFK",
        )
        result = validate_weather_event(nyc)
        self.assertFalse(result.is_valid)
        self.assertIn("station_mismatch", result.contract.skip_reason)

    def test_discovery_filters_mock_catalog(self):
        nyc = event(
            "highest-temperature-in-nyc-on-may-1-2026",
            "Highest temperature in NYC on May 1?",
            "This market resolves in degrees Fahrenheit using Wunderground.",
        )
        london = event(
            "highest-temperature-in-london-on-may-1-2026",
            "Highest temperature in London on May 1?",
            "This market resolves in degrees Celsius using Wunderground.",
            station="EGLC",
            unit="°C",
            city_question="London",
        )
        lowest = event(
            "lowest-temperature-in-nyc-on-may-1-2026",
            "Lowest temperature in NYC on May 1?",
            "This market resolves in degrees Fahrenheit using Wunderground.",
        )
        precip = {"slug": "precipitation-in-nyc-in-may", "title": "Precipitation in NYC in May?"}

        results = discover_contracts_from_events([nyc, london, lowest, precip], [date(2026, 5, 1)])
        valid = [r.contract.event_slug for r in results if r.is_valid]

        self.assertEqual(valid, ["highest-temperature-in-nyc-on-may-1-2026"])

    def test_temperature_bin_parser_handles_fahrenheit_labels(self):
        self.assertEqual(parse_temperature_range("53°F or below").kind, "below_or_equal")
        self.assertEqual(parse_temperature_range("54-55°F").kind, "between")
        self.assertEqual(parse_temperature_range("72°F or higher").kind, "above_or_equal")

    def test_fee_rate_normalization(self):
        self.assertEqual(normalize_fee_rate(1000), 0.05)
        self.assertEqual(normalize_fee_rate("0.05"), 0.05)
        self.assertIsNone(normalize_fee_rate("nope"))


if __name__ == "__main__":
    unittest.main()

