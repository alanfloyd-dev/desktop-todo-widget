# Weather

Alan Desktop treats weather as optional ambient information, not as an application startup dependency. It uses the key-free Open-Meteo Geocoding and Forecast APIs directly; there is no Alan Desktop account, proxy, or sync service.

## Location and timezone

Settings searches only after an explicit button press or Enter. The user must select a candidate; the application never chooses the first result or requests Windows location permission. The selected label, coordinates, timezone, and optional country/admin text are stored locally. Later forecasts use the saved coordinates and timezone without repeating geocoding.

Weather dates belong to the selected location. The Forecast API response's current local date selects the matching daily high, low, and precipitation probability. Todo task days remain based on the computer's local clock and configured rollover; the two timezone domains do not interact.

## Provider boundary

`weather.rs` is the only module that consumes Open-Meteo response JSON or maps WMO weather codes. Vue receives a normalized snapshot containing condition, current temperature, daily high/low, maximum precipitation probability, fetch time, location key, timezone, and unit. Unknown codes become `unknown`; malformed or incomplete responses are discarded without panicking.

Requests use a ten-second timeout, require a successful HTTP status, and normalize failures as no-location, network, timeout, invalid-response, or provider errors. Normal logs include only provider and error category—never a label, coordinate, timezone, or search query.

## Cache and refresh policy

Migration 3 adds `weather_cache`. Cache rows are selected by coordinate/timezone identity plus temperature unit. A cache for one place or unit can therefore never appear under another setting.

- refresh is recommended at 60 minutes
- display is fresh below 2 hours
- display is stale from 2 through 24 hours
- display is very stale after 24 hours
- manual refresh has a 30-second cooldown

The UI reads matching cache first and renders immediately. Stale or missing configured data refreshes in the background. A failed refresh leaves the last valid cache intact. One Rust single-flight gate prevents concurrent requests, while the root App owns one hourly timer that survives layout and window-mode changes without duplication.

Very stale data is retained locally but the main Widget reports weather unavailable with its last-update age. No location shows a quiet link into Settings.

## Privacy and attribution

Weather configuration and search results are local/private data. Copy diagnostics reports only whether weather is configured, cache freshness/age, provider name, and normalized last-refresh result.

Weather data by [Open-Meteo.com](https://open-meteo.com/), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Attribution is visible in Settings and `THIRD_PARTY_NOTICES.md`, rather than occupying the Widget's ambient first screen.
