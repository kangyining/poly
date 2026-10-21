# Parameter Sweep Report: nyc_10usdc_micro_v1

## Scope

- City: `NYC`
- Date range: `2026-02-05` to `2026-04-25`
- Available dates used: `78`
- Replay mode: `single`
- Parameter combinations: `12`
- Rank metric: `roi`
- Minimum trades for ranking: `10`
- CSV: `/mnt/f/Polymark/poly-weather/backtest_param_finetune/results/nyc_10usdc_micro_v1.csv`

## Grid

- `min_ev_threshold`: `0.06,0.08,0.1`
- `desired_shares`: `5.0`
- `funding_pct`: `1.0`
- `max_stake_usd`: `5.0,8.0,10.0,12.0`
- `fee_rate`: `0.05`

## Top Results

| Rank | Backtest ID | ROI | PnL | Stake | Trades | Win% | Sharpe | min_ev | shares | funding | max_stake |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | `f55deef5-b9e9-4241-876d-0afe9eb778e4` | 20.52% | 10.2171 | 49.7829 | 18 | 66.67% | 3.8535 | 0.080 | 5.00 | 1.00 | 5.00 |
| 2 | `6bc278c8-0cb5-4b7f-9799-1b6c46ac783e` | 20.52% | 10.2171 | 49.7829 | 18 | 66.67% | 3.8535 | 0.100 | 5.00 | 1.00 | 5.00 |
| 3 | `91e85c34-c3bf-421d-8d4c-184b5903e9d3` | 10.69% | 5.3116 | 49.6884 | 16 | 68.75% | 2.3382 | 0.060 | 5.00 | 1.00 | 5.00 |
| 4 | `cf1ea883-e37b-4a3f-ac5b-45742fb73da1` | 0.41% | 0.4896 | 119.5104 | 39 | 61.54% | 0.1415 | 0.100 | 5.00 | 1.00 | 12.00 |
| 5 | `31e18e93-1155-4faf-a7ae-abdf817271e1` | -4.09% | -4.9051 | 119.9051 | 39 | 58.97% | -1.6925 | 0.080 | 5.00 | 1.00 | 12.00 |
| 6 | `feb2edce-2699-42ca-b4e3-8c755ae4dab3` | -4.77% | -4.7541 | 99.7541 | 33 | 57.58% | -1.7353 | 0.100 | 5.00 | 1.00 | 10.00 |
| 7 | `51277f9f-d234-42cc-9708-ce3cc96ad099` | -6.04% | -4.8227 | 79.8227 | 26 | 57.69% | -2.2227 | 0.100 | 5.00 | 1.00 | 8.00 |
| 8 | `aef3bf2e-5721-4cd2-9c4a-380f7e4bf89c` | -8.33% | -9.9970 | 119.9970 | 38 | 57.89% | -3.9327 | 0.060 | 5.00 | 1.00 | 12.00 |
| 9 | `5a661d6c-6882-417d-b081-5485049b80da` | -9.66% | -9.6283 | 99.6283 | 33 | 54.55% | -3.7678 | 0.080 | 5.00 | 1.00 | 10.00 |
| 10 | `13ec0ffb-c57a-44bb-ade8-04a18a5680de` | -9.91% | -9.8986 | 99.8986 | 32 | 56.25% | -4.4422 | 0.060 | 5.00 | 1.00 | 10.00 |
| 11 | `7375bf2c-39e0-469a-892d-c1b4bbd35336` | -11.99% | -9.5379 | 79.5379 | 27 | 51.85% | -4.9199 | 0.060 | 5.00 | 1.00 | 8.00 |
| 12 | `6b138cc1-4449-40e5-ab53-f2f341c3e438` | -11.99% | -9.5379 | 79.5379 | 27 | 51.85% | -4.9199 | 0.080 | 5.00 | 1.00 | 8.00 |

## Notes

- Rows below the minimum trade count receive a blank `rank_score` and sort below eligible rows.
- This report ranks the tested period only. Keep a separate validation period before promoting parameters.
- Each row also exists in SQLite as a normal backtest run, so the dashboard can inspect the generated `backtest_id`.
