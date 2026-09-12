use crate::{
    appearance::{self, AppearanceProfiles, AppearanceSettings},
    database::{Database, Shortcut},
    locale::Language,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const PRODUCT_SETTINGS_KEY: &str = "product_settings";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductWindowMode {
    Sidebar,
    #[default]
    Floating,
    Desktop,
}

impl TryFrom<&str> for ProductWindowMode {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "sidebar" => Ok(Self::Sidebar),
            "floating" => Ok(Self::Floating),
            "desktop" => Ok(Self::Desktop),
            _ => Err(format!("unknown product window mode: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FloatingPresentation {
    #[default]
    Collapsed,
    Expanded,
}

/// WebView2 hosting backend for the product window.
///
/// This is a product-level choice, deliberately expressed without naming any
/// hosting API, and it is **orthogonal to [`ProductWindowMode`]**: Floating,
/// Sidebar, and Desktop all work on either backend.
///
/// * `Standard` — the existing windowed WebView2 controller. Full Windows UI
///   Automation exposure, so screen readers and automation tools can read the
///   content. This is the release default.
/// * `Enhanced` — the CompositionController path that enables Acrylic and
///   transparent hosting. It currently does **not** expose the WebView content
///   tree to Windows UI Automation, so it is opt-in.
///
/// The choice is read once at startup because the controller type is fixed when
/// the WebView is created; it cannot be changed without recreating the WebView.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderingBackend {
    #[default]
    Standard,
    Enhanced,
}

impl RenderingBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Enhanced => "enhanced",
        }
    }

    /// True when this backend hosts the WebView through the composition
    /// controller. The only decision the platform layer needs.
    pub fn uses_composition_hosting(self) -> bool {
        matches!(self, Self::Enhanced)
    }
}

impl TryFrom<&str> for RenderingBackend {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "standard" => Ok(Self::Standard),
            "enhanced" => Ok(Self::Enhanced),
            other => Err(format!("unsupported rendering backend: {other}")),
        }
    }
}

impl TemperatureUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }
}

