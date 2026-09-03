use crate::{
    database::WeatherCacheRow,
    settings::{AppState, ProductSettings, TemperatureUnit},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::Manager;
use url::Url;

const GEOCODING_ENDPOINT: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_ENDPOINT: &str = "https://api.open-meteo.com/v1/forecast";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_AFTER_SECONDS: i64 = 60 * 60;
const STALE_AFTER_SECONDS: i64 = 2 * 60 * 60;
const VERY_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;
const MANUAL_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WeatherCondition {
    Clear,
    MainlyClear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Showers,
    Thunderstorm,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherSnapshot {
    pub condition: WeatherCondition,
    pub temperature: f64,
    pub daily_high: f64,
    pub daily_low: f64,
    pub precipitation_probability: u8,
    pub fetched_at: i64,
    pub location_key: String,
    pub timezone: String,
    pub temperature_unit: TemperatureUnit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationCandidate {
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
    pub country: String,
    pub admin1: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheStatus {
    Fresh,
    Stale,
    VeryStale,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherViewState {
    pub configured: bool,
    pub snapshot: Option<WeatherSnapshot>,
    pub cache_status: CacheStatus,
    pub cache_age_seconds: Option<i64>,
    pub refresh_recommended: bool,
    pub refreshing: bool,
    pub last_refresh: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherDiagnosticSnapshot {
    pub configured: bool,
    pub cache_status: &'static str,
    pub cache_age_seconds: Option<i64>,
    pub last_refresh: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "PascalCase")]
pub enum WeatherError {
    NoLocation,
    Network,
    Timeout,
    InvalidResponse,
    ProviderError,
    InProgress,
    Cooldown,
}

impl WeatherError {
    fn diagnostic_label(&self) -> &'static str {
        match self {
            Self::NoLocation => "no-location",
            Self::Network => "network-error",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid-response",
            Self::ProviderError => "provider-error",
            Self::InProgress => "in-progress",
            Self::Cooldown => "cooldown",
        }
    }
}

#[derive(Default)]
pub struct WeatherRuntime {
    refreshing: AtomicBool,
    last_manual_refresh: Mutex<Option<Instant>>,
    last_refresh: Mutex<String>,
}

impl WeatherRuntime {
    fn last_refresh(&self) -> String {
        self.last_refresh
            .lock()
            .map(|value| {
                if value.is_empty() {
                    "none".into()
                } else {
                    value.clone()
                }
            })
            .unwrap_or_else(|_| "unavailable".into())
    }

    fn record(&self, value: &str) {
        if let Ok(mut status) = self.last_refresh.lock() {
            *status = value.into();
        }
    }

    fn begin_refresh(&self, manual: bool) -> Result<(), WeatherError> {
        if manual {
            let mut last = self
                .last_manual_refresh
                .lock()
                .map_err(|_| WeatherError::ProviderError)?;
            if last.is_some_and(|instant| instant.elapsed() < MANUAL_REFRESH_COOLDOWN) {
                return Err(WeatherError::Cooldown);
            }
            *last = Some(Instant::now());
        }
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return Err(WeatherError::InProgress);
        }
        Ok(())
    }

    fn finish_refresh(&self) {
        self.refreshing.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
struct WeatherLocation {
    latitude: f64,
    longitude: f64,
    timezone: String,
    key: String,
}

impl WeatherLocation {
    fn from_settings(settings: &ProductSettings) -> Result<Option<Self>, WeatherError> {
        match (settings.weather_latitude, settings.weather_longitude) {
            (None, None) if settings.weather_timezone.is_empty() => Ok(None),
            (Some(latitude), Some(longitude))
                if latitude.is_finite()
                    && (-90.0..=90.0).contains(&latitude)
                    && longitude.is_finite()
                    && (-180.0..=180.0).contains(&longitude)
                    && !settings.weather_timezone.trim().is_empty() =>
            {
                let timezone = settings.weather_timezone.trim().to_string();
                Ok(Some(Self {
                    latitude,
                    longitude,
                    key: location_key(latitude, longitude, &timezone),
                    timezone,
                }))
            }
            _ => Err(WeatherError::InvalidResponse),
        }
    }
}

struct HttpResponse {
    status: u16,
    body: String,
}

trait HttpTransport {
    fn get(&self, url: Url) -> Result<HttpResponse, WeatherError>;
}

struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    fn new() -> Result<Self, WeatherError> {
        reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Alan-Desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|_| WeatherError::Network)
    }
}

impl HttpTransport for ReqwestTransport {
    fn get(&self, url: Url) -> Result<HttpResponse, WeatherError> {
        let response = self.client.get(url).send().map_err(|error| {
            if error.is_timeout() {
                WeatherError::Timeout
            } else {
                WeatherError::Network
            }
        })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|_| WeatherError::Network)?;
        Ok(HttpResponse { status, body })
    }
}

#[derive(Deserialize)]
struct GeocodingResponse {
    #[serde(default)]
    results: Vec<RawLocation>,
}

#[derive(Deserialize)]
struct RawLocation {
    name: String,
    latitude: f64,
    longitude: f64,
    timezone: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    admin1: String,
}

#[derive(Deserialize)]
struct ForecastResponse {
    timezone: String,
    current: RawCurrent,
    daily: RawDaily,
}

#[derive(Deserialize)]
struct RawCurrent {
    time: String,
    temperature_2m: f64,
    weather_code: i32,
}

#[derive(Deserialize)]
struct RawDaily {
    time: Vec<String>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_probability_max: Vec<u8>,
}

#[tauri::command]
pub async fn search_weather_locations(
    query: String,
) -> Result<Vec<LocationCandidate>, WeatherError> {
    let query = query.trim().to_string();
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new()?;
        search_with_transport(&transport, &query)
    })
    .await
    .map_err(|_| WeatherError::ProviderError)?
}

