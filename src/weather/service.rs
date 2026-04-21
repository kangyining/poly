use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDate;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::core::bus::EventBus;
use crate::core::events::{Event, EventSource, ForecastUpdated, WeatherEvent};
use crate::weather::client::{Forecast, NwsForecastProvider, StaticForecastProvider};

#[derive(Clone)]
pub struct WeatherService {
    bus: EventBus,
    station_code: String,
    lat: f64,
    lon: f64,
    forecast_high_f: Option<f64>,
    target_date: NaiveDate,
    poll_interval_secs: u64,
}

impl WeatherService {
    pub fn new(
        bus: EventBus,
        station_code: String,
        lat: f64,
        lon: f64,
        forecast_high_f: Option<f64>,
        target_date: NaiveDate,
        poll_interval_secs: u64,
    ) -> Self {
        Self {
            bus,
            station_code,
            lat,
            lon,
            forecast_high_f,
            target_date,
            poll_interval_secs,
        }
    }

    pub async fn fetch_forecast(&self, target_date: NaiveDate) -> Result<Forecast> {
        if let Some(high_f) = self.forecast_high_f {
            return StaticForecastProvider::new(high_f)
                .get_forecast(target_date, &self.station_code);
        }

        NwsForecastProvider::new(self.lat, self.lon)
            .get_forecast(target_date, &self.station_code)
            .await
    }

    pub async fn run_poller(self: Arc<Self>) -> Result<()> {
        let mut previous: Option<Forecast> = None;

        loop {
            match self.fetch_forecast(self.target_date).await {
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
                            "weather service published ForecastUpdated event"
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
                    warn!(error = %error, "weather service failed to fetch forecast");
                }
            }

            sleep(Duration::from_secs(self.poll_interval_secs)).await;
        }
    }
}
