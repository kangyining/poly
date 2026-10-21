# Fixture Notes

Integration tests synthesize their SQLite fixture at runtime from `db/schema.sql`.
This keeps the schema in one place while still exercising a Python-compatible
database shape with settlements, forecasts, price history, and paper positions.
