use anyhow::Result;
use chrono::NaiveDate;

use crate::weather::{Forecast, NwsForecastProvider, StaticForecastProvider};

pub struct WeatherService {
    station_code: String,
    lat: f64,
    lon: f64,
    forecast_high_f: Option<f64>,
}

impl WeatherService {
    pub fn new(station_code: String, lat: f64, lon: f64, forecast_high_f: Option<f64>) -> Self {
        Self {
            station_code,
            lat,
            lon,
            forecast_high_f,
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
}
