use crate::{
    appearance::{self, AppearanceProfiles, AppearanceSettings, BackgroundType},
    database::Database,
    locale::Language,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex};

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
    /// Unknown members written by a future version of this evolvable
    /// document. Round-tripped verbatim so a rollback to this runtime can
    /// never silently delete a newer link shape (the Phase 2A settings
    /// unknown-key contract). Never consulted by this build's logic.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl QuickLink {
    /// A link carrying no future-version extras; the normal in-process way to
    /// build one. Only a deserialized document can populate `extra`.
    pub fn new(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            url: url.into(),
            extra: BTreeMap::new(),
        }
    }
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
    /// Unknown members written by a future version of this settings document.
    ///
    /// `ProductSettings` is the product's primary evolvable persistence
    /// surface: every release rewrites the whole document on load
    /// (`AppState::load` persists after `normalize`), so serde's default
    /// "ignore unknown keys" behavior would make any rollback runtime
    /// permanently delete members a newer version had stored. Flattening this
    /// map round-trips them instead — the Phase 2A frozen settings
    /// unknown-key contract (docs/application-lifecycle.md §10).
    ///
    /// Semantics: a known key is always consumed by its typed field and never
    /// lands here, so unknown members cannot override typed values and cannot
    /// duplicate known serialized output; `normalize` never touches this map;
    /// equality includes it, which is the honest round-trip semantic. Keys
    /// consumed by a documented legacy migration (`homepageLabel`,
    /// `homepageUrl`, `appearanceSettings`) are deliberately not preserved:
    /// they are migrated into their current representation instead.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
            extra: BTreeMap::new(),
        }
    }
}

