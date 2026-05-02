"""
SQLite storage layer.
"""

import json
import sqlite3
import logging
from datetime import datetime, timezone
from pathlib import Path

from data.models import MarketSnapshot, METARRecord, ForecastRecord, SettlementRecord, WeatherContract

log = logging.getLogger(__name__)

SCHEMA_PATH = Path(__file__).parent.parent / "db" / "schema.sql"
DEFAULT_DB   = Path(__file__).parent.parent / "db" / "data.db"


class Database:
    def __init__(self, db_path: Path = DEFAULT_DB, read_only: bool = False):
        db_path = Path(db_path)
        if read_only:
            if not db_path.exists():
                raise FileNotFoundError(f"SQLite DB does not exist for read-only open: {db_path}")
            self.conn = sqlite3.connect(db_path.resolve().as_uri() + "?mode=ro", uri=True, check_same_thread=False)
        else:
            db_path.parent.mkdir(parents=True, exist_ok=True)
            self.conn = sqlite3.connect(str(db_path), check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        if not read_only:
            self._apply_schema()
        log.info("Database ready: %s%s", db_path, " (read-only)" if read_only else "")

    def _apply_schema(self):
        sql = SCHEMA_PATH.read_text()
        self.conn.executescript(sql)
        self._apply_migrations()
        self.conn.commit()

    def _apply_migrations(self):
        self._ensure_columns(
            "market_snapshots",
            {
                "hours_to_close": "REAL",
                "no_asks_json": "TEXT",
                "fee_rate": "REAL",
                "no_vwap_10": "REAL",
                "market_type": "TEXT",
                "measure": "TEXT",
                "unit": "TEXT",
                "city_slug": "TEXT",
                "station_id": "TEXT",
                "source_url": "TEXT",
                "validation_status": "TEXT",
                "skip_reason": "TEXT",
            },
        )
        self._ensure_columns(
            "forecast_records",
            {
                "market_type": "TEXT",
                "measure": "TEXT",
                "unit": "TEXT",
                "city_slug": "TEXT",
                "station_id": "TEXT",
                "event_slug": "TEXT",
                "value_ecmwf": "REAL",
                "value_gfs": "REAL",
                "value_unit": "TEXT",
            },
        )
        self._ensure_columns(
            "settlement_records",
            {
                "market_type": "TEXT",
                "measure": "TEXT",
                "unit": "TEXT",
                "city_slug": "TEXT",
                "station_id": "TEXT",
                "source_url": "TEXT",
                "settlement_value": "REAL",
                "settlement_unit": "TEXT",
                "settlement_temp_source": "TEXT",
                "settlement_temp_station": "TEXT",
                "settlement_temp_raw_json": "TEXT",
            },
        )
        self._ensure_columns(
            "forecast_model_runs",
            {
                "market_type": "TEXT",
                "measure": "TEXT",
                "unit": "TEXT",
                "city_slug": "TEXT",
                "station_id": "TEXT",
                "event_slug": "TEXT",
                "value": "REAL",
                "value_unit": "TEXT",
            },
        )
        self._ensure_columns(
            "positions",
            {
                "fee_rate": "REAL",
            },
        )
        self._ensure_auxiliary_tables()

    def _ensure_columns(self, table: str, columns: dict[str, str]) -> None:
        existing = {
            row["name"]
            for row in self.conn.execute(f"PRAGMA table_info({table})").fetchall()
        }
        for name, ddl in columns.items():
            if name in existing:
                continue
            self.conn.execute(f"ALTER TABLE {table} ADD COLUMN {name} {ddl}")
            log.info("Applied DB migration: added %s.%s", table, name)

    def _ensure_auxiliary_tables(self) -> None:
        """Create research/strategy-lab tables when older DB files predate schema.sql."""
        self.conn.executescript("""
            CREATE TABLE IF NOT EXISTS weather_contracts (
                id                     INTEGER PRIMARY KEY AUTOINCREMENT,
                event_slug             TEXT NOT NULL UNIQUE,
                event_id               TEXT,
                target_date            TEXT NOT NULL,
                market_type            TEXT NOT NULL,
                measure                TEXT NOT NULL,
                unit                   TEXT NOT NULL,
                city_slug              TEXT NOT NULL,
                display_city           TEXT NOT NULL,
                resolution_source_kind TEXT,
                station_id             TEXT,
                source_url             TEXT,
                timezone               TEXT,
                lat                    REAL,
                lon                    REAL,
                validation_status      TEXT NOT NULL,
                skip_reason            TEXT,
                discovered_at          TEXT NOT NULL,
                raw_json               TEXT
            );
            CREATE TABLE IF NOT EXISTS research_sources (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                url             TEXT NOT NULL UNIQUE,
                source_type     TEXT NOT NULL,
                date_checked    TEXT NOT NULL,
                claim_summary   TEXT NOT NULL,
                strategy_tags   TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS forecast_model_runs (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp           TEXT NOT NULL,
                city                TEXT NOT NULL,
                target_date         TEXT NOT NULL,
                source              TEXT NOT NULL,
                model_name          TEXT NOT NULL,
                model_run_time      TEXT,
                forecast_lead_hours REAL,
                temp_max_f          REAL,
                raw_json            TEXT
            );
            CREATE TABLE IF NOT EXISTS market_relationships (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                city            TEXT NOT NULL,
                target_date     TEXT NOT NULL,
                event_slug      TEXT,
                market_type     TEXT NOT NULL,
                bin_label       TEXT,
                lower_bound_f   REAL,
                upper_bound_f   REAL,
                threshold_f     REAL,
                source_station  TEXT,
                confidence      REAL,
                raw_json        TEXT
            );
            CREATE TABLE IF NOT EXISTS wallet_signal_snapshots (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp       TEXT NOT NULL,
                wallet          TEXT NOT NULL,
                strategy_tag    TEXT,
                market_id       TEXT,
                event_slug      TEXT,
                token_id        TEXT,
                side            TEXT,
                price           REAL,
                size            REAL,
                raw_json        TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_forecast_model_runs_city_date
                ON forecast_model_runs(city, target_date);
            CREATE INDEX IF NOT EXISTS idx_market_relationships_city_date
                ON market_relationships(city, target_date);
            CREATE INDEX IF NOT EXISTS idx_wallet_signal_snapshots_wallet_time
                ON wallet_signal_snapshots(wallet, timestamp);
            CREATE INDEX IF NOT EXISTS idx_weather_contracts_status_date
                ON weather_contracts(validation_status, target_date);
        """)

    # ── weather contracts ─────────────────────────────────────────────────

    def upsert_weather_contract(self, c: WeatherContract) -> None:
        self.conn.execute("""
            INSERT INTO weather_contracts (
                event_slug, event_id, target_date, market_type, measure, unit,
                city_slug, display_city, resolution_source_kind, station_id,
                source_url, timezone, lat, lon, validation_status, skip_reason,
                discovered_at, raw_json
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT(event_slug) DO UPDATE SET
                event_id=excluded.event_id,
                target_date=excluded.target_date,
                market_type=excluded.market_type,
                measure=excluded.measure,
                unit=excluded.unit,
                city_slug=excluded.city_slug,
                display_city=excluded.display_city,
                resolution_source_kind=excluded.resolution_source_kind,
                station_id=excluded.station_id,
                source_url=excluded.source_url,
                timezone=excluded.timezone,
                lat=excluded.lat,
                lon=excluded.lon,
                validation_status=excluded.validation_status,
                skip_reason=excluded.skip_reason,
                discovered_at=excluded.discovered_at,
                raw_json=excluded.raw_json
        """, (
            c.event_slug, c.event_id, c.target_date, c.market_type, c.measure, c.unit,
            c.city_slug, c.display_city, c.resolution_source_kind, c.station_id,
            c.source_url, c.timezone, c.lat, c.lon, c.validation_status, c.skip_reason,
            datetime.now(timezone.utc).isoformat(), c.raw_json,
        ))
        self.conn.commit()

    # ── market snapshots ──────────────────────────────────────────────────

    def insert_snapshot(self, s: MarketSnapshot):
        self.conn.execute("""
            INSERT INTO market_snapshots (
                timestamp, city, target_date, hours_to_close, event_id, event_slug,
                bin_label, bin_index, token_id_yes, token_id_no,
                yes_price, no_price,
                yes_best_bid, yes_best_ask, no_best_bid, no_best_ask,
                book_depth_bid, book_depth_ask, no_asks_json, fee_rate, no_vwap_10,
                volume_24h, liquidity,
                market_type, measure, unit, city_slug, station_id, source_url,
                validation_status, skip_reason
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            s.timestamp.isoformat(), s.city, s.target_date, s.hours_to_close,
            s.event_id, s.event_slug, s.bin_label, s.bin_index,
            s.token_id_yes, s.token_id_no,
            s.yes_price, s.no_price,
            s.yes_best_bid, s.yes_best_ask, s.no_best_bid, s.no_best_ask,
            s.book_depth_bid, s.book_depth_ask, s.no_asks_json, s.fee_rate, s.no_vwap_10,
            s.volume_24h, s.liquidity,
            s.market_type, s.measure, s.unit, s.city_slug, s.station_id, s.source_url,
            s.validation_status, s.skip_reason,
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
                model_spread_c, forecast_lead_hours, raw_json,
                market_type, measure, unit, city_slug, station_id, event_slug,
                value_ecmwf, value_gfs, value_unit
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            f.timestamp.isoformat(), f.city, f.target_date, f.source,
            f.temp_max_ecmwf_c, f.temp_max_gfs_c,
            f.temp_max_ecmwf_f, f.temp_max_gfs_f,
            f.model_spread_c, f.forecast_lead_hours, f.raw_json,
            f.market_type, f.measure, f.unit, f.city_slug, f.station_id, f.event_slug,
            f.value_ecmwf, f.value_gfs, f.value_unit,
        ))
        self.conn.commit()

    def insert_forecast_model_run(self, row: dict) -> int:
        cur = self.conn.execute("""
            INSERT INTO forecast_model_runs (
                timestamp, city, target_date, source, model_name, model_run_time,
                forecast_lead_hours, temp_max_f, raw_json,
                market_type, measure, unit, city_slug, station_id, event_slug,
                value, value_unit
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            row["timestamp"], row["city"], row["target_date"], row["source"],
            row["model_name"], row.get("model_run_time"),
            row.get("forecast_lead_hours"), row.get("temp_max_f"),
            row.get("raw_json"),
            row.get("market_type"), row.get("measure"), row.get("unit"),
            row.get("city_slug"), row.get("station_id"), row.get("event_slug"),
            row.get("value"), row.get("value_unit"),
        ))
        self.conn.commit()
        return cur.lastrowid

    def seed_research_sources(self, sources: list[dict]) -> int:
        saved = 0
        for source in sources:
            cur = self.conn.execute("""
                INSERT OR IGNORE INTO research_sources (
                    url, source_type, date_checked, claim_summary, strategy_tags
                ) VALUES (?,?,?,?,?)
            """, (
                source["url"],
                source["source_type"],
                source["date_checked"],
                source["claim_summary"],
                json.dumps(source.get("strategy_tags", [])),
            ))
            saved += cur.rowcount
        self.conn.commit()
        return saved

    # ── settlement records ────────────────────────────────────────────────

    def insert_settlement(self, s: SettlementRecord):
        self.conn.execute("""
            INSERT OR IGNORE INTO settlement_records (
                city, target_date, event_id, event_slug,
                winning_bin, settlement_temp_f,
                wunderground_station, resolved_at, raw_outcome_prices,
                market_type, measure, unit, city_slug, station_id, source_url,
                settlement_value, settlement_unit,
                settlement_temp_source, settlement_temp_station, settlement_temp_raw_json
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            s.city, s.target_date, s.event_id, s.event_slug,
            s.winning_bin, s.settlement_temp_f,
            s.wunderground_station,
            s.resolved_at.isoformat() if s.resolved_at else None,
            s.raw_outcome_prices,
            s.market_type, s.measure, s.unit, s.city_slug, s.station_id, s.source_url,
            s.settlement_value, s.settlement_unit,
            s.settlement_temp_source, s.settlement_temp_station, s.settlement_temp_raw_json,
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

    # ── price history ────────────────────────────────────────────────────

    def insert_price_history(self, city: str, target_date: str, bin_label: str,
                              bin_index: int, token_id_yes: str, timestamp: str,
                              yes_price: float, hours_to_close: float,
                              source: str = 'clob-backfill'):
        self.conn.execute("""
            INSERT OR IGNORE INTO price_history (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, (city, target_date, bin_label, bin_index, token_id_yes,
              timestamp, yes_price, hours_to_close, source))
        self.conn.commit()

    def price_history_exists(self, city: str, target_date: str) -> bool:
        row = self.conn.execute(
            "SELECT 1 FROM price_history WHERE city=? AND target_date=? LIMIT 1",
            (city, target_date)
        ).fetchone()
        return row is not None

    def insert_price_history_rows(self, rows: list[tuple]) -> None:
        if not rows:
            return
        self.conn.executemany("""
            INSERT OR IGNORE INTO price_history (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, rows)
        self.conn.commit()

    def insert_price_history_minute(self, city: str, target_date: str, bin_label: str,
                                    bin_index: int, token_id_yes: str, timestamp: str,
                                    yes_price: float, hours_to_close: float,
                                    source: str = 'clob-backfill'):
        self.conn.execute("""
            INSERT OR IGNORE INTO price_history_1m (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, (city, target_date, bin_label, bin_index, token_id_yes,
              timestamp, yes_price, hours_to_close, source))
        self.conn.commit()

    def insert_price_history_minute_rows(self, rows: list[tuple]) -> None:
        if not rows:
            return
        self.conn.executemany("""
            INSERT OR IGNORE INTO price_history_1m (
                city, target_date, bin_label, bin_index, token_id_yes,
                timestamp, yes_price, hours_to_close, source
            ) VALUES (?,?,?,?,?,?,?,?,?)
        """, rows)
        self.conn.commit()

    def price_history_minute_exists(self, city: str, target_date: str) -> bool:
        row = self.conn.execute(
            "SELECT 1 FROM price_history_1m WHERE city=? AND target_date=? LIMIT 1",
            (city, target_date)
        ).fetchone()
        return row is not None

    # ── TAF records ───────────────────────────────────────────────────────

    def insert_taf(self, icao: str, city: str, issue_time,
                   valid_from, valid_to, raw_taf: str):
        from datetime import datetime, timezone
        now = datetime.now(timezone.utc).isoformat()
        self.conn.execute("""
            INSERT INTO taf_records (
                timestamp, icao, city, issue_time, valid_from, valid_to, raw_taf
            ) VALUES (?,?,?,?,?,?,?)
        """, (
            now, icao, city,
            issue_time.isoformat() if hasattr(issue_time, 'isoformat') else issue_time,
            valid_from.isoformat() if hasattr(valid_from, 'isoformat') else valid_from,
            valid_to.isoformat() if hasattr(valid_to, 'isoformat') else valid_to,
            raw_taf,
        ))
        self.conn.commit()

    # ── NWS forecast records ──────────────────────────────────────────────

    def insert_nws_forecast(self, city: str, target_date: str, period_name: str,
                             is_daytime: bool, temp_f: float,
                             short_forecast: str, lead_hours: float):
        from datetime import datetime, timezone
        now = datetime.now(timezone.utc).isoformat()
        self.conn.execute("""
            INSERT INTO nws_forecasts (
                timestamp, city, target_date, period_name,
                is_daytime, temp_f, short_forecast, forecast_lead_hours
            ) VALUES (?,?,?,?,?,?,?,?)
        """, (
            now, city, target_date, period_name,
            1 if is_daytime else 0, temp_f, short_forecast, lead_hours,
        ))
        self.conn.commit()

    # ── positions ─────────────────────────────────────────────────────────

    def insert_position(self, pos: dict) -> int:
        """
        Insert a position from open_positions.json dict (written by execute.py).
        Returns the new row id. Ignores duplicates (same token_id + order_id).
        """
        cur = self.conn.execute("""
            INSERT OR IGNORE INTO positions (
                token_id, market_id, bin_label, event_slug, target_date,
                entry_price, fee_rate, shares, stake_usd, order_id,
                status, pnl, opened_at, settled_at, is_paper,
                strategy_name, backtest_id, trade_rationale
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """, (
            pos["token_id"], pos.get("market_id", ""),
            pos["bin_label"], pos["event_slug"], pos["target_date"],
            pos["entry_price"], pos.get("fee_rate"),
            pos["shares"], pos["stake_usd"],
            pos.get("order_id"),
            pos.get("status", "open"), pos.get("pnl"),
            pos["opened_at"], pos.get("settled_at"),
            1 if pos.get("is_paper", True) else 0,
            pos.get("strategy_name", "default"),
            pos.get("backtest_id"),
            pos.get("trade_rationale"),
        ))
        self.conn.commit()
        return cur.lastrowid

    def get_open_positions(self) -> list[dict]:
        """Return all open positions as plain dicts."""
        rows = self.conn.execute(
            "SELECT * FROM positions WHERE status='open' ORDER BY opened_at"
        ).fetchall()
        return [dict(r) for r in rows]

    def close_position(self, position_id: int, pnl: float, settled_at: str) -> None:
        """Mark a position closed with its PnL."""
        self.conn.execute(
            "UPDATE positions SET status='closed', pnl=?, settled_at=? WHERE id=?",
            (pnl, settled_at, position_id),
        )
        self.conn.commit()

    def export_open_positions_json(self, path) -> None:
        """
        Write open_positions.json consumed by the Rust bot to skip already-held bins.
        Only exports token_id and status so the file stays small.
        """
        import json
        rows = self.get_open_positions()
        out = [
            {
                "token_id":   r["token_id"],
                "market_id":  r["market_id"],
                "bin_label":  r["bin_label"],
                "event_slug": r["event_slug"],
                "target_date": r["target_date"],
                "entry_price": r["entry_price"],
                "shares":     r["shares"],
                "stake_usd":  r["stake_usd"],
                "order_id":   r["order_id"],
                "status":     r["status"],
                "pnl":        r["pnl"],
                "opened_at":  r["opened_at"],
                "settled_at": r["settled_at"],
                "is_paper":   bool(r["is_paper"]),
            }
            for r in rows
        ]
        from pathlib import Path
        Path(path).write_text(json.dumps(out, indent=2))
        log.info("exported %d open positions -> %s", len(out), path)

    def close(self):
        self.conn.close()
