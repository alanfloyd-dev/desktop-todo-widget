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
    product_window::apply_native_composition,
    settings::{self, AppState, ProductSettings, TemperatureUnit},
    task_day,
};
use serde::{Deserialize, Serialize};
use tauri::WebviewWindow;

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
    /// The complete module visibility preferences, replaced as one coherent
    /// object. Absent = no change; present = explicit replacement, with the
    /// unknown nested ids a client cannot express preserved from the stored
    /// document (see `ModuleVisibility::replaced_by`). Known members are
    /// type-checked during this struct's deserialization, so an invalid value
    /// fails the command before anything is mutated.
    module_visibility: Option<settings::ModuleVisibility>,
    /// The dashboard layout document, version-gated. Absent = no change; a
    /// version-1 patch merges entry by entry (unknown future-module entries
    /// preserved, per-entry unknown fields preserved); an opaque document —
    /// only ever produced by the frontend pass-through of a stored
    /// future-format layout — replaces verbatim. A version conflict between
    /// patch and stored document fails the command before anything mutates
    /// (see `ModuleLayout::merged_with`).
    module_layout: Option<settings::ModuleLayout>,
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
pub fn product_state(state: tauri::State<'_, AppState>) -> Result<ProductViewState, String> {
    let settings = state.snapshot()?;
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
    if let Some(layout) = &patch.module_layout {
        validate_module_layout_patch(layout)?;
    }
    let before = state.snapshot()?;
    // Computed from the pre-update snapshot so a version conflict (or any
    // merge refusal) fails the command before the update transaction begins.
    let merged_layout = match &patch.module_layout {
        Some(layout) => Some(before.module_layout.merged_with(layout.clone())?),
        None => None,
    };
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
        // The rendering_backend patch field was dropped in the standard-only
        // retirement: the hosting decision is pinned to Standard at startup
        // (see `run()` in lib.rs). Serde silently ignores the key if an older
        // frontend still sends it.
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
        if let Some(value) = patch.module_visibility {
            settings.module_visibility = settings.module_visibility.clone().replaced_by(value);
        }
        if let Some(value) = merged_layout {
            settings.module_layout = value;
        }
    })?;
    appearance::cleanup_replaced_assets(
        &state,
        &before.appearance_profiles,
        before.avatar_asset_id.as_deref(),
        &settings.appearance_profiles,
        settings.avatar_asset_id.as_deref(),
    );
    let effective_settings = settings.clone();
    apply_native_composition(
        &window,
        effective_settings.mode,
        effective_settings.floating_presentation,
        &effective_settings
            .appearance_profiles
            .for_mode(effective_settings.mode),
    )?;
    Ok(ProductViewState::new(&state, effective_settings))
}

/// Rejects a layout patch this build must not store.
///
/// An opaque patch is the frontend pass-through of a stored future-format
/// layout document and carries no semantics to validate. A version-1 patch is
/// the product surface: entries must name themselves, spans must be 1 or 2,
/// and the list is bounded — `serde_json::Number` already excludes non-finite
/// orders, so nothing else needs rejecting here. Structural merge semantics
/// (unknown-entry preservation, version conflicts) live in
/// `ModuleLayout::merged_with`, which runs against the stored document after
/// this check.
fn validate_module_layout_patch(layout: &settings::ModuleLayout) -> Result<(), String> {
    const MAX_LAYOUT_ENTRIES: usize = 1000;
    match layout {
        settings::ModuleLayout::Opaque(_) => Ok(()),
        settings::ModuleLayout::V1(document) => {
            if document.entries.len() > MAX_LAYOUT_ENTRIES {
                return Err(format!(
                    "module layout carries more than {MAX_LAYOUT_ENTRIES} entries"
                ));
            }
            for entry in &document.entries {
                if entry.id.trim().is_empty() {
                    return Err("module layout entry is missing its id".into());
                }
                if let Some(col_span) = entry.col_span {
                    if col_span != 1 && col_span != 2 {
                        return Err(format!(
                            "module layout entry {} carries an unsupported column span",
                            entry.id
                        ));
                    }
                }
            }
            Ok(())
        }
    }
}

