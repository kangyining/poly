use anyhow::Result;
use dotenvy::dotenv;

use polymarket_weather_bot::config::AppConfig;
use polymarket_weather_bot::market::TradingClient;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let config = AppConfig::load(std::path::Path::new("config.toml"))?;
    let client = TradingClient::new(&config.api.clob_url);
    let wallet_address = client.wallet_address()?;
    let balance = client.get_balance_allowance().await?;

    println!("wallet_address: {}", wallet_address);
    println!("balance_usdc: {}", balance.balance);

    if balance.allowances.is_empty() {
        println!("allowances: none");
    } else {
        println!("allowances:");
        for (address, amount) in balance.allowances {
            println!("  {} => {}", address, amount);
        }
    }

    Ok(())
}
