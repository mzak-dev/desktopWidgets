//! Current weather from Open-Meteo (no API key) as the `weather` Code Source.

use wayfinder_plugin::*;

#[derive(Default)]
pub struct Weather;

/// WMO weather interpretation codes, as Open-Meteo reports them.
pub fn sky(code: i64) -> &'static str {
    match code {
        0 => "Clear",
        1 | 2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 | 80..=82 => "Rain",
        71..=77 | 85 | 86 => "Snow",
        95..=99 => "Thunderstorm",
        _ => "Unknown",
    }
}

impl Source for Weather {
    fn sample(&mut self, cx: &Cx) -> Sample {
        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code,wind_speed_10m&temperature_unit={}",
            cx.number("latitude"),
            cx.number("longitude"),
            if cx.flag("fahrenheit") { "fahrenheit" } else { "celsius" },
        );
        let now = match http::get(&url).and_then(|r| r.json()) {
            Ok(v) => v["current"].clone(),
            Err(e) => return Sample::error(e).every(minutes(2)),
        };
        let temp = now["temperature_2m"].as_f64().unwrap_or(f64::NAN);
        Sample::new(json!({
            "temp": temp.round(),
            "sky": sky(now["weather_code"].as_i64().unwrap_or(-1)),
            "wind": now["wind_speed_10m"].as_f64().unwrap_or(0.0).round(),
            "updated": format!("{:02}:{:02}", cx.local.hour, cx.local.minute),
        }))
        .every(minutes(15))
    }

    fn act(&mut self, verb: &str, _arg: &str, _cx: &Cx) -> Act {
        match verb {
            "refresh" => Act::Resample,
            _ => Act::Nothing,
        }
    }
}

export_source!(Weather);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_open_meteo_and_names_the_sky() {
        testing::answer_http(|req| {
            assert!(req["url"].as_str().unwrap().starts_with("https://api.open-meteo.com/v1/forecast?latitude=59.91&longitude=10.75"));
            http::Response { status: 200, body: r#"{"current":{"temperature_2m":12.6,"weather_code":3,"wind_speed_10m":9.4}}"#.into(), ..Default::default() }
        });
        let s = Weather.sample(&testing::cx("weather-1", json!({ "latitude": 59.91, "longitude": 10.75 })));
        assert_eq!(s.value().unwrap()["temp"], 13.0);
        assert_eq!(s.value().unwrap()["sky"], "Cloudy");
        assert_eq!(s.refresh(), Some(minutes(15)));
    }

    #[test]
    fn a_failed_request_is_an_error_that_retries_soon() {
        testing::answer_http(|_| http::Response { status: 500, ..Default::default() });
        let s = Weather.sample(&testing::cx("weather-1", json!({})));
        assert_eq!((s.error_message(), s.refresh()), (Some("the server answered 500"), Some(minutes(2))));
    }
}
