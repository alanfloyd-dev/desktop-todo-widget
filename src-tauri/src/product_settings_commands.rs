//! The settings IPC surface: reading the product state and applying a
//! settings patch from the frontend.
//!
//! `product_state` is a projection of the persisted document through the
//! runtime's QA overlay; `update_product_settings` is the one command that
//! writes the settings document. Both return the same [`ProductViewState`]
//! the `product-state` event carries, so a native action and a
//! frontend-initiated save cannot produce different views.
//!
//! This module owns the patch pipeline only: validation runs before the
//! document changes, replaced assets are released after persistence succeeds,
//! and the window's native material refresh goes through the product window's
//! narrow `apply_native_composition` entry. Window mode transitions, geometry,
//! and the native state machine live in [`crate::product_window`].

use crate::{
    appearance::{self, AppearanceProfiles},
    product_window::{apply_native_composition, ProductWindowRuntime},
    settings::{self, AppState, ProductSettings, RenderingBackend, TemperatureUnit},
    task_day,
};
use serde::{Deserialize, Serialize};
use tauri::{Manager, WebviewWindow};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    day_rollover: Option<String>,
    weather_location_label: Option<String>,
    weather_latitude: Option<f64>,
    weather_longitude: Option<f64>,
    weather_timezone: Option<String>,
    weather_country: Option<String>,
    weather_admin1: Option<String>,
    temperature_unit: Option<TemperatureUnit>,
    appearance: Option<String>,
    /// One complete appearance per window mode. The Settings panel edits a
    /// profile draft and writes all three back, so the payload stays a single
    /// value instead of needing a "which mode" side channel.
    appearance_profiles: Option<AppearanceProfiles>,
    #[serde(default, deserialize_with = "deserialize_nullable_option")]
    avatar_asset_id: Option<Option<String>>,
    sidebar_width: Option<u32>,
    display_name: Option<String>,
    /// The complete Quick Links list, in display order.
    ///
    /// Replace-not-patch on purpose: add, edit, delete and reorder are all
    /// "this is the new list", and the ids are assigned by the client that owns
    /// the draft. A per-operation protocol (add/delete/move commands) would have
    /// to keep a second implementation of the same ordering rules on the Rust
    /// side for no benefit at this size.
    quick_links: Option<Vec<settings::QuickLink>>,
    /// Persisted rendering backend preference. Applied to settings only; the
    /// hosting backend itself is chosen at startup and needs a restart.
    rendering_backend: Option<RenderingBackend>,
    /// Persisted UI language preference. Applied immediately on the frontend.
    language: Option<crate::locale::Language>,
}

fn deserialize_nullable_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductViewState {
    settings: ProductSettings,
    desktop_experimental: bool,
    database_path: String,
    /// Raw operating system locale, used by the frontend to resolve the
    /// `System` language choice with the same rule as the native menus.
    system_locale: String,
}

impl ProductViewState {
    pub fn new(state: &AppState, settings: ProductSettings) -> Self {
        Self {
            settings,
            desktop_experimental: true,
            database_path: state.database.path().display().to_string(),
            system_locale: crate::locale::system_locale_hint().to_string(),
        }
    }
}

#[tauri::command]
pub fn product_state(
    state: tauri::State<'_, AppState>,
    runtime: tauri::State<'_, ProductWindowRuntime>,
) -> Result<ProductViewState, String> {
    let settings = runtime.effective_settings(state.snapshot()?);
    Ok(ProductViewState::new(&state, settings))
}

