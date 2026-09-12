use crate::{
    appearance,
    settings::{AppState, ProductSettings},
    task_day,
    weather::{self, WeatherRuntime},
    window_mode::{self, NativeWindowState, SupportWindowSnapshot},
};
use tauri::WebviewWindow;

#[tauri::command]
pub fn copyable_diagnostics(
    window: WebviewWindow,
    app_state: tauri::State<'_, AppState>,
    native_state: tauri::State<'_, NativeWindowState>,
    weather_runtime: tauri::State<'_, WeatherRuntime>,
) -> Result<String, String> {
    let settings = app_state.snapshot()?;
    let native = window_mode::support_window_snapshot(&window, &native_state)?;
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .and_then(|value| value.name().cloned())
        .or(settings.monitor_identity.clone())
        .unwrap_or_else(|| "unknown".into());
    let input = DiagnosticInput {
        app_version: env!("CARGO_PKG_VERSION").into(),
        windows_version: windows_version(),
        webview2_version: tauri::webview_version().unwrap_or_else(|_| "unavailable".into()),
        monitor,
        schema_version: app_state.database.schema_version()?,
        task_count: app_state.database.task_count()?,
        current_task_day: task_day::current_task_day(&settings.day_rollover)?
            .format("%Y-%m-%d")
            .to_string(),
        weather: weather::diagnostic_snapshot(&app_state, &weather_runtime)?,
        background_available: appearance::background_available(
            &app_state,
            settings.active_appearance(),
        ),
        avatar_configured: appearance::avatar_available(
            &app_state,
            settings.avatar_asset_id.as_deref(),
        ),
        native,
    };
    Ok(render_diagnostics(&settings, &input))
}

struct DiagnosticInput {
    app_version: String,
    windows_version: String,
    webview2_version: String,
    monitor: String,
    schema_version: i64,
    task_count: i64,
    current_task_day: String,
    weather: weather::WeatherDiagnosticSnapshot,
    background_available: bool,
    avatar_configured: bool,
    native: SupportWindowSnapshot,
}

fn render_diagnostics(settings: &ProductSettings, input: &DiagnosticInput) -> String {
    format!(
        "Alan Desktop diagnostics\n\
         App version: {}\n\
         Windows: {}\n\
         WebView2: {}\n\
         Window mode: {:?}\n\
         Parent class: {}\n\
         Bounds: {},{} · {}x{}\n\
         Monitor: {}\n\
         Attachment: {}\n\
         Locked: {}\n\
         Topmost: {}\n\
         Database schema: {}\n\
         Task count: {}\n\
         Current task day: {}\n\
         Weather provider: Open-Meteo\n\
         Weather configured: {}\n\
         Weather cache: {}\n\
         Weather cache age: {}\n\
         Weather last refresh: {}\n\
         Appearance background: {:?}\n\
         Appearance available: {}\n\
         Contrast mode: {:?}\n\
         Resolved contrast: {}\n\
         Avatar configured: {}\n\
         Floating presentation: {:?}\n\
         Floating Orb bounds: {},{} · {}x{} DIP",
        input.app_version,
        input.windows_version,
        input.webview2_version,
        settings.mode,
        input.native.parent_class,
        input.native.x,
        input.native.y,
        input.native.width,
        input.native.height,
        input.monitor,
        if input.native.attachment_valid {
            "valid"
        } else {
            "invalid"
        },
        settings.locked,
        input.native.always_on_top,
        input.schema_version,
        input.task_count,
        input.current_task_day,
        if input.weather.configured {
            "yes"
        } else {
            "no"
        },
        input.weather.cache_status,
        input
            .weather
            .cache_age_seconds
            .map(|seconds| format!("{seconds}s"))
            .unwrap_or_else(|| "unavailable".into()),
        input.weather.last_refresh,
        settings.active_appearance().background_type,
        if input.background_available {
            "yes"
        } else {
            "no"
        },
        settings.active_appearance().text_contrast,
        appearance::resolved_for_diagnostics(settings.active_appearance()).as_str(),
        if input.avatar_configured { "yes" } else { "no" },
        settings.floating_presentation,
        settings
            .floating_orb_x
            .map(|value| value.to_string())
            .unwrap_or_else(|| "auto".into()),
        settings
            .floating_orb_y
            .map(|value| value.to_string())
            .unwrap_or_else(|| "auto".into()),
        crate::product_window::ORB_SIZE_DIP,
        crate::product_window::ORB_SIZE_DIP,
    )
}

#[cfg(target_os = "windows")]
fn windows_version() -> String {
    windows_build_from_registry().unwrap_or_else(|| "Windows build unavailable".into())
}