impl ProductSettings {
    /// The settings document a brand-new profile starts from.
    ///
    /// This is *not* [`Default`]: it is used only when no `product_settings` row
    /// exists at all, i.e. a first run on a machine that has never stored this
    /// profile. It differs from the historical defaults in exactly two ways:
    ///
    /// * the Floating presentation starts **expanded**, so a first run shows the
    ///   widget — date line, tasks, footer, and the visible Settings affordance —
    ///   instead of a 56 DIP Orb in a screen corner that is easy to read as
    ///   "nothing opened";
    /// * the material starts on **Gradient** rather than Glass.
    ///
    /// The material change is a readability decision for the default configuration
    /// only. A fresh profile also starts on the Standard rendering backend, which
    /// has no native Acrylic, so Glass there is a translucent tint over whatever
    /// the wallpaper happens to be: over busy or high-contrast wallpapers the
    /// widget's own hierarchy competes with the backdrop. The Gradient material
    /// keeps the same translucent graphite language — the documented palette, not
    /// a new one — while giving the surface a defined shape that reads the same on
    /// dark, light, saturated, and textured wallpapers.
    ///
    /// Everything else, including the rendering backend and the Gradient values
    /// themselves, is the documented default. Because this function is reachable
    /// only from the missing-row branch of [`AppState::load`], an existing
    /// persisted profile can never be changed by it: a stored Glass profile stays
    /// Glass, and a document written before these keys existed keeps resolving
    /// through the serde default in [`ProductSettings::default`].
    fn for_fresh_profile() -> Self {
        Self {
            floating_presentation: FloatingPresentation::Expanded,
            appearance_profiles: AppearanceProfiles::from_all(fresh_profile_appearance()),
            ..Self::default()
        }
    }

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
        self.quick_links.push(QuickLink::new(
            LEGACY_HOMEPAGE_LINK_ID,
            if name.is_empty() {
                DEFAULT_QUICK_LINK_NAME
            } else {
                name
            },
            url,
        ));
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
        // The Enhanced hosting path is retired (standard-only; `run()` in lib.rs
        // pins the runtime decision to Standard). Fold a persisted `enhanced`
        // here so the startup `load -> normalize -> persist` flow rewrites old
        // documents with the persist call that already happens — one fold, no
        // separate migration and no extra DB write. The variant itself stays
        // parseable: a document written by an older build must keep loading
        // until this fold has rewritten it.
        self.rendering_backend = RenderingBackend::Standard;
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

/// The material a brand-new profile starts from.
///
/// Gradient with the project's own documented palette (`#11191e` → `#213747` at
/// 135°, the values the Appearance controls already ship) and the documented
/// background opacity, so the fresh surface stays translucent instead of becoming
/// a heavy rectangle. No new colours are introduced for this.
fn fresh_profile_appearance() -> AppearanceSettings {
    AppearanceSettings {
        background_type: BackgroundType::Gradient,
        ..AppearanceSettings::default()
    }
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
    /// "Legitimately absent" is not "broken": a missing row is a first run and
    /// resolves to the default backend. A database that cannot be opened, a
    /// failing settings query, or an unparseable stored document is a real
    /// failure and is returned as `Err` — it must never be disguised as the
    /// user having chosen Standard, and startup treats it as fatal through the
    /// controlled failure path (never as a panic or a half-initialized run).
    pub fn read_rendering_backend(data_dir: &std::path::Path) -> Result<RenderingBackend, String> {
        let database = Database::open(data_dir.join("alan-desktop.sqlite3"))?;
        match database.setting(PRODUCT_SETTINGS_KEY)? {
            None => Ok(RenderingBackend::default()),
            Some(raw) => serde_json::from_str::<ProductSettings>(&raw)
                .map(|settings| settings.rendering_backend)
                .map_err(|error| format!("stored settings document is unreadable: {error}")),
        }
    }

    pub fn load(database: Database) -> Result<Self, String> {
        let mut settings = match database.setting(PRODUCT_SETTINGS_KEY)? {
            Some(value) => serde_json::from_str::<ProductSettings>(&value)
                .map_err(|error| error.to_string())?,
            // No document yet: a first run on this machine, so the fresh-profile
            // defaults apply. A stored document is never re-defaulted — it is
            // parsed and normalized as-is, which is what keeps every existing
            // profile byte-for-byte compatible across an upgrade.
            None => ProductSettings::for_fresh_profile(),
        };
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

    /// Applies one settings change as a single serialized memory + durable
    /// transaction.
    ///
    /// The settings Mutex is held across the whole sequence — clone candidate,
    /// mutate, normalize, serialize, **persist** — and the live state is
    /// committed only after the durable write succeeds. This gives two
    /// guarantees the pre-hardening shape (mutate in place, release the lock,
    /// then persist) could not:
    ///
    /// * a persist failure leaves both memory and disk at the pre-call state
    ///   (never memory=new / disk=old);
    /// * a second concurrent update cannot durably commit out of order or
    ///   overtake the first one, because it cannot even reach its own persist
    ///   until the first update has released the lock — which happens only
    ///   after its durable commit landed. Lock order is always
    ///   settings → database: no production path acquires the connection lock
    ///   and then the settings lock, so the wider critical section cannot
    ///   deadlock.
    pub fn update(
        &self,
        update: impl FnOnce(&mut ProductSettings),
    ) -> Result<ProductSettings, String> {
        let mut settings = self.settings.lock().map_err(|_| "settings lock poisoned")?;
        let mut candidate = settings.clone();
        update(&mut candidate);
        candidate.normalize();
        let document = serde_json::to_string(&candidate).map_err(|error| error.to_string())?;
        self.database.set_setting(PRODUCT_SETTINGS_KEY, &document)?;
        *settings = candidate;
        Ok(settings.clone())
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
    use crate::appearance::{AppearanceSettings, BackgroundType};
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
                        QuickLink::new("link-a", "Example Site", "https://example.com/"),
                        QuickLink::new("link-b", "Example Site", "https://example.com/"),
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

    /// The Enhanced hosting path is retired, but documents stored by older
    /// builds may still say `renderingBackend: "enhanced"`. Such a document
    /// must load, land on Standard through normalize, and be rewritten as
    /// `"standard"` by the persist that `AppState::load` already performs —
    /// while every other field of the document survives unchanged.
    #[test]
    fn stored_enhanced_backend_normalizes_to_standard_and_persists() {
        use super::RenderingBackend;

        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"sidebar","renderingBackend":"enhanced","x":12,"y":-4,"width":640,"height":700,"monitorIdentity":null,"sidebarSide":"right","sidebarWidth":420,"dayRollover":"02:30","weatherLocation":"","appearance":"geological_observatory"}"#,
            )
            .expect("legacy enhanced settings");
        let state = AppState::load(database).expect("load");
        let restored = state.snapshot().expect("snapshot");
        assert_eq!(restored.rendering_backend, RenderingBackend::Standard);
        // The fold only touches the backend; the rest of the document is intact.
        assert_eq!(restored.mode, ProductWindowMode::Sidebar);
        assert_eq!(restored.x, Some(12));
        assert_eq!(restored.y, Some(-4));
        assert_eq!(restored.width, 640);
        assert_eq!(restored.height, 700);
        assert_eq!(restored.sidebar_side, SidebarSide::Right);
        assert_eq!(restored.sidebar_width, 420);
        assert_eq!(restored.day_rollover, "02:30");

        // The startup persist already rewrote the row: the stored JSON now
        // carries `standard`, so the next startup reads Standard directly.
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"renderingBackend\":\"standard\""));
        assert!(!stored.contains("\"enhanced\""));
    }

    /// The fold is a no-op for a document that is already Standard (a fresh
    /// profile included), and the `update` path normalizes too, so no writer
    /// can reintroduce the retired value into the persisted document.
    #[test]
    fn standard_backend_stays_standard_through_load_and_update() {
        use super::RenderingBackend;

        // No stored row: the fresh-profile path, untouched by the fold.
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        assert_eq!(
            state.snapshot().expect("snapshot").rendering_backend,
            RenderingBackend::Standard
        );
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"renderingBackend\":\"standard\""));

        state
            .update(|settings| settings.rendering_backend = RenderingBackend::Enhanced)
            .expect("update");
        assert_eq!(
            state.snapshot().expect("snapshot").rendering_backend,
            RenderingBackend::Standard
        );
        let rewritten = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(rewritten.contains("\"renderingBackend\":\"standard\""));
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

    /// Returns the declaration block of `selector`, up to and including its
    /// closing brace. QA helper for the source-pinned style assertions below.
    fn css_rule<'a>(stylesheet: &'a str, selector: &str) -> &'a str {
        let start = stylesheet
            .find(selector)
            .unwrap_or_else(|| panic!("missing stylesheet rule: {selector}"));
        let body = &stylesheet[start..];
        &body[..body.find('}').expect("rule end") + 1]
    }

    /// The visible Settings entry point: present in every expanded presentation,
    /// absent from the Orb, routed through the one existing settings action.
    ///
    /// The visibility rule is structural — `ProductContent` renders exactly in
    /// Floating expanded, Sidebar, and Desktop, and the Orb replaces it — so this
    /// pins the properties that keep it structural: one unconditional element, no
    /// mode list, and no dependency on the profile identity.
    #[test]
    fn footer_settings_gear_is_visible_in_every_expanded_mode() {
        let source = include_str!("../../src/components/ProductContent.vue");
        let footer_start = source.find("<footer class=\"signature\">").expect("footer");
        let footer = &source[footer_start..];

        assert_eq!(
            footer.matches("class=\"footer-settings-button\"").count(),
            1,
            "the footer must render exactly one Settings gear"
        );
        let gear_start = footer.find("class=\"footer-settings-button\"").expect("gear");
        let button_start = footer[..gear_start].rfind("<button").expect("gear button");
        let gear = &footer[button_start..];
        let tag = &gear[..gear.find('>').expect("gear opening tag")];
        assert!(
            !tag.contains("v-if"),
            "the gear must not be conditional, or an expanded mode would lose it: {tag}"
        );
        assert!(tag.contains("type=\"button\""), "the gear must be a real button");
        assert!(
            tag.contains("@click=\"$emit('openSettings')\""),
            "the gear must emit the settings event"
        );
        assert!(
            gear.contains(":aria-label=\"t('footer.settings')\"")
                && gear.contains(":title=\"t('footer.settings')\""),
            "the gear needs the localized accessible name and tooltip"
        );
        assert!(
            !source.contains("settingsOpen"),
            "the product surface must not own settings state; App.vue does"
        );

        // The Orb is a different component, rendered instead of the product
        // content, so it cannot carry the gear.
        let orb = include_str!("../../src/components/FloatingOrb.vue");
        assert!(
            !orb.contains("footer-settings"),
            "the collapsed Orb must not render the Settings gear"
        );

        // One wiring, into the existing command. `runAction('settings')` is the
        // same path the right-click entry and the tray use, so there is no second
        // settings surface.
        let app = include_str!("../../src/App.vue");
        assert!(
            app.contains("@open-settings=\"runAction('settings')\""),
            "the gear must reuse the product settings action"
        );

        // Letting the gear shrink would push it out of the footer when the
        // identity is long, and the identity already owns the shrinking.
        let styles = include_str!("../../src/styles.css");
        let gear_rule = css_rule(styles, ".signature .footer-settings-button {");
        assert!(gear_rule.contains("width: 30px") && gear_rule.contains("height: 30px"));
        assert!(gear_rule.contains("flex: none"));
        // Near-white at rest: the glyph must stay legible over translucent and
        // wallpaper backgrounds without reading as a primary action.
        assert!(
            gear_rule.contains("color: rgba(255, 255, 255, 0.92)"),
            "the gear rests near-white so it stays visible over any background"
        );
        let icon_rule = css_rule(styles, ".footer-settings-icon {");
        assert!(icon_rule.contains("width: 15px") && icon_rule.contains("height: 15px"));
        let feedback = css_rule(styles, ".signature .footer-settings-button:hover,");
        assert!(feedback.contains(":focus-visible"));
        assert!(
            feedback.contains("background:") && feedback.contains("color: #fff"),
            "hover and keyboard focus need a visible response"
        );
        let pressed = css_rule(styles, ".signature .footer-settings-button:active {");
        assert!(pressed.contains("background:"), "pressing the gear needs feedback");
    }

    /// The gear's label exists in both languages and matches the native menu's
    /// wording, so one action never has two names.
    #[test]
    fn footer_settings_label_matches_the_menu_label_in_both_languages() {
        let catalog = include_str!("../../src/i18n/catalog.ts");
        assert!(catalog.contains("\"footer.settings\": \"Settings\""));
        assert!(catalog.contains("\"footer.settings\": \"设置\""));
        // The right-click entry keeps its own key with the same wording.
        assert!(catalog.contains("\"menu.settings\": \"Settings\""));
        assert!(catalog.contains("\"menu.settings\": \"设置\""));

        let native = include_str!("locale.rs");
        assert!(native.contains("settings: \"Settings\""));
        assert!(native.contains("settings: \"设置\""));
    }

    /// A first run has no settings document at all, and that is the only state in
    /// which the product may pick a more discoverable starting presentation: the
    /// widget opens expanded, so the new user sees the product instead of a small
    /// Orb they would have to find.
    #[test]
    fn fresh_profile_opens_expanded_and_keeps_that_choice() {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        let fresh = state.snapshot().expect("snapshot");
        assert_eq!(fresh.mode, ProductWindowMode::Floating);
        assert_eq!(
            fresh.floating_presentation,
            FloatingPresentation::Expanded,
            "a fresh profile must open visibly"
        );

        // Persisted immediately, so the user's own later collapse is the only
        // thing that can change it from here on.
        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"floatingPresentation\":\"expanded\""));
        let reopened = AppState::load(state.database)
            .expect("reload")
            .snapshot()
            .expect("snapshot");
        assert_eq!(reopened.floating_presentation, FloatingPresentation::Expanded);
    }

    /// The upgrade path must not touch an existing profile. A stored document
    /// wins over the fresh-profile default even when it records a collapse, and a
    /// document written before the key existed keeps its historical default
    /// instead of inheriting the new first-run behavior.
    #[test]
    fn stored_documents_keep_their_floating_presentation() {
        for (document, expected) in [
            (
                r#"{"mode":"floating","floatingPresentation":"collapsed"}"#,
                FloatingPresentation::Collapsed,
            ),
            (
                r#"{"mode":"floating","floatingPresentation":"expanded"}"#,
                FloatingPresentation::Expanded,
            ),
            // Pre-key document: the serde default, not the fresh-profile default.
            (r#"{"mode":"floating"}"#, FloatingPresentation::Collapsed),
            // A Desktop profile is unaffected in every other respect too.
            (
                r#"{"mode":"desktop","floatingPresentation":"collapsed"}"#,
                FloatingPresentation::Collapsed,
            ),
        ] {
            let database = Database::in_memory().expect("database");
            database
                .set_setting("product_settings", document)
                .expect("stored settings");
            let restored = AppState::load(database)
                .expect("load")
                .snapshot()
                .expect("snapshot");
            assert_eq!(
                restored.floating_presentation, expected,
                "an existing profile changed for {document}"
            );
        }
    }

    /// A first run starts on the Gradient material in every mode, using the
    /// project's documented palette and the documented translucency.
    ///
    /// This is a fresh-profile decision only: the Standard backend has no native
    /// Acrylic, so Glass there is a tint over whatever wallpaper is behind it.
    /// Gradient keeps the same graphite language with a defined shape, which reads
    /// consistently over dark, light, saturated, and textured desktops.
    #[test]
    fn fresh_profile_uses_the_gradient_material_in_every_mode() {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        let fresh = state.snapshot().expect("snapshot");

        for (mode, profile) in [
            ("sidebar", &fresh.appearance_profiles.sidebar),
            ("floating", &fresh.appearance_profiles.floating),
            ("desktop", &fresh.appearance_profiles.desktop),
        ] {
            assert_eq!(
                profile.background_type,
                BackgroundType::Gradient,
                "{mode} must start on Gradient"
            );
            // The documented palette, not a fresh invention.
            assert_eq!(profile.gradient_start_color, "#11191e", "{mode}");
            assert_eq!(profile.gradient_end_color, "#213747", "{mode}");
            assert_eq!(profile.gradient_angle, 135.0, "{mode}");
            // Still translucent, and exactly the documented opacity: the fresh
            // surface must not become an opaque black rectangle.
            assert_eq!(
                profile.background_opacity,
                AppearanceSettings::default().background_opacity,
                "{mode}"
            );
            assert!(profile.background_opacity < 1.0, "{mode}");
            // The Glass values a user would see after switching material back are
            // untouched.
            assert_eq!(profile.glass_tint_color, "#11191e", "{mode}");
            assert_eq!(profile.glass_tint_opacity, 0.78, "{mode}");
        }

        // Nothing else about the fresh profile changed.
        assert_eq!(fresh.mode, ProductWindowMode::Floating);
        assert_eq!(fresh.floating_presentation, FloatingPresentation::Expanded);
        assert_eq!(fresh.rendering_backend, super::RenderingBackend::Standard);

        let stored = state
            .database
            .setting("product_settings")
            .expect("stored")
            .expect("value");
        assert!(stored.contains("\"backgroundType\":\"gradient\""));
    }

    /// The upgrade path must not touch an existing profile's material, and a
    /// document written before the appearance keys existed keeps the historical
    /// serde default instead of inheriting the new first-run choice.
    #[test]
    fn stored_documents_keep_their_material_and_historical_defaults() {
        for (document, expected, why) in [
            (
                r#"{"mode":"floating","appearanceProfiles":{"floating":{"backgroundType":"glass"}}}"#,
                BackgroundType::Glass,
                "a saved Glass profile stays Glass",
            ),
            (
                r#"{"mode":"floating","appearanceProfiles":{"floating":{"backgroundType":"gradient"}}}"#,
                BackgroundType::Gradient,
                "a saved Gradient profile stays Gradient",
            ),
            (
                r#"{"mode":"floating","appearanceProfiles":{"floating":{"backgroundType":"solid"}}}"#,
                BackgroundType::Solid,
                "a saved Solid profile stays Solid",
            ),
            (
                r#"{"mode":"floating"}"#,
                BackgroundType::Glass,
                "a pre-appearance document keeps the historical Glass default",
            ),
            (
                r#"{"mode":"floating","appearance":"geological_observatory"}"#,
                BackgroundType::Glass,
                "the appearance id alone does not select a material",
            ),
        ] {
            let database = Database::in_memory().expect("database");
            database
                .set_setting("product_settings", document)
                .expect("stored settings");
            let restored = AppState::load(database)
                .expect("load")
                .snapshot()
                .expect("snapshot");
            assert_eq!(
                restored.appearance_profiles.floating.background_type,
                expected,
                "{why} (document: {document})"
            );
        }
    }

    /// The shipped profile identity is a neutral placeholder, not a person.
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
            super::QuickLink::new("b", "Docs", "https://example.com/docs"),
            super::QuickLink::new("a", "Docs", "https://example.com/docs"),
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
            vec![super::QuickLink::new(
                super::LEGACY_HOMEPAGE_LINK_ID,
                // The user's own label is kept verbatim, language included.
                "Personal Site",
                "https://www.example.net",
            )]
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

        settings.quick_links = vec![super::QuickLink::new(
            "kept",
            "Kept",
            "https://example.com/kept",
        )];
        settings.normalize();
        assert_eq!(settings.quick_links.len(), 1);
        assert_eq!(settings.quick_links[0].id, "kept");
    }

    #[test]
    fn normalizing_drops_unusable_quick_links_and_trims_the_rest() {
        let mut settings = super::ProductSettings {
            quick_links: vec![
                super::QuickLink::new("  keep  ", "  Docs  ", "  https://example.com/docs  "),
                super::QuickLink::new("no-name", "   ", "https://example.com"),
                super::QuickLink::new("bad-url", "Bad", "file:///C:/private.txt"),
                super::QuickLink::new(String::new(), "No id", "https://example.com"),
            ],
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(
            settings.quick_links,
            vec![super::QuickLink::new(
                "keep",
                "Docs",
                "https://example.com/docs",
            )]
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

    // --- Settings unknown-key round-trip (Phase 2A contract) ----------------

    /// A temp SQLite path unique to one test run; callers clean up themselves.
    fn temp_db_path(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "alan-desktop-{label}-{}-{suffix}.sqlite3",
            std::process::id()
        ))
    }

    fn remove_db(path: &std::path::Path) {
        for candidate in [
            path.to_path_buf(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    fn future_feature() -> serde_json::Value {
        serde_json::json!({
            "enabled": true,
            "threshold": 17
        })
    }

    fn stored_document(database: &Database) -> serde_json::Value {
        serde_json::from_str(
            &database
                .setting("product_settings")
                .expect("read stored document")
                .expect("stored document exists"),
        )
        .expect("stored document parses")
    }

    /// A: a top-level unknown member written by a future version survives the
    /// current runtime's load → normalize → persist verbatim.
    #[test]
    fn top_level_unknown_members_round_trip() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","futureFeature":{"enabled":true,"threshold":17}}"#,
            )
            .expect("future document");
        let state = AppState::load(database).expect("load");
        let snapshot = state.snapshot().expect("snapshot");
        // Typed semantics unchanged…
        assert_eq!(snapshot.mode, ProductWindowMode::Floating);
        // …and the unknown member is round-tripped, typed-field shape intact.
        assert_eq!(snapshot.extra.get("futureFeature"), Some(&future_feature()));
        assert_eq!(stored_document(&state.database)["futureFeature"], future_feature());
    }

    /// B: unknown members inside evolvable nested objects (the per-mode
    /// appearance profile, and the profile container itself) survive too —
    /// not only top-level keys.
    #[test]
    fn nested_unknown_members_round_trip() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","appearanceProfiles":{"floating":{"futureMaterial":"quantum"},"futureMode":{"backgroundType":"glass"}}}"#,
            )
            .expect("future document");
        let state = AppState::load(database).expect("load");
        let snapshot = state.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.appearance_profiles.floating.extra.get("futureMaterial"),
            Some(&serde_json::json!("quantum"))
        );
        assert_eq!(
            snapshot
                .appearance_profiles
                .extra
                .get("futureMode")
                .expect("container-level unknown member"),
            &serde_json::json!({ "backgroundType": "glass" })
        );
        let stored = stored_document(&state.database);
        assert_eq!(stored["appearanceProfiles"]["floating"]["futureMaterial"], "quantum");
        assert_eq!(stored["appearanceProfiles"]["futureMode"]["backgroundType"], "glass");
    }

    /// C: a document missing known fields still defaults them, and its
    /// unknown members are unaffected by the defaulting.
    #[test]
    fn missing_known_fields_default_without_touching_unknown_members() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting("product_settings", r#"{"mode":"sidebar","futureFeature":1}"#)
            .expect("partial future document");
        let restored = AppState::load(database)
            .expect("load")
            .snapshot()
            .expect("snapshot");
        assert_eq!(restored.width, 620);
        assert_eq!(restored.day_rollover, "04:00");
        assert_eq!(
            restored.extra.get("futureFeature"),
            Some(&serde_json::json!(1))
        );
    }

    /// D: the runtime updating a known field and persisting keeps every
    /// unknown member, value for value.
    #[test]
    fn known_field_update_preserves_unknown_members() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","dayRollover":"02:00","futureFeature":{"enabled":true,"threshold":17}}"#,
            )
            .expect("future document");
        let state = AppState::load(database).expect("load");
        state
            .update(|settings| settings.mode = ProductWindowMode::Desktop)
            .expect("known-field update");
        let stored = stored_document(&state.database);
        assert_eq!(stored["mode"], "desktop");
        assert_eq!(stored["dayRollover"], "02:00");
        assert_eq!(stored["futureFeature"], future_feature());
    }

    /// E: legacy migration keys are consumed exactly as before (the pair
    /// becomes a Quick Link and is not written back), while an unrelated
    /// future key in the same document is preserved.
    #[test]
    fn legacy_migration_consumes_its_keys_but_keeps_unrelated_unknowns() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"homepageLabel":"Personal Site","homepageUrl":"https://www.example.net","futureFeature":{"enabled":true,"threshold":17}}"#,
            )
            .expect("legacy + future document");
        let state = AppState::load(database).expect("load");
        let snapshot = state.snapshot().expect("snapshot");
        // The legacy pair was consumed by the documented migration…
        assert_eq!(snapshot.quick_links.len(), 1);
        assert_eq!(snapshot.quick_links[0].id, super::LEGACY_HOMEPAGE_LINK_ID);
        // …and is no longer written back, while the unrelated future key is.
        let stored = stored_document(&state.database);
        assert!(stored.get("homepageUrl").is_none());
        assert!(stored.get("homepageLabel").is_none());
        assert_eq!(stored["futureFeature"], future_feature());
    }

    /// A: persist-failure atomicity. A deterministic SQLite write failure —
    /// a BEFORE INSERT trigger that aborts, leaving the existing row and
    /// table intact — must leave the update as a single failed transaction:
    /// memory stays at the pre-call state and the disk row is the genuine
    /// pre-call document. The runtime then self-heals on the next update.
    #[test]
    fn persist_failure_leaves_memory_and_disk_at_the_previous_state() {
        let path = temp_db_path("update-failure-atomicity");
        let state = AppState::load(Database::open(&path).expect("open")).expect("state");
        assert_eq!(
            state.snapshot().expect("snapshot").mode,
            ProductWindowMode::Floating
        );

        let saboteur = rusqlite::Connection::open(&path).expect("saboteur");
        saboteur
            .execute_batch(
                "CREATE TRIGGER block_settings_writes BEFORE INSERT ON app_settings
                 BEGIN SELECT RAISE(ABORT, 'writes are blocked'); END;",
            )
            .expect("install write blocker");

        let error = state
            .update(|settings| settings.mode = ProductWindowMode::Desktop)
            .expect_err("a failed durable write must fail the update");
        assert!(
            error.contains("writes are blocked"),
            "the error must name the durable-write failure: {error}"
        );
        // Memory: pre-call state — the mutation was never committed.
        assert_eq!(
            state.snapshot().expect("snapshot").mode,
            ProductWindowMode::Floating
        );

        // Disk: the genuine pre-call row, untouched by the failed update.
        saboteur
            .execute_batch("DROP TRIGGER block_settings_writes")
            .expect("remove write blocker");
        assert_eq!(stored_document(&state.database)["mode"], "floating");

        // Self-healing: the next update persists normally.
        state
            .update(|settings| settings.mode = ProductWindowMode::Desktop)
            .expect("retry after the failure is resolved");
        assert_eq!(
            state.snapshot().expect("snapshot").mode,
            ProductWindowMode::Desktop
        );
        drop(saboteur);
        remove_db(&path);
    }

    /// B: concurrency ordering. With the settings Mutex held across persist,
    /// a second update cannot durably commit before the first one: while the
    /// first update is blocked inside its own persist (SQLite write lock,
    /// released at a controlled moment), the second is still queued behind
    /// the Mutex and the disk still carries the pre-call document. The
    /// completion order is then asserted through channels, not wall-clock
    /// races: A completes (durable commit included) strictly before B.
    #[test]
    fn concurrent_updates_cannot_commit_out_of_order() {
        let path = temp_db_path("update-ordering");
        let state = AppState::load(Database::open(&path).expect("open")).expect("state");

        let holder = rusqlite::Connection::open(&path).expect("holder");
        holder.execute_batch("BEGIN IMMEDIATE").expect("write lock");

        std::thread::scope(|scope| {
            // A: blocks inside set_setting (busy window) holding the settings
            // Mutex across its persist.
            let (a_done_tx, a_done_rx) = std::sync::mpsc::channel();
            let state_a = &state;
            let a_thread = scope.spawn(move || {
                let result = state_a.update(|settings| settings.display_name = "First".into());
                let _ = a_done_tx.send(result.is_ok());
                result
            });

            // A is now parked inside its persist. B must queue behind the
            // Mutex, never run ahead of A's durable commit.
            std::thread::sleep(std::time::Duration::from_millis(300));
            let (b_done_tx, b_done_rx) = std::sync::mpsc::channel();
            let state_b = &state;
            let b_thread = scope.spawn(move || {
                let result = state_b.update(|settings| settings.display_name = "Second".into());
                let _ = b_done_tx.send(result.is_ok());
                result
            });
            std::thread::sleep(std::time::Duration::from_millis(300));

            // Both updates are still in flight: the disk must be unchanged,
            // proving neither — B in particular — has durably committed.
            let disk_display_name: String = holder
                .query_row(
                    "SELECT value FROM app_settings WHERE key='product_settings'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map(|document| {
                    serde_json::from_str::<serde_json::Value>(&document)
                        .expect("parse disk document")["displayName"]
                        .as_str()
                        .expect("displayName")
                        .to_string()
                })
                .expect("read disk while both updates are in flight");
            assert_eq!(
                disk_display_name, "User",
                "no in-flight update may have committed to disk yet"
            );

            // Release the write lock: A's persist lands, A finishes, and only
            // then can B reach its own persist. The recv order pins this.
            drop(holder);
            assert!(
                a_done_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("A"),
                "A must succeed"
            );
            let a_snapshot = a_thread.join().expect("A thread").expect("A update");
            assert_eq!(a_snapshot.display_name, "First");
            assert!(
                b_done_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("B"),
                "B must succeed"
            );
            let b_snapshot = b_thread.join().expect("B thread").expect("B update");
            assert_eq!(b_snapshot.display_name, "Second");
        });

        // Final state is B's document, durably and in memory.
        assert_eq!(state.snapshot().expect("snapshot").display_name, "Second");
        assert_eq!(stored_document(&state.database)["displayName"], "Second");
        remove_db(&path);
    }

    /// A stored Quick Link written by a future version carries its own
    /// unknown member: the current runtime editing a known field of that link
    /// (and normalizing/persisting the whole document) must keep the unknown
    /// member value-for-value, not just survive its presence.
    #[test]
    fn quick_link_unknown_member_survives_known_field_edit() {
        let database = Database::in_memory().expect("database");
        database
            .set_setting(
                "product_settings",
                r#"{"mode":"floating","quickLinks":[{"id":"link-a","name":"Docs","url":"https://example.com/docs","futureIcon":"data:image/svg;base64,abc"}]}"#,
            )
            .expect("future link document");
        let state = AppState::load(database).expect("load");
        assert_eq!(
            state.snapshot().expect("snapshot").quick_links[0]
                .extra
                .get("futureIcon"),
            Some(&serde_json::json!("data:image/svg;base64,abc"))
        );
        // The runtime edits the known fields of that same link…
        state
            .update(|settings| {
                settings.quick_links[0].name = "Renamed Docs".into();
                settings.quick_links[0].url = "https://example.com/renamed".into();
            })
            .expect("known-field edit");
        // …and the unknown member is preserved verbatim through normalize and
        // the persist that `update` performs.
        let stored = stored_document(&state.database);
        assert_eq!(stored["quickLinks"][0]["name"], "Renamed Docs");
        assert_eq!(stored["quickLinks"][0]["url"], "https://example.com/renamed");
        assert_eq!(
            stored["quickLinks"][0]["futureIcon"],
            "data:image/svg;base64,abc"
        );
    }

    /// F — the round-trip regression this whole feature exists for: a future
    /// version writes future-only keys, the current (rollback) runtime loads
    /// and re-persists the document, and a future version reads again. The
    /// future keys must still be there, byte-value for byte-value.
    #[test]
    fn rollback_simulation_future_keys_survive_the_current_runtime() {
        let path = temp_db_path("unknown-key-rollback");

        // Current runtime creates its document through the real path.
        let state = AppState::load(Database::open(&path).expect("open")).expect("state");
        state
            .update(|settings| settings.display_name = "Rollback Person".into())
            .expect("seed");

        // "Future version" writes a top-level and a nested future-only key.
        {
            let mut document = stored_document(&state.database);
            document["futureFeature"] = future_feature();
            document["appearanceProfiles"]["sidebar"]["futureMaterial"] =
                serde_json::json!("quantum");
            state
                .database
                .set_setting("product_settings", &document.to_string())
                .expect("future write");
        }
        drop(state);

        // Current rollback runtime: full startup load → normalize → persist.
        {
            let rolled_back = AppState::load(Database::open(&path).expect("reopen"))
                .expect("rollback runtime must load");
            assert_eq!(
                rolled_back.snapshot().expect("snapshot").display_name,
                "Rollback Person"
            );
        }

        // "Future version" reads again: both keys survived, exactly.
        let database = Database::open(&path).expect("future reader");
        let stored = stored_document(&database);
        assert_eq!(stored["futureFeature"], future_feature());
        assert_eq!(
            stored["appearanceProfiles"]["sidebar"]["futureMaterial"],
            "quantum"
        );
        drop(database);
        remove_db(&path);
    }

    /// Normalize must never delete or rewrite unknown extras: they are inert
    /// data the current runtime does not consume, even while it clamps,
    /// folds, trims, and prunes the known fields around them.
    #[test]
    fn normalize_never_touches_unknown_extras() {
        let mut settings = super::ProductSettings {
            extra: [
                ("futureFeature".to_string(), future_feature()),
                ("keptKey".to_string(), serde_json::json!("value")),
            ]
            .into_iter()
            .collect(),
            ..super::ProductSettings::default()
        };
        settings.appearance_profiles.floating.extra.insert(
            "futureMaterial".to_string(),
            serde_json::json!("quantum"),
        );
        // Drive every destructive normalize path with hostile inputs. The
        // link itself is usable (an unusable link is pruned whole — the
        // documented prune rule — so there would be nothing left to check);
        // what is under test is that its unknown extras survive the trim.
        settings.width = 9999;
        settings.height = 1;
        settings.display_name = "  padded  ".into();
        settings.quick_links = vec![QuickLink {
            id: "  a  ".into(),
            name: "  Docs  ".into(),
            url: "  https://example.com/docs  ".into(),
            extra: [("futureLinkField".to_string(), serde_json::json!(7))]
                .into_iter()
                .collect(),
        }];
        settings.normalize();
        assert_eq!(settings.extra.get("futureFeature"), Some(&future_feature()));
        assert_eq!(settings.extra.get("keptKey"), Some(&serde_json::json!("value")));
        assert_eq!(
            settings.appearance_profiles.floating.extra.get("futureMaterial"),
            Some(&serde_json::json!("quantum"))
        );
        assert_eq!(
            settings.quick_links[0].extra.get("futureLinkField"),
            Some(&serde_json::json!(7))
        );
    }

    // --- Controlled startup DB failure --------------------------------------

    /// "Legitimately absent" is not "broken": no stored document is a first
    /// run and resolves to the default backend, not an error.
    #[test]
    fn missing_settings_read_as_the_default_backend() {
        let dir = std::env::temp_dir().join(format!(
            "alan-desktop-backend-fresh-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let backend = AppState::read_rendering_backend(&dir).expect("first run");
        assert_eq!(backend, super::RenderingBackend::Standard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stored document with a rendering backend reports it.
    #[test]
    fn stored_rendering_backend_is_reported() {
        let dir = std::env::temp_dir().join(format!(
            "alan-desktop-backend-stored-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        {
            let state = AppState::load(Database::open(dir.join("alan-desktop.sqlite3")).expect("db"))
                .expect("state");
            state
                .update(|settings| settings.day_rollover = "03:00".into())
                .expect("persist a document");
        }
        let backend = AppState::read_rendering_backend(&dir).expect("read backend");
        assert_eq!(backend, super::RenderingBackend::Standard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A database locked beyond the busy timeout is a real error returned to
    /// the caller — never the default backend, never a panic. The controlled
    /// startup path (log + native message + non-zero exit) then takes over;
    /// the lib.rs source test below pins that no `.expect` remains on the
    /// Tauri build. WAL readers are never blocked by a plain write lock, so
    /// the seed leaves migration 2 pending: the early read's initialize has
    /// a real write to perform, and that write is what waits on the lock.
    #[test]
    fn locked_database_is_an_error_not_the_default_backend() {
        let dir = std::env::temp_dir().join(format!(
            "alan-desktop-backend-locked-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("sandbox dir");
        let db_path = dir.join("alan-desktop.sqlite3");
        {
            let seed = rusqlite::Connection::open(&db_path).expect("seed connection");
            seed.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL
                 );
                 INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1);
                 CREATE TABLE shortcuts (
                   id TEXT PRIMARY KEY,
                   label TEXT NOT NULL,
                   url TEXT NOT NULL,
                   sort_order INTEGER NOT NULL DEFAULT 0,
                   enabled INTEGER NOT NULL DEFAULT 1,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );",
            )
            .expect("seed schema");
        }
        let holder = rusqlite::Connection::open(&db_path).expect("holder");
        holder
            .execute_batch("BEGIN IMMEDIATE")
            .expect("write lock");

        let started = std::time::Instant::now();
        let result = AppState::read_rendering_backend(&dir);
        let elapsed = started.elapsed();
        assert!(result.is_err(), "a locked database must be an error");
        assert!(
            result.unwrap_err().to_lowercase().contains("locked"),
            "the error must name the conflict"
        );
        // Bounded: the timeout fired rather than hanging forever (loose bound).
        assert!(
            elapsed >= std::time::Duration::from_millis(1000),
            "{elapsed:?}"
        );
        assert!(elapsed < std::time::Duration::from_secs(15), "{elapsed:?}");
        drop(holder);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