impl TryFrom<&str> for SidebarSide {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => Err(format!("unknown sidebar side: {value}")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ProductSettings {
    #[serde(default = "legacy_geometry_units_version")]
    pub geometry_units_version: u8,
    pub mode: ProductWindowMode,
    /// Which WebView2 hosting backend the window is created with.
    ///
    /// Orthogonal to `mode`: every window mode works on either backend. Chosen
    /// once at startup because the hosting backend is fixed when the WebView is
    /// created and cannot be swapped afterwards.
    pub rendering_backend: RenderingBackend,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub monitor_identity: Option<String>,
    pub floating_presentation: FloatingPresentation,
    pub floating_orb_x: Option<i32>,
    pub floating_orb_y: Option<i32>,
    pub floating_orb_monitor_identity: Option<String>,
    pub desktop_x: Option<i32>,
    pub desktop_y: Option<i32>,
    pub desktop_width: u32,
    pub desktop_height: u32,
    pub sidebar_side: SidebarSide,
    pub sidebar_width: u32,
    pub always_on_top: bool,
    pub locked: bool,
    pub day_rollover: String,
    #[serde(alias = "weatherLocation")]
    pub weather_location_label: String,
    pub weather_latitude: Option<f64>,
    pub weather_longitude: Option<f64>,
    pub weather_timezone: String,
    pub weather_country: String,
    pub weather_admin1: String,
    pub temperature_unit: TemperatureUnit,
    /// Product UI language. `System` follows the operating system locale; see
    /// [`crate::locale`] for the (deliberately small) resolution rules.
    pub language: Language,
    pub appearance: String,
    /// Appearance per window mode: Sidebar, Floating, and Desktop each keep an
    /// independent profile.
    pub appearance_profiles: AppearanceProfiles,
    /// Deserialize-only bridge for the pre-profile settings document.
    ///
    /// Documents written before per-mode profiles stored one `appearanceSettings`
    /// for every mode. That key still parses into this field, and
    /// [`ProductSettings::migrate_legacy_appearance`] copies it into all three
    /// profiles so an existing user's look survives the upgrade unchanged. It is
    /// never serialized: the written document carries `appearanceProfiles` only,
    /// so the migration runs exactly once.
    #[serde(default, skip_serializing, rename = "appearanceSettings")]
    pub legacy_appearance_settings: Option<AppearanceSettings>,
    pub display_name: String,
    pub avatar_asset_id: Option<String>,
    pub homepage_label: String,
    pub homepage_url: String,
}

impl Default for ProductSettings {
    fn default() -> Self {
        Self {
            geometry_units_version: 1,
            mode: ProductWindowMode::Floating,
            // v1 default: the compatibility backend, because it keeps full UI
            // Automation exposure for screen readers. The Enhanced backend is
            // opt-in and documented as accessibility-limited.
            rendering_backend: RenderingBackend::Standard,
            x: None,
            y: None,
            width: 620,
            height: 720,
            monitor_identity: None,
            floating_presentation: FloatingPresentation::Collapsed,
            floating_orb_x: None,
            floating_orb_y: None,
            floating_orb_monitor_identity: None,
            desktop_x: None,
            desktop_y: None,
            desktop_width: 420,
            desktop_height: 700,
            sidebar_side: SidebarSide::Left,
            sidebar_width: 380,
            always_on_top: false,
            locked: false,
            day_rollover: "04:00".into(),
            weather_location_label: String::new(),
            weather_latitude: None,
            weather_longitude: None,
            weather_timezone: String::new(),
            weather_country: String::new(),
            weather_admin1: String::new(),
            temperature_unit: TemperatureUnit::Celsius,
            // Localized UI is opt-in-by-detection: "System" is the default, so a
            // first run matches the operating system language.
            language: Language::System,
            appearance: "geological_observatory".into(),
            appearance_profiles: AppearanceProfiles::default(),
            legacy_appearance_settings: None,
            display_name: "Your Name".into(),
            avatar_asset_id: None,
            homepage_label: "Homepage".into(),
            homepage_url: String::new(),
        }
    }
}

impl ProductSettings {
    /// The appearance profile of the window mode this document is currently in.
    pub fn active_appearance(&self) -> &AppearanceSettings {
        self.appearance_profiles.for_mode(self.mode)
    }

    /// Fans a legacy single-appearance document out to the three profiles.
    ///
    /// Called from [`Self::normalize`], which runs on load before the state is
    /// persisted, so an upgraded document is written with the profiles and
    /// without the legacy key. Taking the value makes the fan-out one-shot.
    fn migrate_legacy_appearance(&mut self) {
        if let Some(legacy) = self.legacy_appearance_settings.take() {
            self.appearance_profiles = AppearanceProfiles::from_all(legacy);
        }
    }

    pub fn normalize(&mut self) {
        self.width = self.width.clamp(360, 1100);
        self.height = self.height.clamp(500, 1200);
        self.desktop_width = self.desktop_width.clamp(360, 1100);
        self.desktop_height = self.desktop_height.clamp(500, 1200);
        self.sidebar_width = self.sidebar_width.clamp(320, 560);
        self.migrate_legacy_appearance();
        self.appearance_profiles.normalize();
        appearance::normalize_avatar_asset_id(&mut self.avatar_asset_id);
        self.display_name = self.display_name.trim().to_string();
        self.homepage_label = self.homepage_label.trim().to_string();
        self.homepage_url = self.homepage_url.trim().to_string();
    }
}

fn legacy_geometry_units_version() -> u8 {
    0
}

pub struct AppState {
    pub database: Database,
    settings: Mutex<ProductSettings>,
}

impl AppState {
    /// Reads the persisted rendering backend without building the full app state.
    ///
    /// The hosting backend has to be known *before* Tauri creates the config
    /// window and its WebView, while `AppState` is only constructed afterwards in
    /// the setup hook. Reading just this one field early is the smallest way to
    /// honour that ordering without moving the whole settings load, and it keeps
    /// a single source of truth: the same `product_settings` row the rest of the
    /// product already uses.
    ///
    /// Returns the default backend when the row is absent or unreadable, so a
    /// damaged or first-run database can never prevent startup.
    pub fn read_rendering_backend(data_dir: &std::path::Path) -> RenderingBackend {
        let Ok(database) = Database::open(data_dir.join("alan-desktop.sqlite3")) else {
            return RenderingBackend::default();
        };
        let Ok(Some(raw)) = database.setting(PRODUCT_SETTINGS_KEY) else {
            return RenderingBackend::default();
        };
        serde_json::from_str::<ProductSettings>(&raw)
            .map(|settings| settings.rendering_backend)
            .unwrap_or_default()
    }

    pub fn load(database: Database) -> Result<Self, String> {
        let mut settings = database
            .setting(PRODUCT_SETTINGS_KEY)?
            .map(|value| serde_json::from_str::<ProductSettings>(&value))
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        settings.normalize();
        let state = Self {
            database,
            settings: Mutex::new(settings),
        };
        state.persist()?;
        let profile = state.snapshot()?;
        if !profile.homepage_url.is_empty() {
            state.database.upsert_shortcut(&Shortcut {
                id: "homepage".into(),
                label: profile.homepage_label,
                url: profile.homepage_url,
                sort_order: 0,
                enabled: true,
            })?;
        }
        Ok(state)
    }

    pub fn snapshot(&self) -> Result<ProductSettings, String> {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .map_err(|_| "settings lock poisoned".into())
    }

    pub fn update(
        &self,
        update: impl FnOnce(&mut ProductSettings),
    ) -> Result<ProductSettings, String> {
        let snapshot = {
            let mut settings = self.settings.lock().map_err(|_| "settings lock poisoned")?;
            update(&mut settings);
            settings.normalize();
            settings.clone()
        };
        self.database.set_setting(
            PRODUCT_SETTINGS_KEY,
            &serde_json::to_string(&snapshot).map_err(|error| error.to_string())?,
        )?;
        Ok(snapshot)
    }

    pub fn persist(&self) -> Result<(), String> {
        let settings = self.snapshot()?;
        self.database.set_setting(
            PRODUCT_SETTINGS_KEY,
            &serde_json::to_string(&settings).map_err(|error| error.to_string())?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AppState, FloatingPresentation, ProductWindowMode, SidebarSide};
    use crate::appearance::BackgroundType;
    use crate::database::Database;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn defaults_to_floating_and_persists_updates() {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        assert_eq!(
            state.snapshot().expect("snapshot").mode,
            ProductWindowMode::Floating
        );
        state
            .update(|settings| {
                settings.mode = ProductWindowMode::Sidebar;
                settings.sidebar_side = SidebarSide::Right;
                settings.locked = true;
            })
            .expect("update");
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"mode\":\"sidebar\""));
        assert!(stored.contains("\"sidebarSide\":\"right\""));
        assert!(stored.contains("\"locked\":true"));
        assert!(stored.contains("\"desktopWidth\":420"));
        assert!(stored.contains("\"desktopHeight\":700"));
    }

    #[test]
    fn restores_window_state_after_database_reopen() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-settings-{}-{suffix}.sqlite3",
            std::process::id()
        ));

        {
            let state = AppState::load(Database::open(&path).expect("database")).expect("state");
            state
                .update(|settings| {
                    settings.mode = ProductWindowMode::Desktop;
                    settings.x = Some(240);
                    settings.y = Some(160);
                    settings.width = 760;
                    settings.height = 880;
                    settings.monitor_identity = Some("DISPLAY-2".into());
                    settings.desktop_x = Some(1440);
                    settings.desktop_y = Some(32);
                    settings.desktop_width = 420;
                    settings.desktop_height = 700;
                    settings.always_on_top = true;
                    settings.locked = true;
                    settings.display_name = "Example Person".into();
                    settings.homepage_label = "Example Site".into();
                    settings.homepage_url = "https://example.com/".into();
                })
                .expect("update");
        }

        let restored = AppState::load(Database::open(&path).expect("reopen"))
            .expect("state")
            .snapshot()
            .expect("snapshot");
        assert_eq!(restored.mode, ProductWindowMode::Desktop);
        assert_eq!((restored.x, restored.y), (Some(240), Some(160)));
        assert_eq!((restored.width, restored.height), (760, 880));
        assert_eq!(restored.monitor_identity.as_deref(), Some("DISPLAY-2"));
        assert_eq!(
            (restored.desktop_x, restored.desktop_y),
            (Some(1440), Some(32))
        );
        assert_eq!(
            (restored.desktop_width, restored.desktop_height),
            (420, 700)
        );
        assert!(restored.always_on_top);
        assert!(restored.locked);
        assert_eq!(restored.display_name, "Example Person");
        assert_eq!(restored.homepage_label, "Example Site");
        assert_eq!(restored.homepage_url, "https://example.com/");

        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[test]
    fn persists_each_supported_mode_transition_and_lock_state() {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        for (from, to) in [
            (ProductWindowMode::Floating, ProductWindowMode::Sidebar),
            (ProductWindowMode::Sidebar, ProductWindowMode::Floating),
            (ProductWindowMode::Floating, ProductWindowMode::Desktop),
            (ProductWindowMode::Desktop, ProductWindowMode::Floating),
        ] {
            state
                .update(|settings| settings.mode = from)
                .expect("prepare transition");
            let updated = state
                .update(|settings| settings.mode = to)
                .expect("persist transition");
            assert_eq!(updated.mode, to);
        }

        let locked = state
            .update(|settings| settings.locked = true)
            .expect("lock");
        assert!(locked.locked);
        assert!(state.snapshot().expect("snapshot").locked);
    }

    /// Pre-B4 databases have no `renderingBackend` key at all. They must load
    /// cleanly and land on the compatibility backend, because Enhanced is
    /// opt-in for v1: silently upgrading existing users to the backend without a
    /// UI Automation tree would remove screen-reader access on restart.
    #[test]
    fn settings_without_rendering_backend_migrate_to_standard() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","x":null,"y":null,"width":620,"height":720,"monitorIdentity":null,"sidebarSide":"left","sidebarWidth":380,"alwaysOnTop":false,"locked":false,"dayRollover":"04:00","weatherLocation":"","appearance":"geological_observatory"}"#,
            )
            .expect("legacy settings");
        let restored = AppState::load(database)
            .expect("compatible load")
            .snapshot()
            .expect("snapshot");
        assert_eq!(restored.rendering_backend, super::RenderingBackend::Standard);
        assert!(!restored.rendering_backend.uses_composition_hosting());
    }

    #[test]
    fn rendering_backend_round_trips_and_parses_from_str() {
        use super::RenderingBackend;
        assert_eq!(RenderingBackend::default(), RenderingBackend::Standard);
        assert_eq!(
            RenderingBackend::try_from("standard").expect("standard"),
            RenderingBackend::Standard
        );
        assert_eq!(
            RenderingBackend::try_from("enhanced").expect("enhanced"),
            RenderingBackend::Enhanced
        );
        assert!(RenderingBackend::try_from("composition").is_err());
        assert!(RenderingBackend::Enhanced.uses_composition_hosting());
        assert!(!RenderingBackend::Standard.uses_composition_hosting());

        let json = serde_json::to_string(&RenderingBackend::Enhanced).expect("serialize");
        assert_eq!(json, "\"enhanced\"");
        assert_eq!(
            serde_json::from_str::<RenderingBackend>("\"standard\"").expect("deserialize"),
            RenderingBackend::Standard
        );
    }

    /// The language choice has to survive a restart, and a settings document
    /// written before the key existed has to keep loading.
    #[test]
    fn language_round_trips_and_legacy_documents_default_to_system() {
        use crate::locale::Language;

        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        assert_eq!(state.snapshot().expect("snapshot").language, Language::System);
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"language\":\"system\""));

        state
            .update(|settings| settings.language = Language::SimplifiedChinese)
            .expect("persist language");

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-language-{}-{suffix}.sqlite3",
            std::process::id()
        ));
        {
            let file_state =
                AppState::load(Database::open(&path).expect("database")).expect("state");
            file_state
                .update(|settings| settings.language = Language::SimplifiedChinese)
                .expect("persist language");
        }
        let reopened =
            AppState::load(Database::open(&path).expect("database")).expect("reopened state");
        assert_eq!(
            reopened.snapshot().expect("snapshot").language,
            Language::SimplifiedChinese
        );

        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","dayRollover":"04:00","appearance":"geological_observatory"}"#,
            )
            .expect("legacy settings");
        let legacy = AppState::load(database)
            .expect("compatible load")
            .snapshot()
            .expect("snapshot");
        assert_eq!(legacy.language, Language::System);
    }

    #[test]
    fn phase_two_settings_json_loads_with_open_source_profile_defaults() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","x":null,"y":null,"width":620,"height":720,"monitorIdentity":null,"sidebarSide":"left","sidebarWidth":380,"alwaysOnTop":false,"locked":false,"dayRollover":"04:00","weatherLocation":"","appearance":"geological_observatory"}"#,
            )
            .expect("legacy settings");
        let restored = AppState::load(database)
            .expect("compatible load")
            .snapshot()
            .expect("snapshot");
        assert_eq!(restored.display_name, "Your Name");
        assert_eq!(restored.homepage_label, "Homepage");
        assert!(restored.homepage_url.is_empty());
        assert!(restored.weather_location_label.is_empty());
        assert_eq!(restored.temperature_unit, super::TemperatureUnit::Celsius);
        assert_eq!(
            restored.floating_presentation,
            FloatingPresentation::Collapsed
        );
        assert_eq!(
            restored.appearance_profiles.floating.background_type,
            BackgroundType::Glass
        );
        assert!(restored.avatar_asset_id.is_none());
        assert_eq!(restored.geometry_units_version, 0);
        assert_eq!((restored.desktop_x, restored.desktop_y), (None, None));
        assert_eq!(
            (restored.desktop_width, restored.desktop_height),
            (420, 700)
        );
    }

    /// The upgrade path for users whose document predates per-mode profiles.
    ///
    /// One `appearanceSettings` value has to become three identical profiles, or
    /// an existing user's window would silently change material on the first
    /// launch after the update. The legacy key must also stop being written, so
    /// the migrated document has exactly one representation.
    #[test]
    fn legacy_single_appearance_settings_fans_out_to_every_profile_and_is_not_rewritten() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r##"{"mode":"desktop","appearance":"geological_observatory","appearanceSettings":{"backgroundType":"solid","solidColor":"#123456","glassTintOpacity":0.4,"blurPx":6,"backgroundOpacity":0.5,"textContrast":"dark","customTextColor":"#abcdef"}}"##,
            )
            .expect("legacy settings");

        let state = AppState::load(database).expect("compatible load");
        let restored = state.snapshot().expect("snapshot");
        for (mode, profile) in [
            ("sidebar", &restored.appearance_profiles.sidebar),
            ("floating", &restored.appearance_profiles.floating),
            ("desktop", &restored.appearance_profiles.desktop),
        ] {
            assert_eq!(
                profile.background_type,
                BackgroundType::Solid,
                "{mode} must inherit the legacy appearance"
            );
            assert_eq!(profile.solid_color, "#123456", "{mode}");
            assert_eq!(profile.glass_tint_opacity, 0.4, "{mode}");
            assert_eq!(profile.blur_px, 6.0, "{mode}");
            assert_eq!(profile.background_opacity, 0.5, "{mode}");
            assert_eq!(profile.text_contrast, crate::appearance::TextContrast::Dark);
            assert_eq!(profile.custom_text_color, "#abcdef", "{mode}");
            // Fields the legacy document never had still take their defaults.
            assert_eq!(profile.overlay_strength, 0.18, "{mode}");
        }

        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"appearanceProfiles\""));
        assert!(stored.contains("\"desktop\":{\"backgroundType\":\"solid\""));
        assert!(
            !stored.contains("\"appearanceSettings\""),
            "the bridge field must not be written back"
        );
    }

    /// A document that predates Appearance entirely keeps the documented
    /// defaults in all three profiles.
    #[test]
    fn document_without_any_appearance_still_resolves_three_default_profiles() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"sidebar","appearance":"geological_observatory"}"#,
            )
            .expect("legacy settings");
        let restored = AppState::load(database)
            .expect("compatible load")
            .snapshot()
            .expect("snapshot");
        assert_eq!(
            restored.appearance_profiles.sidebar,
            crate::appearance::AppearanceSettings::default()
        );
        assert_eq!(
            restored.appearance_profiles.floating,
            crate::appearance::AppearanceSettings::default()
        );
        assert_eq!(
            restored.appearance_profiles.desktop,
            crate::appearance::AppearanceSettings::default()
        );
    }

    /// The three profiles have to survive a restart independently, and the mode
    /// the app is in has to select its own.
    #[test]
    fn per_mode_appearance_profiles_persist_independently() {
        use crate::appearance::{AppearanceSettings, BackgroundType};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-appearance-{}-{suffix}.sqlite3",
            std::process::id()
        ));

        {
            let state = AppState::load(Database::open(&path).expect("database")).expect("state");
            state
                .update(|settings| {
                    settings.appearance_profiles.sidebar = AppearanceSettings {
                        background_type: BackgroundType::Solid,
                        solid_color: "#101112".into(),
                        text_contrast: crate::appearance::TextContrast::Custom,
                        custom_text_color: "#ffcc00".into(),
                        ..AppearanceSettings::default()
                    };
                    settings.appearance_profiles.floating = AppearanceSettings {
                        background_type: BackgroundType::Gradient,
                        gradient_angle: 20.0,
                        ..AppearanceSettings::default()
                    };
                    settings.appearance_profiles.desktop = AppearanceSettings {
                        background_type: BackgroundType::Wallpaper,
                        overlay_strength: 0.6,
                        ..AppearanceSettings::default()
                    };
                })
                .expect("persist profiles");
        }

        let restored = AppState::load(Database::open(&path).expect("reopen")).expect("state");
        let snapshot = restored.snapshot().expect("snapshot");
        assert_eq!(snapshot.appearance_profiles.sidebar.solid_color, "#101112");
        assert_eq!(
            snapshot.appearance_profiles.sidebar.custom_text_color,
            "#ffcc00"
        );
        assert_eq!(snapshot.appearance_profiles.floating.gradient_angle, 20.0);
        assert_eq!(
            snapshot.appearance_profiles.desktop.background_type,
            BackgroundType::Wallpaper
        );
        assert_eq!(snapshot.appearance_profiles.desktop.overlay_strength, 0.6);
        // Editing Desktop must not have touched the other two.
        assert_eq!(snapshot.appearance_profiles.floating.overlay_strength, 0.18);

        restored
            .update(|settings| settings.mode = ProductWindowMode::Desktop)
            .expect("switch mode");
        let desktop = restored.snapshot().expect("snapshot");
        assert_eq!(
            desktop.active_appearance().background_type,
            BackgroundType::Wallpaper
        );
        restored
            .update(|settings| settings.mode = ProductWindowMode::Sidebar)
            .expect("switch back");
        let sidebar = restored.snapshot().expect("snapshot");
        assert_eq!(
            sidebar.active_appearance().background_type,
            BackgroundType::Solid
        );

        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[test]
    fn default_configuration_has_no_personal_runtime_dependency() {
        let settings = super::ProductSettings::default();
        let serialized = serde_json::to_string(&settings).expect("serialize");
        assert!(!serialized.to_ascii_lowercase().contains("alanfloyd.net"));
        assert!(settings.weather_location_label.is_empty());
        assert!(settings.weather_latitude.is_none());
        assert!(settings.homepage_url.is_empty());
        assert_eq!(settings.display_name, "Your Name");
        assert_eq!(
            settings.floating_presentation,
            FloatingPresentation::Collapsed
        );
        assert_eq!(
            settings.appearance_profiles.floating.background_type,
            BackgroundType::Glass
        );
    }
}
