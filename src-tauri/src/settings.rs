use crate::{
    appearance::{self, AppearanceProfiles, AppearanceSettings},
    database::Database,
    locale::Language,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const PRODUCT_SETTINGS_KEY: &str = "product_settings";

/// Persisted name used when a legacy homepage had no label of its own.
///
/// Stable, untranslated data: it is stored in the settings document, so
/// translating it would rewrite the user's own label on a language change.
pub const DEFAULT_QUICK_LINK_NAME: &str = "Homepage";

/// Stable identifier the legacy homepage migration reuses.
///
/// Deterministic rather than a fresh UUID so the migration is idempotent and
/// reload-safe: if it ever ran twice, the second run would produce the same id
/// instead of a duplicate row. [`ProductSettings::migrate_legacy_homepage`] is
/// also guarded on an empty `quick_links`, so this is belt and braces.
pub const LEGACY_HOMEPAGE_LINK_ID: &str = "legacy-homepage";

/// One user-managed Quick Link.
///
/// `id` is the identity used by edit/delete/reorder. It is deliberately not the
/// array index: reordering must not re-key anything, and a future sync or import
/// needs a stable handle. Names and URLs are user data and are never localized.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickLink {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// The only URL shapes a Quick Link may hold: `http` or `https` with a host.
///
/// One implementation for both the write path (saving settings) and the open path
/// ([`crate::product_window::open_quick_link`]), so a link that cannot be stored
/// can never be launched and vice versa. Everything else — `javascript:`,
/// `file:`, `data:`, `shell:`, a bare host, a relative path — is rejected rather
/// than normalized, because the product has no legitimate use for those schemes
/// and guessing would be a security decision made on the user's behalf.
pub fn validate_quick_link_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "link URL must be a valid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("link URL must start with http:// or https://".into());
    }
    if parsed.host().is_none() {
        return Err("link URL must include a host".into());
    }
    Ok(())
}

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
    /// The product's Quick Links, in display order.
    ///
    /// Persisted here rather than in the `shortcuts` table because that table is
    /// documented as a reserved-ID projection (`docs/data-model.md`) and this list
    /// needs stable ids, ordering, and per-link names — the settings document is
    /// the single source of truth for both the product surface and the manager UI.
    /// An empty list is valid and means the product section is hidden.
    #[serde(default)]
    pub quick_links: Vec<QuickLink>,
    /// Deserialize-only bridge for the single footer homepage.
    ///
    /// Documents written before Quick Links stored one label/URL pair. Both still
    /// parse into these fields, and [`ProductSettings::migrate_legacy_homepage`]
    /// turns the pair into exactly one Quick Link so an existing user's entry
    /// survives the upgrade. Neither field is serialized any more, so the
    /// migration runs once and the written document carries `quickLinks` only.
    #[serde(default, skip_serializing)]
    pub homepage_label: String,
    #[serde(default, skip_serializing)]
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
            // Neutral, non-personal default. The footer/Orb identity derives its
            // initials from this value (`User` -> `U`), so the affordance is
            // discoverable on a fresh install without shipping anyone's identity.
            // `default_profile_identity_is_neutral` asserts it.
            display_name: "User".into(),
            avatar_asset_id: None,
            // Neutral by default: the open-source build ships no personal site and
            // no pre-seeded links. `default_configuration_has_no_personal_runtime_dependency`
            // asserts this.
            quick_links: Vec::new(),
            homepage_label: DEFAULT_QUICK_LINK_NAME.into(),
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

    /// Turns a pre-Quick-Links document's single homepage into one Quick Link.
    ///
    /// Runs on load, before the state is persisted, so the upgraded document is
    /// written with `quickLinks` and without the legacy pair.
    ///
    /// Guarded on an empty list: `normalize` may run many times over the life of a
    /// document, so an unconditional conversion would re-append the homepage to a
    /// list the user had deliberately emptied. For the same reason a legacy entry
    /// whose URL is missing or unusable is dropped rather than stored — the open
    /// path would refuse it anyway, and a Quick Link that cannot be clicked is
    /// worse than no Quick Link.
    ///
    /// A legacy label is preserved verbatim, including its language: it is the
    /// user's own text, so nothing here renames or translates it.
    fn migrate_legacy_homepage(&mut self) {
        let url = self.homepage_url.trim().to_string();
        if !self.quick_links.is_empty() || url.is_empty() {
            return;
        }
        if validate_quick_link_url(&url).is_err() {
            return;
        }
        let name = self.homepage_label.trim();
        self.quick_links.push(QuickLink {
            id: LEGACY_HOMEPAGE_LINK_ID.into(),
            name: if name.is_empty() {
                DEFAULT_QUICK_LINK_NAME.into()
            } else {
                name.to_string()
            },
            url,
        });
    }

    /// Drops entries this build cannot render or open.
    ///
    /// A link with no name would be an unlabelled row and a link with an
    /// unusable URL cannot be launched, so neither is kept. Applied on every
    /// normalize so a hand-edited or future-version document degrades to a valid
    /// list instead of failing to load.
    fn normalize_quick_links(&mut self) {
        self.quick_links.retain_mut(|link| {
            link.id = link.id.trim().to_string();
            link.name = link.name.trim().to_string();
            link.url = link.url.trim().to_string();
            !link.name.is_empty()
                && !link.id.is_empty()
                && validate_quick_link_url(&link.url).is_ok()
        });
    }

    pub fn normalize(&mut self) {
        self.width = self.width.clamp(360, 1100);
        self.height = self.height.clamp(500, 1200);
        self.desktop_width = self.desktop_width.clamp(360, 1100);
        self.desktop_height = self.desktop_height.clamp(500, 1200);
        self.sidebar_width = self.sidebar_width.clamp(320, 560);
        self.migrate_legacy_appearance();
        self.migrate_legacy_homepage();
        self.normalize_quick_links();
        self.appearance_profiles.normalize();
        appearance::normalize_avatar_asset_id(&mut self.avatar_asset_id);
        self.display_name = self.display_name.trim().to_string();
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
    use super::{AppState, FloatingPresentation, ProductWindowMode, QuickLink, SidebarSide};
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
                    settings.quick_links = vec![
                        QuickLink {
                            id: "link-a".into(),
                            name: "Example Site".into(),
                            url: "https://example.com/".into(),
                        },
                        QuickLink {
                            id: "link-b".into(),
                            name: "Example Site".into(),
                            url: "https://example.com/".into(),
                        },
                    ];
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
        // Order and ids survive the round trip; duplicate names and duplicate
        // URLs are allowed and are not collapsed.
        assert_eq!(restored.quick_links.len(), 2);
        assert_eq!(restored.quick_links[0].id, "link-a");
        assert_eq!(restored.quick_links[1].id, "link-b");
        assert_eq!(restored.quick_links[0].name, restored.quick_links[1].name);
        assert_eq!(restored.quick_links[0].url, restored.quick_links[1].url);

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
        assert_eq!(restored.display_name, "User");
        // A pre-Quick-Links document with no homepage configured yields an empty
        // list: there is nothing to migrate. The legacy pair is no longer part of
        // the document, so absent keys resolve to empty here rather than to the
        // old in-memory "Homepage" placeholder.
        assert!(restored.quick_links.is_empty());
        assert!(restored.homepage_label.is_empty());
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
        // Nor any personal profile identity: no name, no initials, no avatar.
        let lowered = serialized.to_ascii_lowercase();
        for personal in ["alan", "floyd", "your name"] {
            assert!(
                !lowered.contains(personal),
                "the default document still ships {personal:?}"
            );
        }
        assert!(settings.weather_location_label.is_empty());
        assert!(settings.weather_latitude.is_none());
        // v1 ships no Quick Links: the product section is hidden until the user
        // adds one, and the open-source default stays neutral.
        assert!(settings.quick_links.is_empty());
        assert!(settings.homepage_url.is_empty());
        assert_eq!(settings.display_name, "User");
        assert!(settings.avatar_asset_id.is_none());
        assert_eq!(
            settings.floating_presentation,
            FloatingPresentation::Collapsed
        );
        assert_eq!(
            settings.appearance_profiles.floating.background_type,
            BackgroundType::Glass
        );
    }

    /// Initials are derived from the display name, with no built-in identity.
    ///
    /// The dangerous regression is a shipped fallback mark (the removed `AD`) that
    /// reappears whenever the name is empty, so an unconfigured user silently gets
    /// someone else's initials. This pins the empty-input rule against the source.
    /// The non-empty rules (`User` -> `U`, `Alan Floyd` -> `AF`, CJK, multi-word)
    /// are verified by running the real function — see
    /// `scripts/rc0-qa/profile-initials.mjs`.
    #[test]
    fn profile_initials_have_no_builtin_fallback_identity() {
        let source = include_str!("../../src/appearance.ts");
        let start = source
            .find("export function profileInitials")
            .expect("profileInitials");
        let body = &source[start..];
        let end = body.find("\n}").expect("profileInitials end");
        let body = &body[..end];

        assert!(
            body.contains("if (!words.length) return \"\";"),
            "an empty display name must yield no initials, not a shipped mark"
        );
        // No non-empty string literal may be returned as a fallback: every returned
        // value has to be derived from the name the user typed.
        for line in body.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("return ") else {
                continue;
            };
            let literal = rest.trim_end_matches(';').trim();
            let is_derived = literal.contains("first") || literal.contains("last");
            assert!(
                is_derived || literal == "\"\"",
                "profileInitials returns a literal identity: {literal}"
            );
        }
    }

    /// The footer identity is user content: absent when unconfigured, while the
    /// Floating collapse control stays reachable regardless.
    ///
    /// Pins the rules that are easy to regress by "helpfully" filling a blank: the
    /// identity block is gated on having an identity at all, no fallback label or
    /// invented initials may be rendered when there is none, and collapsing — a
    /// window action, not profile content — is still offered when the identity is
    /// absent, through the same event.
    #[test]
    fn footer_identity_is_gated_and_collapse_stays_reachable() {
        let source = include_str!("../../src/components/ProductContent.vue");
        let start = source
            .find("const hasIdentity")
            .expect("hasIdentity computed");
        let condition = &source[start..start + 200];
        assert!(
            condition.contains("avatarAvailable") && condition.contains("displayName.trim().length > 0"),
            "the identity block must require an avatar or a non-blank name"
        );

        let footer_start = source.find("<footer class=\"signature\">").expect("footer");
        let footer = &source[footer_start..];
        assert!(
            footer.contains("<span v-else-if=\"hasIdentity\" class=\"profile-identity\">"),
            "the non-Floating identity must be gated on hasIdentity"
        );
        assert!(
            !footer.contains("footer.defaultDisplayName"),
            "the footer must not fall back to a default display name"
        );

        // Floating always offers collapse: the identity button when there is an
        // identity, a standalone control when there is not.
        assert!(
            footer.contains("class=\"profile-identity collapse-affordance\""),
            "the clickable identity block must stay the collapse affordance"
        );
        assert!(
            footer.contains("class=\"collapse-affordance collapse-affordance-standalone\""),
            "a standalone collapse control must exist for the no-identity case"
        );
        assert_eq!(
            footer.matches("@click=\"$emit('collapse')\"").count(),
            2,
            "both collapse controls must emit the one collapse event"
        );

        // The fallback key is gone from the catalogs as well: it must not come back
        // in one language only.
        let catalog = include_str!("../../src/i18n/catalog.ts");
        assert!(
            !catalog.contains("footer.defaultDisplayName"),
            "the defensive display-name fallback should not be declared"
        );
    }

    /// The collapse label is the one the product asks for, in both languages.
    ///
    /// The label lives in four places (frontend footer + menu catalog, native
    /// menu), and a rename that misses one would leave two names for one action.
    #[test]
    fn collapse_label_is_collapse_to_orb_everywhere() {
        let catalog = include_str!("../../src/i18n/catalog.ts");
        assert!(catalog.contains("\"footer.collapseToOrb\": \"Collapse to Orb\""));
        assert!(catalog.contains("\"menu.collapseFloating\": \"Collapse to Orb\""));
        assert!(catalog.contains("\"footer.collapseToOrb\": \"折叠为悬浮球\""));
        assert!(catalog.contains("\"menu.collapseFloating\": \"折叠为悬浮球\""));
        assert!(
            !catalog.contains("Collapse to Avatar Orb") && !catalog.contains("收起到头像球"),
            "the old collapse wording must not remain"
        );

        let native = include_str!("locale.rs");
        assert!(native.contains("collapse_floating: \"Collapse to Orb\""));
        assert!(native.contains("collapse_floating: \"折叠为悬浮球\""));
    }

    /// The shipped profile identity is a neutral placeholder, not a person.
    ///
    /// The frontend derives its initials from this value, so the default name is
    /// the only place the neutral `U` comes from; a personal literal here would
    /// ship as every new user's profile.
    #[test]
    fn default_profile_identity_is_neutral() {
        let settings = super::ProductSettings::default();
        assert_eq!(settings.display_name, "User");
        assert!(settings.avatar_asset_id.is_none());

        // A user's own name survives load and normalize untouched.
        let database = Database::in_memory().expect("database");
        database
            .set_setting("product_settings", r#"{"displayName":"Alan Floyd"}"#)
            .expect("settings");
        let restored = AppState::load(database).expect("load").snapshot().expect("snapshot");
        assert_eq!(restored.display_name, "Alan Floyd");

        // An explicitly empty name is preserved as empty, so a user who clears the
        // field is not silently handed the placeholder back.
        let database = Database::in_memory().expect("database");
        database
            .set_setting("product_settings", r#"{"displayName":"   "}"#)
            .expect("settings");
        let restored = AppState::load(database).expect("load").snapshot().expect("snapshot");
        assert!(restored.display_name.is_empty());
    }

    #[test]
    fn quick_links_default_shape_is_an_empty_list() {
        let settings = super::ProductSettings::default();
        // camelCase on the wire, and an empty list is serialized rather than omitted
        // so a reader never has to distinguish "absent" from "empty".
        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(json["quickLinks"], serde_json::json!([]));
        assert!(json.get("homepageLabel").is_none());
        assert!(json.get("homepageUrl").is_none());

        let parsed: super::ProductSettings =
            serde_json::from_value(serde_json::json!({ "quickLinks": [] })).expect("parse");
        assert!(parsed.quick_links.is_empty());
    }

    #[test]
    fn quick_links_round_trip_with_camel_case_keys() {
        let mut settings = super::ProductSettings::default();
        settings.quick_links = vec![
            super::QuickLink {
                id: "b".into(),
                name: "Docs".into(),
                url: "https://example.com/docs".into(),
            },
            super::QuickLink {
                id: "a".into(),
                name: "Docs".into(),
                url: "https://example.com/docs".into(),
            },
        ];
        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(
            json["quickLinks"],
            serde_json::json!([
                { "id": "b", "name": "Docs", "url": "https://example.com/docs" },
                { "id": "a", "name": "Docs", "url": "https://example.com/docs" },
            ])
        );
        let parsed: super::ProductSettings = serde_json::from_value(json).expect("parse");
        // Array order is the display order and is preserved verbatim.
        assert_eq!(parsed.quick_links, settings.quick_links);
    }

    #[test]
    fn legacy_homepage_migrates_to_one_quick_link_once() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"displayName":"Example Person","homepageLabel":"Personal Site","homepageUrl":"https://www.example.net","mode":"floating"}"#,
            )
            .expect("legacy settings");

        let state = AppState::load(database).expect("load");
        let migrated = state.snapshot().expect("snapshot");
        assert_eq!(
            migrated.quick_links,
            vec![super::QuickLink {
                id: super::LEGACY_HOMEPAGE_LINK_ID.into(),
                // The user's own label is kept verbatim, language included.
                name: "Personal Site".into(),
                url: "https://www.example.net".into(),
            }]
        );

        // The written document drops the legacy pair, so loading it again must not
        // append a second link.
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(!stored.contains("homepageUrl"));
        let reloaded = AppState::load(state.database).expect("reload").snapshot().expect("snapshot");
        assert_eq!(reloaded.quick_links.len(), 1);
    }

    #[test]
    fn legacy_homepage_without_a_label_gets_the_neutral_name() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"homepageLabel":"","homepageUrl":"https://example.com"}"#,
            )
            .expect("legacy settings");
        let migrated = AppState::load(database).expect("load").snapshot().expect("snapshot");
        assert_eq!(migrated.quick_links.len(), 1);
        assert_eq!(migrated.quick_links[0].name, super::DEFAULT_QUICK_LINK_NAME);
    }

    #[test]
    fn legacy_homepage_with_no_url_migrates_to_nothing() {
        // The pre-Quick-Links default: a "Homepage" label and no URL. That is a
        // default, not user configuration, so it must not become a link.
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"homepageLabel":"Homepage","homepageUrl":""}"#,
            )
            .expect("legacy settings");
        let migrated = AppState::load(database).expect("load").snapshot().expect("snapshot");
        assert!(migrated.quick_links.is_empty());
    }

    #[test]
    fn legacy_homepage_with_an_unusable_url_is_not_migrated() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"homepageLabel":"Something","homepageUrl":"javascript:alert(1)"}"#,
            )
            .expect("legacy settings");
        let migrated = AppState::load(database).expect("load").snapshot().expect("snapshot");
        assert!(migrated.quick_links.is_empty());
    }

    #[test]
    fn an_existing_list_is_never_re_seeded_from_the_legacy_pair() {
        // A document that already has links keeps exactly those: the legacy pair is
        // read for migration only, and a user who emptied their list stays empty.
        let mut settings = super::ProductSettings {
            homepage_label: "Personal Site".into(),
            homepage_url: "https://www.example.net".into(),
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.quick_links.len(), 1);

        // Emptied by the user, with the legacy pair still present in the document.
        settings.quick_links.clear();
        settings.normalize_quick_links();
        assert!(settings.quick_links.is_empty());

        settings.quick_links = vec![super::QuickLink {
            id: "kept".into(),
            name: "Kept".into(),
            url: "https://example.com/kept".into(),
        }];
        settings.normalize();
        assert_eq!(settings.quick_links.len(), 1);
        assert_eq!(settings.quick_links[0].id, "kept");
    }

    #[test]
    fn normalizing_drops_unusable_quick_links_and_trims_the_rest() {
        let mut settings = super::ProductSettings {
            quick_links: vec![
                super::QuickLink {
                    id: "  keep  ".into(),
                    name: "  Docs  ".into(),
                    url: "  https://example.com/docs  ".into(),
                },
                super::QuickLink {
                    id: "no-name".into(),
                    name: "   ".into(),
                    url: "https://example.com".into(),
                },
                super::QuickLink {
                    id: "bad-url".into(),
                    name: "Bad".into(),
                    url: "file:///C:/private.txt".into(),
                },
                super::QuickLink {
                    id: String::new(),
                    name: "No id".into(),
                    url: "https://example.com".into(),
                },
            ],
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(
            settings.quick_links,
            vec![super::QuickLink {
                id: "keep".into(),
                name: "Docs".into(),
                url: "https://example.com/docs".into(),
            }]
        );
    }

    #[test]
    fn url_validation_accepts_only_http_and_https_with_a_host() {
        for accepted in [
            "https://example.com/",
            "http://localhost:3000/",
            "https://example.com/path?query=1#fragment",
        ] {
            assert!(
                super::validate_quick_link_url(accepted).is_ok(),
                "{accepted} should be accepted"
            );
        }
        for rejected in [
            "",
            "example.com",
            "/relative/path",
            "javascript:alert(1)",
            "file:///C:/private.txt",
            "data:text/html,<h1>x</h1>",
            "shell:startup",
            "mailto:someone@example.com",
            "ftp://example.com/file",
            "https://",
            "https:// example.com",
        ] {
            assert!(
                super::validate_quick_link_url(rejected).is_err(),
                "{rejected} should be rejected"
            );
        }
        assert!(super::validate_quick_link_url("https://example.com/").is_ok());
    }
}
