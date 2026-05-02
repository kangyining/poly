use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDate;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::config::MarketContext;
use crate::core::bus::EventBus;
use crate::core::events::{Event, EventSource, ForecastUpdated};
use crate::weather::client::{Forecast, NwsForecastProvider, StaticForecastProvider};

#[derive(Clone)]
pub struct WeatherService {
    bus: EventBus,
    context: MarketContext,
    refresh_interval_secs: u64,
}

impl WeatherService {
    pub fn new(bus: EventBus, context: MarketContext, refresh_interval_secs: u64) -> Self {
        Self {
            bus,
            context,
            refresh_interval_secs,
        }
    }

    pub async fn fetch_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        if let Some(high_f) = self.context.forecast_high_f {
            return StaticForecastProvider::new(high_f).get_forecast(
                &self.context.key,
                &self.context.city_slug,
                target_date,
                &self.context.station_code,
            );
        }

        NwsForecastProvider::new(self.context.lat, self.context.lon)
            .get_forecast(
                &self.context.key,
                &self.context.city_slug,
                target_date,
                &self.context.station_code,
            )
            .await
    }

    pub async fn run_poller(self: Arc<Self>) -> Result<()> {
        let mut previous: Option<Forecast> = None;

        loop {
            match self.fetch_forecast(self.context.target_date).await {
                Ok(current) => {
                    let changed = previous
                        .as_ref()
                        .map(|prior| {
                            prior.target_date != current.target_date
                                || (prior.forecast_high_f - current.forecast_high_f).abs() >= 0.1
                        })
                        .unwrap_or(true);

                    if changed {
                        info!(
                            context_key = %self.context.key,
                            city = %self.context.city_slug,
                            target_date = %current.target_date,
                            forecast_high_f = current.forecast_high_f,
                            "weather service published ForecastUpdated event"
                        );
                        self.bus.publish(
                            EventSource::WeatherPoller,
                            Event::ForecastUpdated(ForecastUpdated {
                                previous: previous.clone(),
                                current: current.clone(),
                            }),
                        );
                    }

                    previous = Some(current);
                }
                Err(error) => {
                    warn!(error = %error, "weather service failed to fetch forecast");
                }
            }

            sleep(Duration::from_secs(self.refresh_interval_secs)).await;
        }
    }
}