#[cfg(target_os = "windows")]
fn windows_build_from_registry() -> Option<String> {
    use std::ffi::c_void;

    type Hkey = isize;
    const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002_u32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: Hkey,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            value_type: *mut u32,
            data: *mut c_void,
            data_size: *mut u32,
        ) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let sub_key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let build_name = wide("CurrentBuildNumber");
    let ubr_name = wide("UBR");
    let mut build_buffer = [0_u16; 32];
    let mut build_bytes = std::mem::size_of_val(&build_buffer) as u32;
    let mut ubr = 0_u32;
    let mut ubr_bytes = std::mem::size_of::<u32>() as u32;

    // SAFETY: RegGetValueW receives fixed-size writable buffers with their
    // exact byte lengths. HKLM is a predefined non-owning handle, so there is
    // no registry handle to close and no localized shell output to decode.
    let build_status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key.as_ptr(),
            build_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            build_buffer.as_mut_ptr().cast(),
            &mut build_bytes,
        )
    };
    // SAFETY: `ubr` is a DWORD-sized output and `ubr_bytes` advertises that
    // same size. Failure is handled by omitting the revision rather than
    // trusting partial data.
    let ubr_status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key.as_ptr(),
            ubr_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut ubr as *mut u32).cast(),
            &mut ubr_bytes,
        )
    };
    if build_status != 0 {
        return None;
    }
    let length = build_buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(build_buffer.len());
    let build = String::from_utf16(&build_buffer[..length]).ok()?;
    let build_number = build.parse::<u32>().ok()?;
    let product = if build_number >= 22_000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    Some(if ubr_status == 0 {
        format!("{product} · build {build}.{ubr}")
    } else {
        format!("{product} · build {build}")
    })
}

#[cfg(not(target_os = "windows"))]
fn windows_version() -> String {
    "unsupported platform".into()
}

#[cfg(test)]
mod tests {
    use super::{render_diagnostics, DiagnosticInput};
    use crate::{
        settings::ProductSettings, weather::WeatherDiagnosticSnapshot,
        window_mode::SupportWindowSnapshot,
    };

    #[test]
    fn support_diagnostics_use_an_allowlist_and_redact_private_settings() {
        let settings = ProductSettings {
            weather_location_label: "Private City".into(),
            weather_latitude: Some(12.3456),
            weather_longitude: Some(65.4321),
            homepage_url: "https://private.example/secret".into(),
            display_name: "Private Person".into(),
            avatar_asset_id: Some("avatar-00000000-0000-0000-0000-000000000000-private.png".into()),
            appearance_profiles: crate::appearance::AppearanceProfiles {
                // The active profile is the one diagnostics report on.
                floating: crate::appearance::AppearanceSettings {
                    image_asset_id: Some(
                        "background-00000000-0000-0000-0000-000000000000-private.png".into(),
                    ),
                    ..crate::appearance::AppearanceSettings::default()
                },
                ..crate::appearance::AppearanceProfiles::default()
            },
            ..ProductSettings::default()
        };
        let input = DiagnosticInput {
            app_version: "0.1.0".into(),
            windows_version: "Windows 11 build 12345".into(),
            webview2_version: "123.0".into(),
            monitor: "DISPLAY1".into(),
            schema_version: 2,
            task_count: 4,
            current_task_day: "2026-08-29".into(),
            weather: WeatherDiagnosticSnapshot {
                configured: true,
                cache_status: "stale",
                cache_age_seconds: Some(7_200),
                last_refresh: "network-error".into(),
            },
            background_available: true,
            avatar_configured: false,
            native: SupportWindowSnapshot {
                parent_class: "SHELLDLL_DefView".into(),
                attachment_valid: true,
                always_on_top: false,
                x: 20,
                y: 30,
                width: 620,
                height: 720,
            },
        };
        let rendered = render_diagnostics(&settings, &input);
        assert!(rendered.contains("Window mode: Floating"));
        assert!(rendered.contains("Database schema: 2"));
        assert!(rendered.contains("Task count: 4"));
        assert!(rendered.contains("Current task day: 2026-08-29"));
        assert!(rendered.contains("Weather provider: Open-Meteo"));
        assert!(rendered.contains("Weather configured: yes"));
        assert!(rendered.contains("Weather cache: stale"));
        assert!(rendered.contains("Appearance background: Glass"));
        assert!(rendered.contains("Avatar configured: no"));
        assert!(rendered.contains("Floating Orb bounds:"));
        for private in [
            "Private City",
            "12.3456",
            "65.4321",
            "private.example",
            "Private Person",
            "00000000-0000-0000-0000-000000000000-private",
            "C:\\Users\\Private\\project",
        ] {
            assert!(!rendered.contains(private));
        }
    }
}
