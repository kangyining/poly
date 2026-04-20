"""
Phase 0 data recorder — run this continuously to collect data.

Schedule:
  Every 5m  → market snapshots (all cities, today + tomorrow)
  Every 1h  → METAR observations (KORD, KDAL, KLGA)
  Every 30m → NWS forecasts (next 7 days)
  Every 1h  → Open-Meteo forecasts (ECMWF + GFS)
  Every 24h → settlement check (yesterday's results)

Usage:
  python run_recorder.py                      # run scheduler loop
  python run_recorder.py --once               # run all jobs once and exit
  python run_recorder.py --backfill           # pull historical settled markets
  python run_recorder.py --backfill-forecasts # historical ECMWF/GFS forecasts
  python run_recorder.py --backfill-prices    # historical CLOB price history
  python run_recorder.py --backfill-settlement-temps  # fill missing temperatures from ASOS
"""

import argparse
import logging
import sys
import time
from datetime import datetime, timezone

import schedule

from data.db import Database
from data.market_recorder import record_market_snapshots
from data.metar_recorder import record_metars
from data.forecast_recorder import record_forecasts
from data.settlement_tracker import daily_check, backfill
from data.historical_forecast_backfill import backfill_historical_forecasts
from data.taf_nws_recorder import record_tafs, record_nws_forecasts
from data.price_history_backfill import backfill_price_history
from data.asos_daily_high_backfill import backfill_missing_temps
from data.price_history_missing_bins_backfill import backfill_missing_bins
from data.true_forecast_backfill import backfill_true_forecasts

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s — %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
    handlers=[
        logging.StreamHandler(sys.stdout),
        logging.FileHandler("recorder.log"),
    ],
)
log = logging.getLogger(__name__)


def run_all(db: Database):
    log.info("=== Running all jobs ===")
    record_metars(db)
    record_tafs(db)
    record_market_snapshots(db)
    record_forecasts(db)
    record_nws_forecasts(db)
    daily_check(db)
    log.info("Snapshots in DB: %d | Settlements in DB: %d",
             db.snapshot_count(), db.settlement_count())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--once",     action="store_true",
                        help="Run all jobs once and exit")
    parser.add_argument("--backfill", action="store_true",
                        help="Pull historical settled markets and exit")
    parser.add_argument("--backfill-forecasts", action="store_true",
                        help="Backfill historical ECMWF/GFS forecasts for all settled dates")
    parser.add_argument("--backfill-prices", action="store_true",
                        help="Backfill hourly CLOB price history for all settled markets")
    parser.add_argument("--backfill-settlement-temps", action="store_true",
                        help="Backfill missing settlement temperatures from ASOS for all cities")
    parser.add_argument("--backfill-missing-bins", action="store_true",
                        help="Retry CLOB backfill for missing price history bins (warm-side bins 7-10)")
    parser.add_argument("--backfill-true-forecasts", action="store_true",
                        help="Backfill true 24h GFS forecasts using gfs_global past_model_run")
    parser.add_argument("--city", type=str, default=None,
                        help="City to target for backfill (default: all cities)")
    parser.add_argument("--days-back", type=int, default=60,
                        help="Days to look back for backfill (default 60)")
    args = parser.parse_args()

    db = Database()

    if args.backfill:
        log.info("Starting backfill — looking back %d days", args.days_back)
        count = backfill(db, days_back=args.days_back)
        log.info("Backfill done. Total settlements in DB: %d", count)
        db.close()
        return

    if args.backfill_prices:
        log.info("Starting price history backfill")
        count = backfill_price_history(db)
        total = db.conn.execute("SELECT COUNT(*) FROM price_history").fetchone()[0]
        log.info("Done. Total price_history rows in DB: %d", total)
        db.close()
        return

    if args.backfill_forecasts:
        log.info("Starting historical forecast backfill")
        count = backfill_historical_forecasts(db)
        log.info("Done. Total forecast records in DB: %d", db.conn.execute(
            "SELECT COUNT(*) FROM forecast_records").fetchone()[0])
        db.close()
        return

    if args.backfill_settlement_temps:
        log.info("Starting settlement temperature backfill from ASOS")
        for city in ["Chicago", "Dallas", "NYC"]:
            count = backfill_missing_temps(db, city=city)
        total = db.conn.execute(
            "SELECT COUNT(*) FROM settlement_records WHERE settlement_temp_f IS NOT NULL"
        ).fetchone()[0]
        log.info("Done. Settlement records with temps: %d", total)
        db.close()
        return

    if args.backfill_missing_bins:
        log.info("Starting missing bins price history backfill (warm-side retry)")
        for city in ["Chicago", "Dallas", "NYC"]:
            count = backfill_missing_bins(db, city=city)
        total = db.conn.execute("SELECT COUNT(*) FROM price_history").fetchone()[0]
        log.info("Done. Total price_history rows: %d", total)
        db.close()
        return

    if args.backfill_true_forecasts:
        log.info("Starting true 24h GFS forecast backfill (gfs_global)")
        count = backfill_true_forecasts(db, city=args.city)
        total = db.conn.execute(
            "SELECT COUNT(*) FROM forecast_records WHERE source = 'gfs-global-24h'"
        ).fetchone()[0]
        log.info("Done. Total gfs-global-24h records in DB: %d", total)
        db.close()
        return

    if args.once:
        run_all(db)
        db.close()
        return

    # ── Continuous scheduler ──────────────────────────────────────────────
    log.info("Starting continuous recorder (Ctrl+C to stop)")

    # Run immediately on start
    run_all(db)

    # Then schedule
    schedule.every(1).hours.do(record_metars, db)
    schedule.every(1).hours.do(record_tafs, db)
    schedule.every(5).minutes.do(record_market_snapshots, db)
    schedule.every(1).hours.do(record_forecasts, db)
    schedule.every(30).minutes.do(record_nws_forecasts, db)
    schedule.every().day.at("06:00").do(daily_check, db)

    try:
        while True:
            schedule.run_pending()
            time.sleep(30)
    except KeyboardInterrupt:
        log.info("Recorder stopped by user")
    finally:
        db.close()


if __name__ == "__main__":
    main()
