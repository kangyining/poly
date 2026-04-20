use std::sync::Arc;

use anyhow::Result;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::bus::EventBus;
use crate::events::{Event, EventSource, ForecastUpdated, WeatherEvent};
use crate::services::WeatherService;
use crate::weather::Forecast;

pub struct WeatherSource {
    bus: EventBus,
    weather_service: Arc<WeatherService>,
    target_date: chrono::NaiveDate,
    poll_interval_secs: u64,
}

impl WeatherSource {
    pub fn new(
        bus: EventBus,
        weather_service: Arc<WeatherService>,
        target_date: chrono::NaiveDate,
        poll_interval_secs: u64,
    ) -> Self {
        Self {
            bus,
            weather_service,
            target_date,
            poll_interval_secs,
        }
    }
}

impl WeatherSource {
    pub async fn run(self) -> Result<()> {
        let mut previous: Option<Forecast> = None;

        loop {
            match self.weather_service.fetch_forecast(self.target_date).await {
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
                            target_date = %current.target_date,
                            forecast_high_f = current.forecast_high_f,
                            "weather source produced ForecastUpdated event"
                        );
                        self.bus.publish(
                            EventSource::WeatherPoller,
                            Event::Weather(WeatherEvent::ForecastUpdated(ForecastUpdated {
                                previous: previous.clone(),
                                current: current.clone(),
                            })),
                        );
                    }

                    previous = Some(current);
                }
                Err(error) => {
                    warn!(error = %error, "weather source failed to fetch forecast");
                }
            }

            sleep(Duration::from_secs(self.poll_interval_secs)).await;
        }
    }
}