fn validate_weather_patch(patch: &SettingsPatch) -> Result<(), String> {    if let Some(latitude) = patch.weather_latitude {
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
    use super::{validate_module_layout_patch, validate_quick_links, SettingsPatch};
    use crate::settings::ModuleLayout;

    /// The visibility patch member is optional (absent = no change) and its
    /// known members are type-checked during deserialization — the command's
    /// only validation gate, and it runs before anything is mutated.
    #[test]
    fn module_visibility_patch_is_optional_and_type_checked() {
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({})).expect("empty");
        assert!(patch.module_visibility.is_none());

        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "moduleVisibility": { "todo": false }
        }))
        .expect("visibility patch");
        let visibility = patch.module_visibility.expect("present");
        assert!(!visibility.todo);
        assert!(visibility.weather && visibility.shortcuts && visibility.review);

        assert!(
            serde_json::from_value::<SettingsPatch>(serde_json::json!({
                "moduleVisibility": { "todo": "yes" }
            }))
            .is_err(),
            "a non-boolean known member must fail, not default"
        );
    }

    /// The layout patch member is optional, round-trips both document kinds —
    /// version-1 documents structurally, future-format documents opaquely —
    /// and its version-1 entries are validated before any merge runs.
    #[test]
    fn module_layout_patch_is_optional_version_gated_and_validated() {
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({})).expect("empty");
        assert!(patch.module_layout.is_none());

        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "moduleLayout": {
                "version": 1,
                "entries": [
                    { "id": "todo", "order": 0, "colSpan": 2 },
                    { "id": "weather", "order": 1, "colSpan": 1 }
                ]
            }
        }))
        .expect("v1 layout patch");
        let ModuleLayout::V1(document) = patch.module_layout.expect("present") else {
            panic!("a version-1 patch parses structurally");
        };
        assert_eq!(document.entries.len(), 2);

        // The pass-through of a stored future-format document arrives opaque,
        // not as an error: preservation must not depend on the shape.
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "moduleLayout": { "version": 2, "entries": [], "futureField": { "mode": "new" } }
        }))
        .expect("opaque layout patch");
        assert!(matches!(patch.module_layout, Some(ModuleLayout::Opaque(_))));
        assert!(validate_module_layout_patch(patch.module_layout.as_ref().unwrap()).is_ok());

        // A v1 patch naming no module, an invalid span, and an oversized list
        // are all rejected explicitly before anything mutates.
        let bad = serde_json::json!({
            "moduleLayout": { "version": 1, "entries": [ { "id": "  ", "order": 0, "colSpan": 1 } ] }
        });
        let parsed: SettingsPatch = serde_json::from_value(bad).expect("parses");
        assert!(validate_module_layout_patch(parsed.module_layout.as_ref().unwrap()).is_err());

        let bad = serde_json::json!({
            "moduleLayout": { "version": 1, "entries": [ { "id": "todo", "order": 0, "colSpan": 3 } ] }
        });
        let parsed: SettingsPatch = serde_json::from_value(bad).expect("parses");
        assert!(validate_module_layout_patch(parsed.module_layout.as_ref().unwrap()).is_err());

        let oversized: Vec<_> = (0..1001)
            .map(|index| {
                serde_json::json!({ "id": format!("m{index}"), "order": index, "colSpan": 1 })
            })
            .collect();
        let parsed: SettingsPatch = serde_json::from_value(serde_json::json!({
            "moduleLayout": { "version": 1, "entries": oversized }
        }))
        .expect("parses");
        assert!(validate_module_layout_patch(parsed.module_layout.as_ref().unwrap()).is_err());
    }

    #[test]
    fn quick_links_reject_only_unusable_rows() {
        let link = |id: &str, name: &str, url: &str| crate::settings::QuickLink::new(id, name, url);
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
