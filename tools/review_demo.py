"""Create synthetic, offline weather inputs in a new directory."""
import argparse
import json
from datetime import datetime, timedelta, timezone, date
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "poly-weather"))
from data.db import Database


def seed_fixture(path: Path, target_date: str = "2026-04-02"):
    day = date.fromisoformat(target_date)
    previous = (day - timedelta(days=1)).isoformat()
    slug = f"highest-temperature-in-nyc-on-{day.strftime('%B').lower()}-{day.day}-{day.year}"
    db = Database(path)
    try:
        db.conn.execute("""INSERT INTO settlement_records
            (city,target_date,event_id,event_slug,winning_bin,settlement_temp_f,resolved_at)
            VALUES (?,?,?,?,?,?,?)""", ("NYC", target_date, "synthetic-event",
                slug, "80-81°F", 80.4, f"{target_date}T12:00:00Z"))
        db.conn.execute("""INSERT INTO forecast_records
            (timestamp,city,target_date,source,temp_max_ecmwf_f,temp_max_gfs_f,model_spread_c,forecast_lead_hours,raw_json)
            VALUES (?,?,?,?,?,?,?,?,?)""", (f"{previous}T06:00:00Z", "NYC", target_date, "gfs-global-24h",
                80.0, 80.0, 0.0, 30.0, json.dumps({"past_model_run": f"{previous}T06:00:00Z"})))
        db.conn.execute("""INSERT INTO price_history_1m
            (city,target_date,bin_label,bin_index,token_id_yes,timestamp,yes_price,hours_to_close,source)
            VALUES (?,?,?,?,?,?,?,?,?)""", ("NYC", target_date, "70-71°F", 0, "synthetic-token",
                f"{previous}T12:00:00Z", 0.95, 24.0, "synthetic-fixture"))
        db.conn.commit()
    finally:
        db.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New directory; existing data is never overwritten")
    output = parser.parse_args().output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    target_date = (datetime.now(timezone.utc).date() - timedelta(days=1)).isoformat()
    seed_fixture(output / "data.db", target_date)
    print(json.dumps({"data": "synthetic", "database": str(output / "data.db"),
                      "date_from": target_date, "date_to": target_date,
                      "setup": "Set POLY_WEATHER_DB_PATH to this database when starting the dashboard API."}, indent=2))


if __name__ == "__main__":
    main()
