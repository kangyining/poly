"""
SQLite storage layer.
"""

import json
import sqlite3
import logging
from datetime import datetime
from pathlib import Path

from data.models import MarketSnapshot, METARRecord, ForecastRecord, SettlementRecord

log = logging.getLogger(__name__)

SCHEMA_PATH = Path(__file__).parent.parent / "db" / "schema.sql"
DEFAULT_DB   = Path(__file__).parent.parent / "db" / "data.db"


class Database:
    def __init__(self, db_path: Path = DEFAULT_DB):
        db_path.parent.mkdir(parents=True, exist_ok=True)
        self.conn = sqlite3.connect(str(db_path), check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self._apply_schema()
        log.info("Database ready: %s", db_path)

    def _apply_schema(self):
        sql = SCHEMA_PATH.read_text()
        self.conn.executescript(sql)
        self.conn.commit()

    # ── market snapshots ──────────────────────────────────────────────────

    def insert_snapshot(self, s: MarketSnapshot):
        self.conn.execute("""
            INSERT INTO market_snapshots (
                timestamp, city, target_date, event_id, event_slug,
                bin_label, bin_index, token_id_yes, token_id_no,
                yes_price, no_price,
                yes_best_bid, yes_best_ask, no_best_bid, no_best_ask,
                book_depth_bid, book_depth_ask, volume_24h, liquidity
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            s.timestamp.isoformat(), s.city, s.target_date,
            s.event_id, s.event_slug, s.bin_label, s.bin_index,
            s.token_id_yes, s.token_id_no,
            s.yes_price, s.no_price,
            s.yes_best_bid, s.yes_best_ask, s.no_best_bid, s.no_best_ask,
            s.book_depth_bid, s.book_depth_ask, s.volume_24h, s.liquidity,
        ))
        self.conn.commit()

    def snapshot_count(self) -> int:
        return self.conn.execute(
            "SELECT COUNT(*) FROM market_snapshots"
        ).fetchone()[0]

    # ── METAR records ─────────────────────────────────────────────────────

    def insert_metar(self, m: METARRecord):
        self.conn.execute("""
            INSERT INTO metar_records (
                timestamp, icao, city, temp_c, temp_f,
                dewpoint_c, obs_time, raw_metar
            ) VALUES (?,?,?,?,?,?,?,?)
        """, (
            m.timestamp.isoformat(), m.icao, m.city,
            m.temp_c, m.temp_f, m.dewpoint_c,
            m.obs_time.isoformat() if m.obs_time else None,
            m.raw_metar,
        ))
        self.conn.commit()

    # ── forecast records ──────────────────────────────────────────────────

    def insert_forecast(self, f: ForecastRecord):
        self.conn.execute("""
            INSERT INTO forecast_records (
                timestamp, city, target_date, source,
                temp_max_ecmwf_c, temp_max_gfs_c,
                temp_max_ecmwf_f, temp_max_gfs_f,
                model_spread_c, forecast_lead_hours, raw_json
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?)
        """, (
            f.timestamp.isoformat(), f.city, f.target_date, f.source,
            f.temp_max_ecmwf_c, f.temp_max_gfs_c,
            f.temp_max_ecmwf_f, f.temp_max_gfs_f,
            f.model_spread_c, f.forecast_lead_hours, f.raw_json,
        ))
        self.conn.commit()

    # ── settlement records ────────────────────────────────────────────────

    def insert_settlement(self, s: SettlementRecord):
        self.conn.execute("""
            INSERT OR IGNORE INTO settlement_records (
                city, target_date, event_id, event_slug,
                winning_bin, settlement_temp_f,
                wunderground_station, resolved_at, raw_outcome_prices
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, (
            s.city, s.target_date, s.event_id, s.event_slug,
            s.winning_bin, s.settlement_temp_f,
            s.wunderground_station,
            s.resolved_at.isoformat() if s.resolved_at else None,
            s.raw_outcome_prices,
        ))
        self.conn.commit()

    def settlement_count(self) -> int:
        return self.conn.execute(
            "SELECT COUNT(*) FROM settlement_records"
        ).fetchone()[0]

    def settlement_exists(self, city: str, target_date: str) -> bool:
        row = self.conn.execute(
            "SELECT 1 FROM settlement_records WHERE city=? AND target_date=?",
            (city, target_date)
        ).fetchone()
        return row is not None

    def close(self):
        self.conn.close()