#[tauri::command]
pub fn open_weather_attribution() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg("https://open-meteo.com/")
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn weather_state(
    app_state: tauri::State<'_, AppState>,
    runtime: tauri::State<'_, WeatherRuntime>,
) -> Result<WeatherViewState, WeatherError> {
    weather_view(&app_state, &runtime, unix_timestamp())
}

#[tauri::command]
pub async fn refresh_weather(
    app: tauri::AppHandle,
    manual: bool,
) -> Result<WeatherViewState, WeatherError> {
    let runtime = app.state::<WeatherRuntime>();
    let settings = app
        .state::<AppState>()
        .snapshot()
        .map_err(|_| WeatherError::ProviderError)?;
    let location = WeatherLocation::from_settings(&settings)?.ok_or(WeatherError::NoLocation)?;
    let unit = settings.temperature_unit;
    // Validate all fallible local state before taking the refresh gate. Once
    // acquired, every network result passes through `finish_refresh`, so an
    // invalid settings document cannot strand the single-flight flag.
    runtime.begin_refresh(manual)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new()?;
        fetch_with_transport(&transport, &location, unit, unix_timestamp())
    })
    .await
    .map_err(|_| WeatherError::ProviderError)
    .and_then(|value| value);

    runtime.finish_refresh();
    match result {
        Ok(snapshot) => {
            let payload_json =
                serde_json::to_string(&snapshot).map_err(|_| WeatherError::InvalidResponse)?;
            app.state::<AppState>()
                .database
                .upsert_weather_cache(&WeatherCacheRow {
                    location_key: snapshot.location_key.clone(),
                    temperature_unit: snapshot.temperature_unit.as_str().into(),
                    payload_json,
                    fetched_at: snapshot.fetched_at,
                })
                .map_err(|_| WeatherError::ProviderError)?;
            runtime.record("success");
            weather_view(&app.state::<AppState>(), &runtime, unix_timestamp())
        }
        Err(error) => {
            // Location is deliberately omitted: normal logs expose only the
            // provider and normalized failure class.
            eprintln!(
                "[weather] provider=Open-Meteo result={}",
                error.diagnostic_label()
            );
            runtime.record(error.diagnostic_label());
            Err(error)
        }
    }
}

