use crate::{
    appearance::{self, AppearanceProfiles, AppearanceSettings},
    platform,
    settings::{
        self, AppState, FloatingPresentation, ProductSettings, ProductWindowMode, RenderingBackend,
        SidebarSide, TemperatureUnit,
    },
    task_day,
    window_mode::{self, NativeWindowState},
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::Arc;
#[cfg(target_os = "windows")]
use tauri::window::{Effect, EffectsBuilder};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

const SNAP_THRESHOLD: i32 = 24;
const MIN_VISIBLE: i32 = 80;
pub const ORB_SIZE_DIP: u32 = 56;
const ORB_MARGIN_DIP: i32 = 32;

#[derive(Default)]
pub struct ProductWindowRuntime {
    applying: AtomicBool,
    qa_native_material_permanently_off: bool,
    qa_native_material_bypassed: AtomicBool,
    qa_force_floating_expanded: bool,
    /// Whether this process actually hosts the WebView through the
    /// CompositionController.
    ///
    /// The native material host sets a `DesktopWindowTarget` on the top-level
    /// HWND. That is only valid when the WebView is composited into the app's own
    /// visual tree; driving it while the WebView is windowed (a child HWND)
    /// replaces the WebView's presented surface and starves it. This flag is what
    /// keeps the two hosting paths apart.
    composition_hosting: bool,
    #[cfg(target_os = "windows")]
    native_material: platform::windows::composition_host::NativeWindowContextStore,
}

impl ProductWindowRuntime {
    pub fn new(
        qa_native_material_permanently_off: bool,
        initial_bypass: bool,
        qa_force_floating_expanded: bool,
        composition_hosting: bool,
    ) -> Self {
        Self {
            applying: AtomicBool::new(false),
            qa_native_material_permanently_off,
            qa_native_material_bypassed: AtomicBool::new(initial_bypass),
            qa_force_floating_expanded,
            composition_hosting,
            #[cfg(target_os = "windows")]
            native_material: Arc::new(Default::default()),
        }
    }

    /// Shared handle to the platform composition host, used by the pre-WebView
    /// Wry hooks.
    #[cfg(target_os = "windows")]
    pub fn native_material_store(
        &self,
    ) -> platform::windows::composition_host::NativeWindowContextStore {
        Arc::clone(&self.native_material)
    }

    /// Applies the post-WebView CompositionController settings.
    #[cfg(target_os = "windows")]
    pub fn attach_composition_controller(&self, window: &WebviewWindow) -> Result<(), String> {
        platform::windows::composition_host::attach_controller(&self.native_material, window)
    }

    #[cfg(target_os = "windows")]
    pub fn shutdown_native_material(&self) {
        platform::windows::composition_host::shutdown(&self.native_material);
    }

    #[cfg(not(target_os = "windows"))]
    pub fn shutdown_native_material(&self) {}

    fn effective_settings(&self, mut settings: ProductSettings) -> ProductSettings {
        if self.qa_force_floating_expanded {
            settings.mode = ProductWindowMode::Floating;
            settings.floating_presentation = FloatingPresentation::Expanded;
            settings.width = 642;
            settings.height = 750;
        }
        settings
    }
}

struct ApplyingGuard<'a>(&'a AtomicBool);

impl Drop for ApplyingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

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

