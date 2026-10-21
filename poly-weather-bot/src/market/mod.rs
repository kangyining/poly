mod client;
mod service;
mod trading_client;

pub use client::{PolymarketClient, TemperatureRange, WeatherMarket};
pub use service::MarketService;
pub use trading_client::TradingClient;
