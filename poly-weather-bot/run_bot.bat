@echo off
REM Polymarket Weather Bot launcher
REM
REM Usage:
REM   run_bot.bat              -- single pass, paper mode (default)
REM   run_bot.bat --run-loop   -- recurring loop, paper mode (default 15-second repricing / 30-minute weather refresh)
REM   run_bot.bat --run-loop --dry-run=false   -- live trading loop
REM
REM All flags are passed through to the binary. See --help for full list.
REM
REM Environment variables (for live trading):
REM   set POLYMARKET_PRIVATE_KEY=0x...

setlocal

set BOT=%~dp0target\release\polymarket-weather-bot.exe

if not exist "%BOT%" (
    echo [ERROR] Bot binary not found. Run: cargo build --release
    exit /b 1
)

echo [INFO] Starting polymarket-weather-bot...
echo [INFO] Args: %*
echo.

"%BOT%" %*