fn search_with_transport(
    transport: &impl HttpTransport,
    query: &str,
) -> Result<Vec<LocationCandidate>, WeatherError> {
    let mut url = Url::parse(GEOCODING_ENDPOINT).map_err(|_| WeatherError::ProviderError)?;
    url.query_pairs_mut()
        .append_pair("name", query)
        .append_pair("count", "8")
        .append_pair("language", "en")
        .append_pair("format", "json");
    let response = checked_response(transport.get(url)?)?;
    let raw: GeocodingResponse =
        serde_json::from_str(&response.body).map_err(|_| WeatherError::InvalidResponse)?;
    raw.results
        .into_iter()
        .map(|result| {
            if result.name.trim().is_empty()
                || result.timezone.trim().is_empty()
                || !result.latitude.is_finite()
                || !result.longitude.is_finite()
            {
                return Err(WeatherError::InvalidResponse);
            }
            Ok(LocationCandidate {
                label: result.name,
                latitude: result.latitude,
                longitude: result.longitude,
                timezone: result.timezone,
                country: result.country,
                admin1: result.admin1,
            })
        })
        .collect()
}

fn fetch_with_transport(
    transport: &impl HttpTransport,
    location: &WeatherLocation,
    unit: TemperatureUnit,
    fetched_at: i64,
) -> Result<WeatherSnapshot, WeatherError> {
    let mut url = Url::parse(FORECAST_ENDPOINT).map_err(|_| WeatherError::ProviderError)?;
    url.query_pairs_mut()
        .append_pair("latitude", &location.latitude.to_string())
        .append_pair("longitude", &location.longitude.to_string())
        .append_pair("timezone", &location.timezone)
        .append_pair("current", "temperature_2m,weather_code")
        .append_pair(
            "daily",
            "temperature_2m_max,temperature_2m_min,precipitation_probability_max",
        )
        .append_pair("forecast_days", "2")
        .append_pair("temperature_unit", unit.as_str());
    let response = checked_response(transport.get(url)?)?;
    normalize_forecast(&response.body, location, unit, fetched_at)
}

fn checked_response(response: HttpResponse) -> Result<HttpResponse, WeatherError> {
    if (200..300).contains(&response.status) {
        Ok(response)
    } else {
        Err(WeatherError::ProviderError)
    }
}

fn normalize_forecast(
    payload: &str,
    location: &WeatherLocation,
    unit: TemperatureUnit,
    fetched_at: i64,
) -> Result<WeatherSnapshot, WeatherError> {
    let raw: ForecastResponse =
        serde_json::from_str(payload).map_err(|_| WeatherError::InvalidResponse)?;
    if raw.timezone != location.timezone
        || !raw.current.temperature_2m.is_finite()
        || raw.current.time.len() < 10
    {
        return Err(WeatherError::InvalidResponse);
    }
    let local_day = &raw.current.time[..10];
    let index = raw
        .daily
        .time
        .iter()
        .position(|day| day == local_day)
        .ok_or(WeatherError::InvalidResponse)?;
    let high = *raw
        .daily
        .temperature_2m_max
        .get(index)
        .filter(|value| value.is_finite())
        .ok_or(WeatherError::InvalidResponse)?;
    let low = *raw
        .daily
        .temperature_2m_min
        .get(index)
        .filter(|value| value.is_finite())
        .ok_or(WeatherError::InvalidResponse)?;
    let precipitation_probability = *raw
        .daily
        .precipitation_probability_max
        .get(index)
        .filter(|value| **value <= 100)
        .ok_or(WeatherError::InvalidResponse)?;
    Ok(WeatherSnapshot {
        condition: condition_from_wmo(raw.current.weather_code),
        temperature: raw.current.temperature_2m,
        daily_high: high,
        daily_low: low,
        precipitation_probability,
        fetched_at,
        location_key: location.key.clone(),
        timezone: location.timezone.clone(),
        temperature_unit: unit,
    })
}