#[derive(Clone, Debug, PartialEq)]
struct WorkArea {
    identity: Option<String>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
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

fn apply_native_composition(
    window: &WebviewWindow,
    runtime: &ProductWindowRuntime,
    mode: ProductWindowMode,
    presentation: FloatingPresentation,
    appearance: &AppearanceSettings,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if runtime.qa_native_material_bypassed.load(Ordering::Acquire) {
            window
                .set_effects(None)
                .map_err(|error| error.to_string())?;
            platform::windows::composition_host::shutdown(&runtime.native_material);
            window
                .app_handle()
                .state::<crate::qa_diagnostics::QaDiagnostics>()
                .record("native_material_bypassed=true");
            return Ok(());
        }
        let requested_glass = appearance.background_type == appearance::BackgroundType::Glass;
        let native_host = match (mode, presentation) {
            (ProductWindowMode::Floating, FloatingPresentation::Expanded) => {
                platform::windows::composition_host::NativeHost::FloatingExpanded
            }
            (ProductWindowMode::Floating, FloatingPresentation::Collapsed) => {
                platform::windows::composition_host::NativeHost::FloatingCollapsed
            }
            (ProductWindowMode::Sidebar, _) => {
                platform::windows::composition_host::NativeHost::Sidebar
            }
            (ProductWindowMode::Desktop, _) => {
                platform::windows::composition_host::NativeHost::Desktop
            }
        };
        window
            .app_handle()
            .state::<crate::qa_diagnostics::QaDiagnostics>()
            .record(format!(
                "native_material_bypassed=false requested_glass={requested_glass} native_host={native_host:?} composition_hosting={}",
                runtime.composition_hosting
            ));
        // The native material host (DesktopWindowTarget + DesktopAcrylicController)
        // belongs to the CompositionController hosting path, where the WebView is
        // composited into the app's own visual tree. Driving it while the WebView
        // is windowed — the Standard path — puts a composition target on the
        // parent of the WebView's child HWND: the window then presents only the
        // acrylic fill (blank frosted glass with a fully mounted Vue app behind
        // it) and Chromium treats the view as occluded, throttling rendering and
        // resource loading into multi-second stalls. The Standard path therefore
        // stays CSS-only, which is what the appearance-composition line below has
        // always reported.
        if mode == ProductWindowMode::Sidebar
            && appearance.background_type == appearance::BackgroundType::Glass
        {
            if runtime.composition_hosting {
                platform::windows::composition_host::apply_material(
                    &runtime.native_material,
                    window,
                    requested_glass,
                    native_host,
                )?;
            }
            window
                .set_effects(EffectsBuilder::new().effect(Effect::Acrylic).build())
                .map_err(|error| error.to_string())?;
            eprintln!(
                "[appearance-composition] mode={mode:?} · webview=transparent · native=Acrylic · css=graphite-tint"
            );
        } else {
            window
                .set_effects(None)
                .map_err(|error| error.to_string())?;
            if runtime.composition_hosting {
                platform::windows::composition_host::apply_material(
                    &runtime.native_material,
                    window,
                    requested_glass,
                    native_host,
                )?;
            }
            let fallback = if mode == ProductWindowMode::Desktop
                && appearance.background_type == appearance::BackgroundType::Glass
            {
                // Desktop is child-hosted under the shell, and
                // DesktopAcrylicController needs top-level HWND semantics, so the
                // native Acrylic path is unreachable here by design rather than by
                // omission — Enhanced Desktop is still supported, it just falls
                // back to translucent Graphite. See docs/desktop-mode.md.
                "translucent-graphite"
            } else if mode == ProductWindowMode::Floating
                && presentation == FloatingPresentation::Collapsed
                && appearance.background_type == appearance::BackgroundType::Glass
            {
                "transparent-orb-tint"
            } else {
                "selected-css-material"
            };
            eprintln!(
                "[appearance-composition] mode={mode:?} · webview=transparent · native=none · css={fallback}"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, runtime, mode, presentation, appearance);
    }
    Ok(())
}

#[tauri::command]
pub fn qa_native_material_control(
    window: WebviewWindow,
    diagnostics: tauri::State<'_, crate::qa_diagnostics::QaDiagnostics>,
    action: &str,
) -> Result<(), String> {
    if !diagnostics.native_material_late() {
        return Err("manual material control requires --qa-native-material-late".into());
    }
    let action = match action {
        "attach" => "attach",
        "detach" => "detach",
        _ => return Err("unknown QA material action".into()),
    };
    diagnostics.record(format!(
        "native_{action}_requested=true timestamp_ms={}",
        crate::qa_diagnostics::timestamp_ms()
    ));
    let app = window.app_handle().clone();
    window
        .run_on_main_thread(move || {
            let diagnostics = app.state::<crate::qa_diagnostics::QaDiagnostics>();
            let runtime = app.state::<ProductWindowRuntime>();
            let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            diagnostics.record(format!(
                "native_{action}_thread={thread} timestamp_ms={}",
                crate::qa_diagnostics::timestamp_ms()
            ));
            if action == "attach" {
                if runtime.qa_native_material_permanently_off {
                    diagnostics.record("native_attach_started=false reason=permanently-off");
                    return;
                }
                runtime
                    .qa_native_material_bypassed
                    .store(false, Ordering::Release);
            } else {
                runtime
                    .qa_native_material_bypassed
                    .store(true, Ordering::Release);
            }
            diagnostics.record(format!(
                "native_{action}_started=true timestamp_ms={}",
                crate::qa_diagnostics::timestamp_ms()
            ));
            let result = (|| -> Result<(), String> {
                let window = app
                    .get_webview_window("main")
                    .ok_or_else(|| "main window unavailable".to_string())?;
                let settings = runtime.effective_settings(app.state::<AppState>().snapshot()?);
                apply_native_composition(
                    &window,
                    &runtime,
                    settings.mode,
                    settings.floating_presentation,
                    &settings.appearance_profiles.for_mode(settings.mode),
                )?;
                diagnostics.record(platform::windows::composition_host::diagnostic_summary(
                    &runtime.native_material,
                ));
                Ok(())
            })();
            match result {
                Ok(()) => diagnostics.record(format!("native_{action}_completed=true")),
                Err(error) => diagnostics.record(format!(
                    "native_{action}_completed=false reason={}",
                    error.replace(['\r', '\n'], " ")
                )),
            }
        })
        .map_err(|error| error.to_string())
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

#[tauri::command]
pub fn request_window_drag(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let settings = state.snapshot()?;
    if !window_drag_allowed(&settings) {
        return Ok(());
    }
    eprintln!(
        "[window-drag] requested=true mode={:?} locked={} composition_hosting=true",
        settings.mode, settings.locked
    );
    window.start_dragging().map_err(|error| error.to_string())
}

fn window_drag_allowed(settings: &ProductSettings) -> bool {
    // Native dragging is a Floating capability. Sidebar is edge-anchored, so it
    // is never freely repositioned and keeps the guard. Keeping the same guard
    // behind the command prevents future Weather/Appearance DOM changes from
    // accidentally enabling top-level dragging in Sidebar mode.
    matches!(
        settings.mode,
        ProductWindowMode::Floating | ProductWindowMode::Desktop
    ) && !settings.locked
}

/// Whether the product window should expose a user-grabbable sizing border.
///
/// Sidebar is resizable because its window width **is** the product's
/// `sidebarWidth` setting: `handle_window_event` already clamps and persists the
/// width from the `Resized` event. On a frameless window, though, omitting
/// `WS_SIZEBOX` leaves no edge to grab, which silently froze that setting at
/// whatever value had last been stored. Floating in its collapsed Orb
/// presentation stays fixed-size.
///
/// Desktop is resizable while unlocked so the widget can be sized by hand. It is
/// a frameless `WS_CHILD` of `SHELLDLL_DefView`, so it has no native frame: the
/// sizing border comes from the undecorated resize borders Tauri installs for
/// frameless resizable windows, and the Desktop child style keeps the
/// `WS_THICKFRAME` bit those borders test for (`window_mode`). The
/// `Resized`/`Moved` handlers already persist Desktop geometry, so a resize
/// reuses the whole existing geometry path instead of adding a second one.
///
/// Sidebar ignores `locked` deliberately: the lock guards free repositioning,
/// while the sidebar is always anchored to a screen edge and the Resized handler
/// persists its width regardless of lock state.
pub(crate) fn product_window_resizable(mode: ProductWindowMode, settings: &ProductSettings) -> bool {
    match mode {
        ProductWindowMode::Sidebar => true,
        ProductWindowMode::Floating => {
            settings.floating_presentation == FloatingPresentation::Expanded && !settings.locked
        }
        ProductWindowMode::Desktop => !settings.locked,
    }
}

#[tauri::command]
pub fn open_quick_link(state: tauri::State<'_, AppState>, id: &str) -> Result<(), String> {
    let settings = state.snapshot()?;
    let link = settings
        .quick_links
        .iter()
        .find(|link| link.id == id)
        .ok_or_else(|| format!("quick link not found: {id}"))?;
    // The URL was validated when it was stored; it is validated again here because
    // the open path must not depend on any other writer having done so.
    settings::validate_quick_link_url(&link.url)?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer.exe")
        .arg(&link.url)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

pub fn restore_product_window(
    window: &WebviewWindow,
    app_state: &AppState,
    native_state: &NativeWindowState,
    runtime: &ProductWindowRuntime,
) -> Result<(), String> {
    ensure_logical_geometry(window, app_state)?;
    let mode = app_state.snapshot()?.mode;
    apply_product_mode(window, app_state, native_state, runtime, mode)
}

pub fn restore_window_to_visual_qa(
    window: &WebviewWindow,
    app_state: &AppState,
    native_state: &NativeWindowState,
    runtime: &ProductWindowRuntime,
) -> Result<(), String> {
    if runtime.applying.swap(true, Ordering::AcqRel) {
        return Err("window mode transition already in progress".into());
    }
    let _guard = ApplyingGuard(&runtime.applying);
    let settings = runtime.effective_settings(app_state.snapshot()?);

    window_mode::apply_adapter_mode(window, native_state, "normal")?;
    window
        .set_decorations(false)
        .and_then(|_| window.set_shadow(true))
        .and_then(|_| window.set_always_on_top(false))
        .and_then(|_| window.set_resizable(true))
        .map_err(|error| error.to_string())?;
    apply_minimum_size(window, 360, 500)?;
    window
        .set_size(PhysicalSize::new(642, 750))
        .map_err(|error| error.to_string())?;
    apply_native_composition(
        window,
        runtime,
        ProductWindowMode::Floating,
        FloatingPresentation::Expanded,
        settings
            .appearance_profiles
            .for_mode(ProductWindowMode::Floating),
    )?;

    window
        .app_handle()
        .state::<crate::qa_diagnostics::QaDiagnostics>()
        .record("[p7b-wtv] forced_geometry=true mode=FloatingExpanded size=642x750");
    Ok(())
}

pub fn apply_product_mode(
    window: &WebviewWindow,
    app_state: &AppState,
    native_state: &NativeWindowState,
    runtime: &ProductWindowRuntime,
    mode: ProductWindowMode,
) -> Result<(), String> {
    if runtime.applying.swap(true, Ordering::AcqRel) {
        return Err("window mode transition already in progress".into());
    }
    let _guard = ApplyingGuard(&runtime.applying);
    ensure_logical_geometry(window, app_state)?;
    let mut settings = app_state.snapshot()?;

    if mode == ProductWindowMode::Desktop {
        // A Desktop widget is reparented below SHELLDLL_DefView. Clear any
        // top-level DWM effect before that Phase 1 transition; child-window
        // Acrylic is not a reliable composition path.
        apply_native_composition(
            window,
            runtime,
            mode,
            settings.floating_presentation,
            settings.appearance_profiles.for_mode(mode),
        )?;
    }

    if settings.mode == ProductWindowMode::Desktop && mode != ProductWindowMode::Desktop {
        save_current_desktop_rect(window, app_state)?;
        settings = app_state.snapshot()?;
    }
    if mode == ProductWindowMode::Desktop
        && settings.mode == ProductWindowMode::Floating
        && settings.floating_presentation == FloatingPresentation::Expanded
    {
        save_current_floating_rect(window, app_state)?;
        settings = app_state.snapshot()?;
    }

    window
        .set_decorations(false)
        .map_err(|error| error.to_string())?;
    if mode == ProductWindowMode::Desktop {
        // Do not remove or move this earlier resizable pass, and do not let the
        // generic pass below run for Desktop.
        // Every tao window-flag setter recomputes the whole Win32 style from
        // tao's flags, so any of them running *after* the Shell reparent would
        // overwrite the child style `window_mode` just applied (`WS_CHILD` and
        // the sizing-border bit included), silently breaking Desktop child
        // hosting. The widget's final style therefore has to be settled before
        // `apply_adapter_desktop_mode`.
        window
            .set_resizable(product_window_resizable(mode, &settings))
            .map_err(|error| error.to_string())?;
    }

    let mut applied_desktop_bounds = None;
    match mode {
        ProductWindowMode::Desktop => {
            window
                .set_shadow(false)
                .map_err(|error| error.to_string())?;
            window
                .set_always_on_top(false)
                .map_err(|error| error.to_string())?;
            apply_minimum_size(window, 360, 500)?;
            let scale_factor =
                normalized_scale_factor(window.scale_factor().map_err(|error| error.to_string())?);
            let request = desktop_request_from_settings(&settings, scale_factor);
            eprintln!(
                "[desktop-lifecycle] mode transition: {:?} -> Desktop · geometry=bounded-desktop-widget · logical={} · tauriScaleFactor={scale_factor:.3} · physical={:?}",
                settings.mode,
                request.logical_bounds,
                request.bounds,
            );
            let applied_physical =
                window_mode::apply_adapter_desktop_mode(window, native_state, request)?;
            let applied_scale =
                normalized_scale_factor(window_mode::desktop_window_scale_factor(window)?);
            applied_desktop_bounds =
                Some(physical_bounds_to_logical(applied_physical, applied_scale));
        }
        ProductWindowMode::Floating => {
            window_mode::apply_adapter_mode(window, native_state, "normal")?;
            match settings.floating_presentation {
                FloatingPresentation::Collapsed => {
                    window
                        .set_shadow(false)
                        .map_err(|error| error.to_string())?;
                    apply_minimum_size(window, ORB_SIZE_DIP, ORB_SIZE_DIP)?
                }
                FloatingPresentation::Expanded => {
                    window.set_shadow(true).map_err(|error| error.to_string())?;
                    apply_minimum_size(window, 360, 500)?
                }
            }
            apply_saved_floating_rect(window, &settings)?;
            window
                .set_always_on_top(settings.always_on_top)
                .map_err(|error| error.to_string())?;
        }
        ProductWindowMode::Sidebar => {
            window_mode::apply_adapter_mode(window, native_state, "normal")?;
            window.set_shadow(true).map_err(|error| error.to_string())?;
            apply_minimum_size(window, 320, 500)?;
            apply_sidebar_rect(window, &settings)?;
            window
                .set_always_on_top(settings.always_on_top)
                .map_err(|error| error.to_string())?;
        }
    }
    if mode != ProductWindowMode::Desktop {
        // Do not reorder this after the Desktop branch above.
        // Desktop already settled its style before the reparent; re-running the
        // generic pass would clobber the child style. See the note above.
        window
            .set_resizable(product_window_resizable(mode, &settings))
            .map_err(|error| error.to_string())?;
    }
    app_state.update(|stored| {
        stored.mode = mode;
        if let Some(bounds) = applied_desktop_bounds {
            stored.desktop_x = Some(bounds.x);
            stored.desktop_y = Some(bounds.y);
            stored.desktop_width = bounds.width;
            stored.desktop_height = bounds.height;
        }
    })?;
    if mode != ProductWindowMode::Desktop {
        apply_native_composition(
            window,
            runtime,
            mode,
            settings.floating_presentation,
            settings.appearance_profiles.for_mode(mode),
        )?;
    }
    // A Desktop widget is a child of `SHELLDLL_DefView`, so it sits below every
    // top-level window — including the Win32 window WebView2 keeps for a
    // composition-hosted WebView, which would otherwise win the mouse hit test and
    // leave the visible widget completely inert. This runs last, after every
    // style/frame change of the transition, because each of them lets WebView2
    // re-apply its own bounds and visibility.
    //
    // The repair moves that runtime window off the virtual screen without hiding
    // it; hiding restores the hit test but stops WebView2 delivering forwarded
    // input to the page. See platform/windows/composition_host/input_target.rs.
    #[cfg(target_os = "windows")]
    if runtime.composition_hosting {
        platform::windows::composition_host::sync_input_target(
            &runtime.native_material,
            mode == ProductWindowMode::Desktop,
        )?;
    }
    Ok(())
}

pub fn set_floating_presentation(
    window: &WebviewWindow,
    app_state: &AppState,
    runtime: &ProductWindowRuntime,
    presentation: FloatingPresentation,
) -> Result<(), String> {
    if runtime.applying.swap(true, Ordering::AcqRel) {
        return Err("window presentation transition already in progress".into());
    }
    let _guard = ApplyingGuard(&runtime.applying);
    let settings = app_state.snapshot()?;
    if settings.mode != ProductWindowMode::Floating {
        return Err("Floating presentation is only available in Floating mode".into());
    }
    if settings.floating_presentation == presentation {
        return Ok(());
    }
    match presentation {
        FloatingPresentation::Collapsed => {
            // Expanded size, position, and monitor are persisted *before* the Orb
            // geometry is written, so collapsing never destroys the expanded rect
            // the user set. The Orb anchor is a separate saved value on purpose.
            save_current_floating_rect(window, app_state)?;
            let updated = app_state
                .update(|stored| stored.floating_presentation = FloatingPresentation::Collapsed)?;
            apply_minimum_size(window, ORB_SIZE_DIP, ORB_SIZE_DIP)?;
            window
                .set_shadow(false)
                .map_err(|error| error.to_string())?;
            apply_orb_rect(window, &updated)?;
            window
                .set_resizable(false)
                .map_err(|error| error.to_string())?;
        }
        FloatingPresentation::Expanded => {
            let areas = work_areas(window)?;
            let fallback = areas
                .first()
                .cloned()
                .ok_or_else(|| "no display work area available".to_string())?;
            // Derived from the Orb's saved corner rather than from `stored.x/y`:
            // the expanded window opens against the edge the Orb was parked on, so
            // dragging the Orb somewhere else and expanding again does not send the
            // widget back to a stale pre-collapse position.
            let rect = smart_expanded_rect(&settings, &areas, &fallback);
            eprintln!(
                "[floating-orb] apply expanded · logical={}x{} DIP · physical={},{} {}x{} · anchor={:?},{:?} · locked={} · topmost={}",
                settings.width,
                settings.height,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                settings.floating_orb_x,
                settings.floating_orb_y,
                settings.locked,
                settings.always_on_top,
            );
            app_state
                .update(|stored| stored.floating_presentation = FloatingPresentation::Expanded)?;
            window.set_shadow(true).map_err(|error| error.to_string())?;
            apply_minimum_size(window, 360, 500)?;
            window
                .set_size(PhysicalSize::new(rect.width, rect.height))
                .and_then(|_| window.set_position(PhysicalPosition::new(rect.x, rect.y)))
                .and_then(|_| window.set_resizable(!settings.locked))
                .map_err(|error| error.to_string())?;
        }
    }
    window
        .set_always_on_top(settings.always_on_top)
        .map_err(|error| error.to_string())?;
    let updated = app_state.snapshot()?;
    apply_native_composition(
        window,
        runtime,
        updated.mode,
        updated.floating_presentation,
        updated.appearance_profiles.for_mode(updated.mode),
    )?;
    Ok(())
}

fn apply_minimum_size(window: &WebviewWindow, width: u32, height: u32) -> Result<(), String> {
    let scale = normalized_scale_factor(window.scale_factor().map_err(|error| error.to_string())?);
    window
        .set_min_size(Some(PhysicalSize::new(
            logical_u32_to_physical(width, scale),
            logical_u32_to_physical(height, scale),
        )))
        .map_err(|error| error.to_string())
}

fn save_current_floating_rect(window: &WebviewWindow, app_state: &AppState) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let scale_factor =
        normalized_scale_factor(window.scale_factor().map_err(|error| error.to_string())?);
    let monitor_identity = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .and_then(|monitor| monitor.name().cloned());
    app_state.update(|stored| {
        stored.x = Some(physical_i32_to_logical(position.x, scale_factor));
        stored.y = Some(physical_i32_to_logical(position.y, scale_factor));
        // Resized events are the authoritative expanded content size. The
        // Win32 outer rect can include an invisible DWM frame even for this
        // frameless window; re-saving that outer size during Orb/mode changes
        // would grow the stored DIP geometry on every transition.
        stored.monitor_identity = monitor_identity;
    })?;
    Ok(())
}

fn apply_saved_floating_rect(
    window: &WebviewWindow,
    settings: &ProductSettings,
) -> Result<(), String> {
    match settings.floating_presentation {
        FloatingPresentation::Collapsed => apply_orb_rect(window, settings),
        FloatingPresentation::Expanded => apply_saved_main_rect(window, settings),
    }
}

fn apply_orb_rect(window: &WebviewWindow, settings: &ProductSettings) -> Result<(), String> {
    let areas = work_areas(window)?;
    let fallback = areas
        .first()
        .cloned()
        .ok_or_else(|| "no display work area available".to_string())?;
    let rect = orb_rect(settings, &areas, &fallback);
    let target = select_orb_work_area(settings, &areas).unwrap_or_else(|| fallback.clone());
    eprintln!(
        "[floating-orb] apply collapsed · logical={}x{} DIP · scale={:.3} · physical={},{} {}x{} · locked={} · topmost={}",
        ORB_SIZE_DIP,
        ORB_SIZE_DIP,
        normalized_scale_factor(target.scale_factor),
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        settings.locked,
        settings.always_on_top,
    );
    window
        .set_size(PhysicalSize::new(rect.width, rect.height))
        .and_then(|_| window.set_position(PhysicalPosition::new(rect.x, rect.y)))
        .map_err(|error| error.to_string())
}

fn orb_rect(settings: &ProductSettings, areas: &[WorkArea], fallback: &WorkArea) -> WindowRect {
    let target = select_orb_work_area(settings, areas).unwrap_or_else(|| fallback.clone());
    let scale = normalized_scale_factor(target.scale_factor);
    let size = logical_u32_to_physical(ORB_SIZE_DIP, scale)
        .min(target.width)
        .min(target.height);
    let margin = logical_i32_to_physical(ORB_MARGIN_DIP, scale);
    let requested_x = settings
        .floating_orb_x
        .map(|value| logical_i32_to_physical(value, scale))
        .unwrap_or(target.x + target.width as i32 - size as i32 - margin);
    let requested_y = settings
        .floating_orb_y
        .map(|value| logical_i32_to_physical(value, scale))
        .unwrap_or(target.y + target.height as i32 - size as i32 - margin);
    WindowRect {
        x: requested_x.clamp(target.x, target.x + target.width as i32 - size as i32),
        y: requested_y.clamp(target.y, target.y + target.height as i32 - size as i32),
        width: size,
        height: size,
    }
}

fn smart_expanded_rect(
    settings: &ProductSettings,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    let target = select_orb_work_area(settings, areas).unwrap_or_else(|| fallback.clone());
    let orb = orb_rect(settings, areas, fallback);
    let scale = normalized_scale_factor(target.scale_factor);
    let width = logical_u32_to_physical(settings.width.clamp(360, 1100), scale).min(target.width);
    let height =
        logical_u32_to_physical(settings.height.clamp(500, 1200), scale).min(target.height);
    let orb_center_x = orb.x + orb.width as i32 / 2;
    let orb_center_y = orb.y + orb.height as i32 / 2;
    let area_center_x = target.x + target.width as i32 / 2;
    let area_center_y = target.y + target.height as i32 / 2;
    let requested_x = if orb_center_x >= area_center_x {
        orb.x + orb.width as i32 - width as i32
    } else {
        orb.x
    };
    let requested_y = if orb_center_y >= area_center_y {
        orb.y + orb.height as i32 - height as i32
    } else {
        orb.y
    };
    WindowRect {
        x: requested_x.clamp(target.x, target.x + target.width as i32 - width as i32),
        y: requested_y.clamp(target.y, target.y + target.height as i32 - height as i32),
        width,
        height,
    }
}

fn desktop_request_from_settings(
    settings: &ProductSettings,
    scale_factor: f64,
) -> window_mode::DesktopWidgetRequest {
    let logical_width = settings.desktop_width.clamp(360, 1100);
    let logical_height = settings.desktop_height.clamp(500, 1200);
    let logical_bounds = match (settings.desktop_x, settings.desktop_y) {
        (Some(x), Some(y)) => format!("{x},{y},{logical_width}x{logical_height} DIP"),
        _ => format!("auto-right,{logical_width}x{logical_height} DIP"),
    };
    let bounds = match (settings.desktop_x, settings.desktop_y) {
        (Some(x), Some(y)) => Some(logical_bounds_to_physical(
            window_mode::DesktopBounds {
                x,
                y,
                width: logical_width,
                height: logical_height,
            },
            scale_factor,
        )),
        _ => None,
    };
    window_mode::DesktopWidgetRequest {
        bounds,
        default_width: logical_u32_to_physical(logical_width, scale_factor),
        default_height: logical_u32_to_physical(logical_height, scale_factor),
        margin: logical_i32_to_physical(32, scale_factor),
        tauri_scale_factor: scale_factor,
        logical_bounds,
    }
}

fn save_current_desktop_rect(window: &WebviewWindow, app_state: &AppState) -> Result<(), String> {
    let physical = window_mode::current_desktop_bounds(window)?;
    let scale_factor = normalized_scale_factor(window_mode::desktop_window_scale_factor(window)?);
    let bounds = physical_bounds_to_logical(physical, scale_factor);
    app_state.update(|stored| {
        stored.desktop_x = Some(bounds.x);
        stored.desktop_y = Some(bounds.y);
        stored.desktop_width = bounds.width;
        stored.desktop_height = bounds.height;
    })?;
    Ok(())
}

fn ensure_logical_geometry(window: &WebviewWindow, app_state: &AppState) -> Result<(), String> {
    if app_state.snapshot()?.geometry_units_version >= 1 {
        return Ok(());
    }
    let scale_factor =
        normalized_scale_factor(window.scale_factor().map_err(|error| error.to_string())?);
    app_state.update(|stored| migrate_geometry_values_to_dip(stored, scale_factor))?;
    eprintln!(
        "[window-geometry] migrated legacy Floating/Sidebar physical values to DIP at scale {scale_factor:.3}; Desktop values retained as intended DIP"
    );
    Ok(())
}

fn migrate_geometry_values_to_dip(settings: &mut ProductSettings, scale_factor: f64) {
    // Before the unit contract was explicit, Floating and Sidebar values came
    // directly from Tauri physical events. Convert those once so a 150%
    // display keeps the same visible rect after switching to DIP.
    settings.x = settings
        .x
        .map(|value| physical_i32_to_logical(value, scale_factor));
    settings.y = settings
        .y
        .map(|value| physical_i32_to_logical(value, scale_factor));
    settings.width = physical_u32_to_logical(settings.width, scale_factor);
    settings.height = physical_u32_to_logical(settings.height, scale_factor);
    settings.sidebar_width = physical_u32_to_logical(settings.sidebar_width, scale_factor);
    // Desktop fields were introduced as logical product defaults but were
    // accidentally passed straight to SetWindowPos. Their numeric values
    // already express the intended DIP size and must not be divided here.
    settings.geometry_units_version = 1;
}

fn normalized_scale_factor(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

fn logical_i32_to_physical(value: i32, scale_factor: f64) -> i32 {
    (value as f64 * normalized_scale_factor(scale_factor)).round() as i32
}

fn logical_u32_to_physical(value: u32, scale_factor: f64) -> u32 {
    (value as f64 * normalized_scale_factor(scale_factor))
        .round()
        .max(1.0) as u32
}

fn physical_i32_to_logical(value: i32, scale_factor: f64) -> i32 {
    (value as f64 / normalized_scale_factor(scale_factor)).round() as i32
}

fn physical_u32_to_logical(value: u32, scale_factor: f64) -> u32 {
    (value as f64 / normalized_scale_factor(scale_factor))
        .round()
        .max(1.0) as u32
}

fn logical_bounds_to_physical(
    bounds: window_mode::DesktopBounds,
    scale_factor: f64,
) -> window_mode::DesktopBounds {
    window_mode::DesktopBounds {
        x: logical_i32_to_physical(bounds.x, scale_factor),
        y: logical_i32_to_physical(bounds.y, scale_factor),
        width: logical_u32_to_physical(bounds.width, scale_factor),
        height: logical_u32_to_physical(bounds.height, scale_factor),
    }
}

fn physical_bounds_to_logical(
    bounds: window_mode::DesktopBounds,
    scale_factor: f64,
) -> window_mode::DesktopBounds {
    window_mode::DesktopBounds {
        x: physical_i32_to_logical(bounds.x, scale_factor),
        y: physical_i32_to_logical(bounds.y, scale_factor),
        width: physical_u32_to_logical(bounds.width, scale_factor),
        height: physical_u32_to_logical(bounds.height, scale_factor),
    }
}

fn apply_saved_main_rect(window: &WebviewWindow, settings: &ProductSettings) -> Result<(), String> {
    let areas = work_areas(window)?;
    let fallback = areas
        .first()
        .cloned()
        .ok_or_else(|| "no display work area available".to_string())?;
    let target = select_work_area(settings, &areas).unwrap_or_else(|| fallback.clone());
    let scale_factor = normalized_scale_factor(target.scale_factor);
    let saved = WindowRect {
        x: settings
            .x
            .map(|x| logical_i32_to_physical(x, scale_factor))
            .unwrap_or(target.x + logical_i32_to_physical(48, scale_factor)),
        y: settings
            .y
            .map(|y| logical_i32_to_physical(y, scale_factor))
            .unwrap_or(target.y + logical_i32_to_physical(48, scale_factor)),
        width: logical_u32_to_physical(settings.width.clamp(360, 1100), scale_factor),
        height: logical_u32_to_physical(settings.height.clamp(500, 1200), scale_factor),
    };
    let rect = validate_window_rect(saved, &areas, &fallback);
    window
        .set_size(PhysicalSize::new(rect.width, rect.height))
        .and_then(|_| window.set_position(PhysicalPosition::new(rect.x, rect.y)))
        .map_err(|error| error.to_string())
}

fn apply_sidebar_rect(window: &WebviewWindow, settings: &ProductSettings) -> Result<(), String> {
    let areas = work_areas(window)?;
    let area = select_work_area(settings, &areas)
        .or_else(|| areas.first().cloned())
        .ok_or_else(|| "no display work area available".to_string())?;
    let width = logical_u32_to_physical(settings.sidebar_width.clamp(320, 560), area.scale_factor)
        .min(area.width);
    let x = match settings.sidebar_side {
        SidebarSide::Left => area.x,
        SidebarSide::Right => area.x + area.width as i32 - width as i32,
    };
    window
        .set_size(PhysicalSize::new(width, area.height))
        .and_then(|_| window.set_position(PhysicalPosition::new(x, area.y)))
        .map_err(|error| error.to_string())
}

fn work_areas(window: &WebviewWindow) -> Result<Vec<WorkArea>, String> {
    window
        .available_monitors()
        .map_err(|error| error.to_string())
        .map(|monitors| {
            monitors
                .into_iter()
                .map(|monitor| WorkArea {
                    identity: monitor.name().cloned(),
                    x: monitor.work_area().position.x,
                    y: monitor.work_area().position.y,
                    width: monitor.work_area().size.width,
                    height: monitor.work_area().size.height,
                    scale_factor: monitor.scale_factor(),
                })
                .collect()
        })
}

fn select_work_area(settings: &ProductSettings, areas: &[WorkArea]) -> Option<WorkArea> {
    // Saved monitor identity wins over saved coordinates: a monitor that was
    // unplugged and reattached, or a layout that changed the origin, moves every
    // coordinate while the identity stays stable. Coordinates are the fallback
    // for a document written before the identity was recorded.
    if let Some(identity) = settings.monitor_identity.as_deref() {
        if let Some(area) = areas
            .iter()
            .find(|area| area.identity.as_deref() == Some(identity))
        {
            return Some(area.clone());
        }
    }
    if let (Some(x), Some(y)) = (settings.x, settings.y) {
        areas
            .iter()
            .find(|area| {
                point_in_area(
                    logical_i32_to_physical(x, area.scale_factor),
                    logical_i32_to_physical(y, area.scale_factor),
                    area,
                )
            })
            .cloned()
    } else {
        None
    }
}

fn select_orb_work_area(settings: &ProductSettings, areas: &[WorkArea]) -> Option<WorkArea> {
    if let Some(identity) = settings.floating_orb_monitor_identity.as_deref() {
        if let Some(area) = areas
            .iter()
            .find(|area| area.identity.as_deref() == Some(identity))
        {
            return Some(area.clone());
        }
    }
    if let (Some(x), Some(y)) = (settings.floating_orb_x, settings.floating_orb_y) {
        areas
            .iter()
            .find(|area| {
                point_in_area(
                    logical_i32_to_physical(x, area.scale_factor),
                    logical_i32_to_physical(y, area.scale_factor),
                    area,
                )
            })
            .cloned()
    } else {
        None
    }
}

fn point_in_area(x: i32, y: i32, area: &WorkArea) -> bool {
    x >= area.x && x < area.x + area.width as i32 && y >= area.y && y < area.y + area.height as i32
}

fn validate_window_rect(saved: WindowRect, areas: &[WorkArea], fallback: &WorkArea) -> WindowRect {
    // Require more than a one-pixel intersection: disconnected displays often
    // leave a technically intersecting resize border that users cannot grab.
    // An 80x80 area keeps enough title/content surface available for recovery.
    if areas
        .iter()
        .any(|area| visible_intersection(saved, area) >= MIN_VISIBLE * MIN_VISIBLE)
    {
        return saved;
    }
    let width = saved.width.min(fallback.width.saturating_sub(32)).max(360);
    let height = saved
        .height
        .min(fallback.height.saturating_sub(32))
        .max(500);
    WindowRect {
        x: fallback.x + 32,
        y: fallback.y + 32,
        width,
        height,
    }
}

fn visible_intersection(window: WindowRect, area: &WorkArea) -> i32 {
    let left = window.x.max(area.x);
    let top = window.y.max(area.y);
    let right = (window.x + window.width as i32).min(area.x + area.width as i32);
    let bottom = (window.y + window.height as i32).min(area.y + area.height as i32);
    (right - left).max(0) * (bottom - top).max(0)
}

fn snap_side(
    position: PhysicalPosition<i32>,
    window_width: u32,
    areas: &[WorkArea],
) -> Option<SidebarSide> {
    areas.iter().find_map(|area| {
        if position.y < area.y - SNAP_THRESHOLD
            || position.y > area.y + area.height as i32 + SNAP_THRESHOLD
        {
            return None;
        }
        if (position.x - area.x).abs() <= SNAP_THRESHOLD {
            Some(SidebarSide::Left)
        } else if (position.x + window_width as i32 - (area.x + area.width as i32)).abs()
            <= SNAP_THRESHOLD
        {
            Some(SidebarSide::Right)
        } else {
            None
        }
    })
}

pub fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    let app = window.app_handle();
    let Some(webview) = app.get_webview_window(window.label()) else {
        return;
    };
    let state = app.state::<AppState>();
    let runtime = app.state::<ProductWindowRuntime>();
    if runtime.applying.load(Ordering::Acquire) {
        return;
    }
    let Ok(settings) = state.snapshot() else {
        return;
    };

    match event {
        WindowEvent::Focused(active) => {
            #[cfg(target_os = "windows")]
            platform::windows::composition_host::set_input_active(&runtime.native_material, *active);
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            runtime.shutdown_native_material();
        }
        WindowEvent::Moved(position) => {
            // Win32 can deliver a queued move from the previous presentation
            // after an Orb/mode transition has already moved the same HWND.
            // Persist only an event that still describes the current rect;
            // otherwise an expanded/Desktop position can overwrite the Orb
            // anchor and cause drift on the next collapse.
            if webview
                .outer_position()
                .is_ok_and(|current| current != *position)
            {
                return;
            }
            if settings.mode == ProductWindowMode::Floating
                && settings.floating_presentation == FloatingPresentation::Expanded
                && !settings.locked
            {
                if let Ok(areas) = work_areas(&webview) {
                    let width = webview.outer_size().map(|size| size.width).unwrap_or(0);
                    if let Some(side) = snap_side(*position, width, &areas) {
                        let _ = state.update(|stored| stored.sidebar_side = side);
                        let native = app.state::<NativeWindowState>();
                        let _ = apply_product_mode(
                            &webview,
                            &state,
                            &native,
                            &runtime,
                            ProductWindowMode::Sidebar,
                        );
                        return;
                    }
                }
            }
            if settings.mode == ProductWindowMode::Floating {
                let scale_factor = webview
                    .scale_factor()
                    .map(normalized_scale_factor)
                    .unwrap_or(1.0);
                let monitor_identity = webview
                    .current_monitor()
                    .ok()
                    .flatten()
                    .and_then(|monitor| monitor.name().cloned());
                let _ = state.update(|stored| match settings.floating_presentation {
                    FloatingPresentation::Collapsed => {
                        stored.floating_orb_x =
                            Some(physical_i32_to_logical(position.x, scale_factor));
                        stored.floating_orb_y =
                            Some(physical_i32_to_logical(position.y, scale_factor));
                        stored.floating_orb_monitor_identity = monitor_identity;
                    }
                    FloatingPresentation::Expanded => {
                        stored.x = Some(physical_i32_to_logical(position.x, scale_factor));
                        stored.y = Some(physical_i32_to_logical(position.y, scale_factor));
                        stored.monitor_identity = monitor_identity;
                    }
                });
            } else if settings.mode == ProductWindowMode::Desktop {
                let _ = save_current_desktop_rect(&webview, &state);
                // WebView2 positions its own window from the parent window's client
                // origin, so a move — including the lifecycle recovery write that
                // bypasses `apply_product_mode` — can put it back over the widget.
                #[cfg(target_os = "windows")]
                if runtime.composition_hosting {
                    let _ = platform::windows::composition_host::sync_input_target(
                        &runtime.native_material,
                        true,
                    );
                }
            }
        }
        WindowEvent::Resized(size) => {
            // The same stale-event rule protects expanded size from a queued
            // 56 DIP Orb resize (and vice versa). Without it a transition's own
            // resize event, delivered after the next presentation has already
            // applied its rect, persists the wrong size and the widget grows or
            // shrinks on every collapse/expand cycle.
            if webview.inner_size().is_ok_and(|current| current != *size) {
                return;
            }
            match settings.mode {
                ProductWindowMode::Sidebar => {
                    let scale_factor = webview
                        .scale_factor()
                        .map(normalized_scale_factor)
                        .unwrap_or(1.0);
                    let _ = state.update(|stored| {
                        stored.sidebar_width =
                            physical_u32_to_logical(size.width, scale_factor).clamp(320, 560)
                    });
                }
                ProductWindowMode::Floating => {
                    let scale_factor = webview
                        .scale_factor()
                        .map(normalized_scale_factor)
                        .unwrap_or(1.0);
                    if settings.floating_presentation == FloatingPresentation::Expanded {
                        let _ = state.update(|stored| {
                            stored.width = physical_u32_to_logical(size.width, scale_factor);
                            stored.height = physical_u32_to_logical(size.height, scale_factor);
                        });
                    }
                }
                ProductWindowMode::Desktop => {
                    let _ = save_current_desktop_rect(&webview, &state);
                }
            }
        }
        _ => {}
    }
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
    use super::{
        desktop_request_from_settings, logical_bounds_to_physical, migrate_geometry_values_to_dip,
        orb_rect, physical_bounds_to_logical, product_window_resizable, select_work_area,
        smart_expanded_rect, snap_side, validate_quick_links, validate_window_rect,
        window_drag_allowed, WindowRect, WorkArea, ORB_SIZE_DIP,
    };
    use crate::settings::{FloatingPresentation, ProductSettings, ProductWindowMode, SidebarSide};
    use crate::window_mode::DesktopBounds;
    use tauri::PhysicalPosition;

    const PRIMARY: WorkArea = WorkArea {
        identity: None,
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
        scale_factor: 1.0,
    };

    /// Regression guard for RC-0: the sidebar's width *is* the persisted
    /// `sidebarWidth`, so the window must expose a sizing border in Sidebar mode.
    /// It previously reported `false` there, which left the frameless window with
    /// no grabbable edge and froze the setting at its last stored value.
    #[test]
    fn sidebar_is_resizable_and_other_presentations_stay_fixed() {
        let expanded = ProductSettings {
            floating_presentation: FloatingPresentation::Expanded,
            locked: false,
            ..ProductSettings::default()
        };
        assert!(product_window_resizable(
            ProductWindowMode::Sidebar,
            &expanded
        ));

        // The lock guards free repositioning, not the edge-anchored width.
        let locked = ProductSettings {
            locked: true,
            ..expanded.clone()
        };
        assert!(product_window_resizable(ProductWindowMode::Sidebar, &locked));

        assert!(product_window_resizable(
            ProductWindowMode::Floating,
            &expanded
        ));
        assert!(!product_window_resizable(
            ProductWindowMode::Floating,
            &locked
        ));

        let collapsed = ProductSettings {
            floating_presentation: FloatingPresentation::Collapsed,
            ..expanded
        };
        assert!(!product_window_resizable(
            ProductWindowMode::Floating,
            &collapsed
        ));
        // The Desktop widget is a frameless child, so it used to be fixed-size.
        // v1 usability makes it resizable while unlocked, through the same
        // undecorated resize borders Sidebar uses.
        assert!(product_window_resizable(
            ProductWindowMode::Desktop,
            &collapsed
        ));
        let desktop_locked = ProductSettings {
            mode: ProductWindowMode::Desktop,
            locked: true,
            ..ProductSettings::default()
        };
        assert!(!product_window_resizable(
            ProductWindowMode::Desktop,
            &desktop_locked
        ));
    }

    #[test]
    fn locked_desktop_is_neither_draggable_nor_resizable() {
        let desktop = ProductSettings {
            mode: ProductWindowMode::Desktop,
            ..ProductSettings::default()
        };
        assert!(window_drag_allowed(&desktop));
        assert!(product_window_resizable(ProductWindowMode::Desktop, &desktop));

        let locked = ProductSettings {
            locked: true,
            ..desktop
        };
        assert!(!window_drag_allowed(&locked));
        assert!(!product_window_resizable(ProductWindowMode::Desktop, &locked));
    }

    #[test]
    fn preserves_visible_saved_rect() {
        let saved = WindowRect {
            x: 120,
            y: 80,
            width: 620,
            height: 720,
        };
        assert_eq!(validate_window_rect(saved, &[PRIMARY], &PRIMARY), saved);
    }

    #[test]
    fn recovers_fully_offscreen_rect_to_primary_work_area() {
        let saved = WindowRect {
            x: 5000,
            y: 5000,
            width: 620,
            height: 720,
        };
        assert_eq!(
            validate_window_rect(saved, &[PRIMARY], &PRIMARY),
            WindowRect {
                x: 32,
                y: 32,
                width: 620,
                height: 720
            }
        );
    }

    #[test]
    fn detects_left_and_right_edge_snap() {
        assert_eq!(
            snap_side(PhysicalPosition::new(8, 200), 620, &[PRIMARY]),
            Some(SidebarSide::Left)
        );
        assert_eq!(
            snap_side(PhysicalPosition::new(1292, 200), 620, &[PRIMARY]),
            Some(SidebarSide::Right)
        );
    }

    #[test]
    fn monitor_identity_wins_over_stale_coordinates() {
        let secondary = WorkArea {
            identity: Some("DISPLAY-2".into()),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1040,
            scale_factor: 1.0,
        };
        let settings = ProductSettings {
            monitor_identity: Some("DISPLAY-2".into()),
            x: Some(200),
            y: Some(200),
            ..ProductSettings::default()
        };
        assert_eq!(
            select_work_area(&settings, &[PRIMARY, secondary.clone()]),
            Some(secondary)
        );
    }

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

    #[test]
    fn native_window_drag_is_eligible_for_unlocked_floating_and_desktop() {
        let mut settings = ProductSettings::default();
        assert!(window_drag_allowed(&settings));

        settings.locked = true;
        assert!(!window_drag_allowed(&settings));

        settings.locked = false;
        settings.mode = ProductWindowMode::Sidebar;
        assert!(!window_drag_allowed(&settings));

        settings.mode = ProductWindowMode::Desktop;
        assert!(window_drag_allowed(&settings));
    }

    #[test]
    fn desktop_geometry_is_independent_from_floating_geometry() {
        let settings = ProductSettings {
            x: Some(120),
            y: Some(80),
            width: 620,
            height: 720,
            desktop_x: Some(1400),
            desktop_y: Some(32),
            desktop_width: 420,
            desktop_height: 700,
            ..ProductSettings::default()
        };
        assert_eq!(
            desktop_request_from_settings(&settings, 1.5).bounds,
            Some(DesktopBounds {
                x: 2100,
                y: 48,
                width: 630,
                height: 1050,
            })
        );
        assert_eq!((settings.x, settings.y), (Some(120), Some(80)));
        assert_eq!((settings.width, settings.height), (620, 720));
    }

    #[test]
    fn desktop_dip_conversion_covers_common_windows_scaling_levels() {
        let logical = DesktopBounds {
            x: 40,
            y: 32,
            width: 420,
            height: 700,
        };
        for (scale, expected) in [
            (1.0, (420, 700)),
            (1.25, (525, 875)),
            (1.5, (630, 1050)),
            (2.0, (840, 1400)),
        ] {
            let physical = logical_bounds_to_physical(logical, scale);
            assert_eq!((physical.width, physical.height), expected);
            assert_eq!(physical_bounds_to_logical(physical, scale), logical);
        }
    }

    #[test]
    fn orb_size_is_exactly_56_dip_at_common_windows_scaling_levels() {
        for (scale, expected) in [(1.0, 56), (1.25, 70), (1.5, 84), (2.0, 112)] {
            let area = WorkArea {
                scale_factor: scale,
                ..PRIMARY
            };
            let rect = orb_rect(
                &ProductSettings::default(),
                std::slice::from_ref(&area),
                &area,
            );
            assert_eq!(ORB_SIZE_DIP, 56);
            assert_eq!((rect.width, rect.height), (expected, expected));
        }
    }

    #[test]
    fn smart_expansion_keeps_the_orb_corner_anchor_and_stays_visible() {
        let cases = [
            ((40, 40), (40, 40)),
            ((1824, 40), (1260, 40)),
            ((40, 944), (40, 280)),
            ((1824, 944), (1260, 280)),
        ];
        for ((orb_x, orb_y), expected_origin) in cases {
            let settings = ProductSettings {
                floating_orb_x: Some(orb_x),
                floating_orb_y: Some(orb_y),
                ..ProductSettings::default()
            };
            let expanded = smart_expanded_rect(&settings, &[PRIMARY], &PRIMARY);
            assert_eq!((expanded.x, expanded.y), expected_origin);
            assert_eq!((expanded.width, expanded.height), (620, 720));
            assert!(expanded.x >= PRIMARY.x && expanded.y >= PRIMARY.y);
            assert!(expanded.x + expanded.width as i32 <= PRIMARY.x + PRIMARY.width as i32);
            assert!(expanded.y + expanded.height as i32 <= PRIMARY.y + PRIMARY.height as i32);
        }
    }

    #[test]
    fn legacy_physical_geometry_migrates_without_shrinking_desktop_dip_defaults() {
        let mut settings = ProductSettings {
            geometry_units_version: 0,
            x: Some(711),
            y: Some(422),
            width: 642,
            height: 733,
            sidebar_width: 570,
            desktop_x: Some(1400),
            desktop_y: Some(32),
            desktop_width: 420,
            desktop_height: 700,
            ..ProductSettings::default()
        };
        migrate_geometry_values_to_dip(&mut settings, 1.5);
        assert_eq!(settings.geometry_units_version, 1);
        assert_eq!((settings.x, settings.y), (Some(474), Some(281)));
        assert_eq!((settings.width, settings.height), (428, 489));
        assert_eq!(settings.sidebar_width, 380);
        assert_eq!(
            (settings.desktop_width, settings.desktop_height),
            (420, 700)
        );
        assert_eq!(
            (settings.desktop_x, settings.desktop_y),
            (Some(1400), Some(32))
        );
    }
}
