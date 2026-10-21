# Poly Dashboard

Monorepo for the current dashboard surface around the Polymarket weather bot.

The older standalone `poly-weather-api` and `poly-weather-ui` apps have been
removed. This directory is now the single API + web dashboard surface in the repo.

## Layout

```text
poly-dashboard/
├── apps/
│   ├── api/   # FastAPI backend for live + backtest dashboard endpoints
│   └── web/   # React + Vite frontend dashboard
```

The dashboard still reads data from the sibling trading repos by default:

- `../poly-weather`
- `../poly-weather-bot`

You can override those paths with environment variables when needed:

- `POLY_WEATHER_DIR`
- `POLY_WEATHER_BOT_DIR`
- `POLY_WEATHER_DB_PATH`
- `POLY_WEATHER_STRATEGIES_PATH`

## Run

Frontend:

```bash
npm run dev:web
```

API:

```bash
npm run dev:api
```

Build the frontend:

```bash
npm run build
```