fn weather_view(
    state: &AppState,
    runtime: &WeatherRuntime,
    now: i64,
) -> Result<WeatherViewState, WeatherError> {
    let settings = state.snapshot().map_err(|_| WeatherError::ProviderError)?;
    let Some(location) = WeatherLocation::from_settings(&settings)? else {
        return Ok(WeatherViewState {
            configured: false,
            snapshot: None,
            cache_status: CacheStatus::Missing,
            cache_age_seconds: None,
            refresh_recommended: false,
            refreshing: runtime.refreshing.load(Ordering::Acquire),
            last_refresh: runtime.last_refresh(),
        });
    };
    let row = state
        .database
        .weather_cache(&location.key, settings.temperature_unit.as_str())
        .map_err(|_| WeatherError::ProviderError)?;
    let snapshot = row.and_then(|row| {
        serde_json::from_str::<WeatherSnapshot>(&row.payload_json)
            .ok()
            .filter(|snapshot| {
                snapshot.location_key == location.key
                    && snapshot.temperature_unit == settings.temperature_unit
                    && snapshot.fetched_at == row.fetched_at
            })
    });
    let age = snapshot
        .as_ref()
        .map(|snapshot| now.saturating_sub(snapshot.fetched_at).max(0));
    Ok(WeatherViewState {
        configured: true,
        cache_status: age.map(cache_status).unwrap_or(CacheStatus::Missing),
        refresh_recommended: age.is_none_or(|age| age >= REFRESH_AFTER_SECONDS),
        cache_age_seconds: age,
        snapshot,
        refreshing: runtime.refreshing.load(Ordering::Acquire),
        last_refresh: runtime.last_refresh(),
    })
}

pub fn diagnostic_snapshot(
    state: &AppState,
    runtime: &WeatherRuntime,
) -> Result<WeatherDiagnosticSnapshot, String> {
    let view = weather_view(state, runtime, unix_timestamp())
        .map_err(|error| error.diagnostic_label().to_string())?;
    Ok(WeatherDiagnosticSnapshot {
        configured: view.configured,
        cache_status: match view.cache_status {
            CacheStatus::Fresh => "fresh",
            CacheStatus::Stale => "stale",
            CacheStatus::VeryStale => "very-stale",
            CacheStatus::Missing => "missing",
        },
        cache_age_seconds: view.cache_age_seconds,
        last_refresh: view.last_refresh,
    })
}

fn cache_status(age_seconds: i64) -> CacheStatus {
    if age_seconds < STALE_AFTER_SECONDS {
        CacheStatus::Fresh
    } else if age_seconds <= VERY_STALE_AFTER_SECONDS {
        CacheStatus::Stale
    } else {
        CacheStatus::VeryStale
    }
}

pub fn condition_from_wmo(code: i32) -> WeatherCondition {
    match code {
        0 => WeatherCondition::Clear,
        1 => WeatherCondition::MainlyClear,
        2 => WeatherCondition::PartlyCloudy,
        3 => WeatherCondition::Cloudy,
        45 | 48 => WeatherCondition::Fog,
        51 | 53 | 55 | 56 | 57 => WeatherCondition::Drizzle,
        61 | 63 | 65 | 66 | 67 => WeatherCondition::Rain,
        71 | 73 | 75 | 77 => WeatherCondition::Snow,
        80..=86 => WeatherCondition::Showers,
        95 | 96 | 99 => WeatherCondition::Thunderstorm,
        _ => WeatherCondition::Unknown,
    }
}