#[tauri::command]
pub fn update_product_settings(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<ProductViewState, String> {
    if let Some(links) = patch.quick_links.as_deref() {
        validate_quick_links(links)?;
    }
    if let Some(rollover) = patch.day_rollover.as_deref() {
        task_day::parse_rollover(rollover)?;
    }
    validate_weather_patch(&patch)?;
    let before = state.snapshot()?;
    let settings = state.update(|settings| {
        if let Some(value) = patch.day_rollover {
            settings.day_rollover = value;
        }
        if let Some(value) = patch.weather_location_label {
            settings.weather_location_label = value.trim().to_string();
        }
        if let Some(value) = patch.weather_latitude {
            settings.weather_latitude = Some(value);
        }
        if let Some(value) = patch.weather_longitude {
            settings.weather_longitude = Some(value);
        }
        if let Some(value) = patch.weather_timezone {
            settings.weather_timezone = value.trim().to_string();
        }
        if let Some(value) = patch.weather_country {
            settings.weather_country = value.trim().to_string();
        }
        if let Some(value) = patch.weather_admin1 {
            settings.weather_admin1 = value.trim().to_string();
        }
        if let Some(value) = patch.temperature_unit {
            settings.temperature_unit = value;
        }
        if let Some(value) = patch.appearance {
            settings.appearance = value;
        }
        if let Some(value) = patch.appearance_profiles {
            settings.appearance_profiles = value;
        }
        if let Some(value) = patch.avatar_asset_id {
            settings.avatar_asset_id = value;
        }
        if let Some(value) = patch.sidebar_width {
            settings.sidebar_width = value.clamp(320, 560);
        }
        if let Some(value) = patch.rendering_backend {
            // Persisted only. The WebView hosting backend is fixed when the
            // WebView is created, so applying it would require recreating the
            // WebView; the UI tells the user a restart is required.
            //
            // Standard and Enhanced are orthogonal to the window mode: every
            // mode runs on either backend. Do not "unify" them here.
            // See docs/phase-7c3b4-dual-backend-release-decision.md.
            settings.rendering_backend = value;
        }
        if let Some(value) = patch.language {
            // Persisted for the frontend, which re-renders immediately, and for
            // the native menus, which the next tray refresh rebuilds.
            settings.language = value;
        }
        if let Some(value) = patch.display_name {
            settings.display_name = value.trim().to_string();
        }
        if let Some(value) = patch.quick_links {
            settings.quick_links = value;
        }
    })?;
    appearance::cleanup_replaced_assets(
        &state,
        &before.appearance_profiles,
        before.avatar_asset_id.as_deref(),
        &settings.appearance_profiles,
        settings.avatar_asset_id.as_deref(),
    );
    let effective_settings = window
        .app_handle()
        .state::<ProductWindowRuntime>()
        .effective_settings(settings.clone());
    apply_native_composition(
        &window,
        &window.app_handle().state::<ProductWindowRuntime>(),
        effective_settings.mode,
        effective_settings.floating_presentation,
        effective_settings
            .appearance_profiles
            .for_mode(effective_settings.mode),
    )?;
    Ok(ProductViewState::new(&state, effective_settings))
}

fn validate_weather_patch(patch: &SettingsPatch) -> Result<(), String> {
    if let Some(latitude) = patch.weather_latitude {
        if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            return Err("weather location latitude is invalid".into());
        }
    }
    if let Some(longitude) = patch.weather_longitude {
        if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
            return Err("weather location longitude is invalid".into());
        }
    }
    if patch.weather_latitude.is_some() != patch.weather_longitude.is_some() {
        return Err("weather location requires both coordinates".into());
    }
    if patch.weather_latitude.is_some()
        && patch
            .weather_timezone
            .as_deref()
            .is_none_or(|timezone| timezone.trim().is_empty())
    {
        return Err("weather location timezone is required".into());
    }
    Ok(())
}

/// Rejects a Quick Links list this build cannot store.
///
/// Names may repeat and URLs may repeat: the user decides what their link list
/// contains, so the only rejections here are the ones that would produce a row
/// with no label or a URL that cannot be launched.
fn validate_quick_links(links: &[settings::QuickLink]) -> Result<(), String> {
    for link in links {
        if link.id.trim().is_empty() {
            return Err("quick link is missing its id".into());
        }
        if link.name.trim().is_empty() {
            return Err("quick link name cannot be empty".into());
        }
        settings::validate_quick_link_url(link.url.trim())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_quick_links;

    #[test]
    fn quick_links_reject_only_unusable_rows() {
        let link = |id: &str, name: &str, url: &str| crate::settings::QuickLink {
            id: id.into(),
            name: name.into(),
            url: url.into(),
        };
        // Duplicate names and duplicate URLs are the user's call, not an error.
        assert!(validate_quick_links(&[
            link("a", "Docs", "https://example.com/docs"),
            link("b", "Docs", "https://example.com/docs"),
        ])
        .is_ok());
        // An empty list is valid: the product section is simply hidden.
        assert!(validate_quick_links(&[]).is_ok());

        assert!(validate_quick_links(&[link("a", "   ", "https://example.com")]).is_err());
        assert!(validate_quick_links(&[link(" ", "Docs", "https://example.com")]).is_err());
        assert!(validate_quick_links(&[link("a", "Docs", "javascript:alert(1)")]).is_err());
    }
}