fn location_key(latitude: f64, longitude: f64, timezone: &str) -> String {
    format!("{latitude:.5},{longitude:.5}|{}", timezone.trim())
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    const GEOCODE_FIXTURE: &str = r#"{
      "results": [
        {"name":"Alpha","latitude":30.1,"longitude":104.2,"timezone":"Asia/Shanghai","country":"China","admin1":"Sichuan"},
        {"name":"Alpha","latitude":31.0,"longitude":105.0,"timezone":"Asia/Shanghai","country":"China","admin1":"Other"}
      ]
    }"#;
    const FORECAST_FIXTURE: &str = r#"{
      "timezone":"Asia/Tokyo",
      "current":{"time":"2026-08-31T00:15","temperature_2m":27.4,"weather_code":3},
      "daily":{
        "time":["2026-08-30","2026-08-31"],
        "temperature_2m_max":[31.0,32.0],
        "temperature_2m_min":[24.0,25.0],
        "precipitation_probability_max":[30,40]
      }
    }"#;

    struct FakeTransport {
        status: u16,
        body: &'static str,
        error: Option<WeatherError>,
        calls: AtomicUsize,
        last_url: Mutex<String>,
    }

    impl FakeTransport {
        fn response(status: u16, body: &'static str) -> Self {
            Self {
                status,
                body,
                error: None,
                calls: AtomicUsize::new(0),
                last_url: Mutex::new(String::new()),
            }
        }

        fn error(error: WeatherError) -> Self {
            Self {
                status: 0,
                body: "",
                error: Some(error),
                calls: AtomicUsize::new(0),
                last_url: Mutex::new(String::new()),
            }
        }
    }

    impl HttpTransport for FakeTransport {
        fn get(&self, url: Url) -> Result<HttpResponse, WeatherError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            *self.last_url.lock().expect("url") = url.to_string();
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(HttpResponse {
                status: self.status,
                body: self.body.into(),
            })
        }
    }

    fn location() -> WeatherLocation {
        WeatherLocation {
            latitude: 35.0,
            longitude: 139.0,
            timezone: "Asia/Tokyo".into(),
            key: location_key(35.0, 139.0, "Asia/Tokyo"),
        }
    }

    fn configured_state() -> AppState {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        state
            .update(|settings| {
                settings.weather_location_label = "Private label".into();
                settings.weather_latitude = Some(35.0);
                settings.weather_longitude = Some(139.0);
                settings.weather_timezone = "Asia/Tokyo".into();
            })
            .expect("configure");
        state
    }

    #[test]
    fn geocoding_handles_valid_multiple_and_empty_results() {
        let transport = FakeTransport::response(200, GEOCODE_FIXTURE);
        let results = search_with_transport(&transport, "Alpha").expect("results");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].label, "Alpha");

        let empty = FakeTransport::response(200, r#"{"results":[]}"#);
        assert!(search_with_transport(&empty, "Nowhere")
            .expect("empty")
            .is_empty());
    }

    #[test]
    fn geocoding_normalizes_invalid_json_network_and_http_errors() {
        assert_eq!(
            search_with_transport(&FakeTransport::response(200, "{"), "Alpha"),
            Err(WeatherError::InvalidResponse)
        );
        assert_eq!(
            search_with_transport(&FakeTransport::error(WeatherError::Network), "Alpha"),
            Err(WeatherError::Network)
        );
        assert_eq!(
            search_with_transport(&FakeTransport::response(500, "{}"), "Alpha"),
            Err(WeatherError::ProviderError)
        );
    }

    #[test]
    fn maps_common_wmo_condition_families_and_unknown_codes() {
        for (code, condition) in [
            (0, WeatherCondition::Clear),
            (2, WeatherCondition::PartlyCloudy),
            (3, WeatherCondition::Cloudy),
            (45, WeatherCondition::Fog),
            (53, WeatherCondition::Drizzle),
            (63, WeatherCondition::Rain),
            (75, WeatherCondition::Snow),
            (81, WeatherCondition::Showers),
            (95, WeatherCondition::Thunderstorm),
            (999, WeatherCondition::Unknown),
        ] {
            assert_eq!(condition_from_wmo(code), condition);
        }
    }

    #[test]
    fn forecast_uses_weather_timezone_day_and_supports_both_units() {
        for unit in [TemperatureUnit::Celsius, TemperatureUnit::Fahrenheit] {
            let transport = FakeTransport::response(200, FORECAST_FIXTURE);
            let snapshot =
                fetch_with_transport(&transport, &location(), unit, 100).expect("forecast");
            assert_eq!(snapshot.daily_high, 32.0);
            assert_eq!(snapshot.daily_low, 25.0);
            assert_eq!(snapshot.precipitation_probability, 40);
            assert_eq!(snapshot.temperature_unit, unit);
            let url = transport.last_url.lock().expect("url").clone();
            assert!(url.contains("timezone=Asia%2FTokyo"));
            assert!(url.contains(&format!("temperature_unit={}", unit.as_str())));
        }
    }

    #[test]
    fn forecast_normalizes_timeout_http_and_malformed_payload() {
        assert_eq!(
            fetch_with_transport(
                &FakeTransport::error(WeatherError::Timeout),
                &location(),
                TemperatureUnit::Celsius,
                1
            ),
            Err(WeatherError::Timeout)
        );
        assert_eq!(
            fetch_with_transport(
                &FakeTransport::response(503, "{}"),
                &location(),
                TemperatureUnit::Celsius,
                1
            ),
            Err(WeatherError::ProviderError)
        );
        assert_eq!(
            fetch_with_transport(
                &FakeTransport::response(200, "not json"),
                &location(),
                TemperatureUnit::Celsius,
                1
            ),
            Err(WeatherError::InvalidResponse)
        );
    }

    #[test]
    fn cache_reports_missing_fresh_stale_and_very_stale() {
        let state = configured_state();
        let runtime = WeatherRuntime::default();
        assert_eq!(
            weather_view(&state, &runtime, 100)
                .expect("missing")
                .cache_status,
            CacheStatus::Missing
        );
        for (age, expected) in [
            (60, CacheStatus::Fresh),
            (3 * 60 * 60, CacheStatus::Stale),
            (25 * 60 * 60, CacheStatus::VeryStale),
        ] {
            let fetched_at = 100_000 - age;
            let snapshot = WeatherSnapshot {
                condition: WeatherCondition::Cloudy,
                temperature: 20.0,
                daily_high: 22.0,
                daily_low: 18.0,
                precipitation_probability: 10,
                fetched_at,
                location_key: location().key,
                timezone: "Asia/Tokyo".into(),
                temperature_unit: TemperatureUnit::Celsius,
            };
            state
                .database
                .upsert_weather_cache(&WeatherCacheRow {
                    location_key: snapshot.location_key.clone(),
                    temperature_unit: "celsius".into(),
                    payload_json: serde_json::to_string(&snapshot).expect("json"),
                    fetched_at,
                })
                .expect("cache");
            assert_eq!(
                weather_view(&state, &runtime, 100_000)
                    .expect("view")
                    .cache_status,
                expected
            );
        }
    }

    #[test]
    fn cache_isolated_by_location_and_ignores_corruption() {
        let state = configured_state();
        let runtime = WeatherRuntime::default();
        state
            .database
            .upsert_weather_cache(&WeatherCacheRow {
                location_key: location().key,
                temperature_unit: "celsius".into(),
                payload_json: "corrupt".into(),
                fetched_at: 90,
            })
            .expect("corrupt cache");
        assert!(weather_view(&state, &runtime, 100)
            .expect("view")
            .snapshot
            .is_none());
        state
            .update(|settings| {
                settings.weather_latitude = Some(40.0);
                settings.weather_longitude = Some(120.0);
            })
            .expect("change location");
        let changed = weather_view(&state, &runtime, 100).expect("changed");
        assert_eq!(changed.cache_status, CacheStatus::Missing);
        assert!(changed.snapshot.is_none());
    }

    #[test]
    fn refresh_runtime_prevents_concurrency_and_manual_spam() {
        let runtime = WeatherRuntime::default();
        runtime.begin_refresh(false).expect("first");
        assert_eq!(runtime.begin_refresh(false), Err(WeatherError::InProgress));
        runtime.finish_refresh();
        runtime.begin_refresh(true).expect("manual");
        runtime.finish_refresh();
        assert_eq!(runtime.begin_refresh(true), Err(WeatherError::Cooldown));
    }
}
