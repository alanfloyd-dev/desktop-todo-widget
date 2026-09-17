use crate::{
    appearance::{self, AppearanceSettings},
    settings::{
        self, AppState, FloatingPresentation, ProductSettings, ProductWindowMode, SidebarSide,
    },
    window_mode::{self, NativeWindowState},
};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

mod geometry;

use self::geometry::{
    desktop_request_from_settings, logical_i32_to_physical, logical_u32_to_physical,
    migrate_geometry_values_to_dip, normalized_scale_factor, orb_rect, physical_bounds_to_logical,
    physical_i32_to_logical, physical_u32_to_logical, select_orb_work_area, select_work_area,
    smart_expanded_rect, snap_side, validate_window_rect, WindowRect, WorkArea,
};

pub const ORB_SIZE_DIP: u32 = 56;

#[derive(Default)]
pub struct ProductWindowRuntime {
    applying: AtomicBool,
}

impl ProductWindowRuntime {
    pub fn new() -> Self {
        Self {
            applying: AtomicBool::new(false),
        }
    }
}

struct ApplyingGuard<'a>(&'a AtomicBool);

impl Drop for ApplyingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) fn apply_native_composition(
    window: &WebviewWindow,
    mode: ProductWindowMode,
    presentation: FloatingPresentation,
    appearance: &AppearanceSettings,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // One native-effect policy for every mode and material: the window
        // stays transparent and the CSS material layers own the look. The
        // former acrylic window request rendered as flat gray on systems
        // where the backdrop source fails, so no native effect is applied.
        let requested_glass = appearance.background_type == appearance::BackgroundType::Glass;
        let fallback = if mode == ProductWindowMode::Desktop && requested_glass {
            // Desktop is child-hosted under the shell; a `SHELLDLL_DefView`
            // child is not a top-level HWND and cannot host backdrop effects.
            // See docs/desktop-mode.md.
            "translucent-graphite"
        } else if mode == ProductWindowMode::Floating
            && presentation == FloatingPresentation::Collapsed
            && requested_glass
        {
            "transparent-orb-tint"
        } else {
            "selected-css-material"
        };
        window
            .app_handle()
            .state::<crate::qa_diagnostics::QaDiagnostics>()
            .record(format!(
                "native_effects_cleared=true requested_glass={requested_glass} backend=standard css={fallback}"
            ));
        window
            .set_effects(None)
            .map_err(|error| error.to_string())?;
        eprintln!(
            "[appearance-composition] mode={mode:?} · webview=transparent · native=none · css={fallback}"
        );
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, mode, presentation, appearance);
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
        "[window-drag] requested=true mode={:?} locked={}",
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
            mode,
            settings.floating_presentation,
            settings.appearance_profiles.for_mode(mode),
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
            // The expanded window reopens at its own saved position: the Orb
            // anchor is a separate geometry family and never redefines the
            // Expanded rect. `expanded_restore_rect` revalidates the saved
            // rect against the current monitor topology and only falls back
            // to the Orb-derived placement when no position was ever saved.
            let rect = expanded_restore_rect(&settings, &areas, &fallback);
            eprintln!(
                "[floating-orb] apply expanded · source={} · logical={}x{} DIP · physical={},{} {}x{} · anchor={:?},{:?} · locked={} · topmost={}",
                if settings.x.is_some() && settings.y.is_some() {
                    "saved"
                } else {
                    "orb-fallback"
                },
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

/// Writes one Expanded position triple (`x`, `y`, `monitor_identity`) into
/// settings, converting the physical screen position to logical DIP. The three
/// fields can only ever change together through here; guards, value sourcing,
/// and `AppState::update` timing stay with the callers. This never writes
/// `width`/`height` — size has exactly one writer, the `Resized` handler.
fn persist_expanded_position(
    stored: &mut ProductSettings,
    position: PhysicalPosition<i32>,
    scale_factor: f64,
    monitor_identity: Option<String>,
) {
    stored.x = Some(physical_i32_to_logical(position.x, scale_factor));
    stored.y = Some(physical_i32_to_logical(position.y, scale_factor));
    stored.monitor_identity = monitor_identity;
}

/// The Orb counterpart of [`persist_expanded_position`], for the collapsed
/// Floating presentation's own triple (`floating_orb_x`, `floating_orb_y`,
/// `floating_orb_monitor_identity`). Deliberately a separate entry point: the
/// Orb anchor is an independent saved value, not a second name for the
/// Expanded position.
fn persist_orb_position(
    stored: &mut ProductSettings,
    position: PhysicalPosition<i32>,
    scale_factor: f64,
    monitor_identity: Option<String>,
) {
    stored.floating_orb_x = Some(physical_i32_to_logical(position.x, scale_factor));
    stored.floating_orb_y = Some(physical_i32_to_logical(position.y, scale_factor));
    stored.floating_orb_monitor_identity = monitor_identity;
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
        persist_expanded_position(stored, position, scale_factor, monitor_identity);
        // Resized events are the authoritative expanded content size. The
        // Win32 outer rect can include an invisible DWM frame even for this
        // frameless window; re-saving that outer size during Orb/mode changes
        // would grow the stored DIP geometry on every transition.
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

/// The physical rect that restores the saved Expanded geometry: the saved
/// monitor (identity first, coordinates as fallback) supplies the scale, the
/// DIP values convert, and `validate_window_rect` provides the existing
/// off-screen / topology-change recovery. Pure: no window access, no writes.
fn saved_expanded_rect(
    settings: &ProductSettings,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    let target = select_work_area(settings, areas).unwrap_or_else(|| fallback.clone());
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
    validate_window_rect(saved, areas, fallback)
}

/// The rect an expand applies. A document with a saved Expanded position
/// restores exactly that rect; only a document that has never saved one
/// (legacy / first collapse never taken) falls back to the Orb-derived smart
/// placement. The Orb anchor never redefines the Expanded position.
fn expanded_restore_rect(
    settings: &ProductSettings,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    if settings.x.is_some() && settings.y.is_some() {
        saved_expanded_rect(settings, areas, fallback)
    } else {
        smart_expanded_rect(settings, areas, fallback)
    }
}

fn apply_saved_main_rect(window: &WebviewWindow, settings: &ProductSettings) -> Result<(), String> {
    let areas = work_areas(window)?;
    let fallback = areas
        .first()
        .cloned()
        .ok_or_else(|| "no display work area available".to_string())?;
    let rect = saved_expanded_rect(settings, &areas, &fallback);
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
                        persist_orb_position(stored, *position, scale_factor, monitor_identity);
                    }
                    FloatingPresentation::Expanded => {
                        persist_expanded_position(stored, *position, scale_factor, monitor_identity);
                    }
                });
            } else if settings.mode == ProductWindowMode::Desktop {
                let _ = save_current_desktop_rect(&webview, &state);
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

#[cfg(test)]
mod tests {
    use super::{product_window_resizable, window_drag_allowed};
    use super::geometry::WorkArea;
    use crate::settings::{FloatingPresentation, ProductSettings, ProductWindowMode};
    use tauri::PhysicalPosition;

    const PRIMARY: WorkArea = WorkArea {
        identity: None,
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
        scale_factor: 1.0,
    };

    /// The bug contract: expanding reopens the Expanded window at its own
    /// saved rect even though the Orb was dragged elsewhere in between, and
    /// the saved width/height pass through untouched. The Orb anchor never
    /// redefines the Expanded position.
    #[test]
    fn expand_restores_the_saved_rect_even_after_the_orb_moved() {
        let mut settings = ProductSettings::default();
        settings.x = Some(120);
        settings.y = Some(80);
        settings.width = 800;
        settings.height = 900;
        settings.monitor_identity = Some("DISPLAY-1".into());
        // Collapse saved the expanded rect; the user then dragged the Orb to
        // the far corner, which may only move the Orb fields.
        super::persist_orb_position(
            &mut settings,
            PhysicalPosition::new(1800, 900),
            1.0,
            Some("DISPLAY-1".into()),
        );
        let rect = super::expanded_restore_rect(&settings, &[PRIMARY], &PRIMARY);
        assert_eq!((rect.x, rect.y), (120, 80));
        assert_eq!((rect.width, rect.height), (800, 900));
    }

    /// A saved rect that is no longer visible (monitor removed / topology
    /// change) still goes through the existing visibility validation and
    /// lands on the recovery placement instead of the saved coordinates.
    #[test]
    fn expand_falls_back_when_the_saved_rect_is_no_longer_visible() {
        let mut settings = ProductSettings::default();
        settings.x = Some(5000);
        settings.y = Some(5000);
        let rect = super::expanded_restore_rect(&settings, &[PRIMARY], &PRIMARY);
        assert_eq!((rect.x, rect.y), (32, 32));
        assert_eq!((rect.width, rect.height), (620, 720));
    }

    /// Without a saved Expanded position the historical Orb-derived smart
    /// placement remains exactly as before — as the fallback only.
    #[test]
    fn expand_without_a_saved_position_keeps_the_orb_derived_placement() {
        let mut settings = ProductSettings::default();
        settings.floating_orb_x = Some(1824);
        settings.floating_orb_y = Some(944);
        let rect = super::expanded_restore_rect(&settings, &[PRIMARY], &PRIMARY);
        assert_eq!(rect, super::smart_expanded_rect(&settings, &[PRIMARY], &PRIMARY));
        assert_eq!((rect.x, rect.y), (1260, 280));
    }

    /// The Expanded position triple has one mutation shape: `x`, `y`, and
    /// `monitor_identity` move together or not at all, and a position helper
    /// never touches size or the Orb family.
    #[test]
    fn expanded_position_helper_writes_the_triple_together() {
        let mut stored = ProductSettings::default();
        super::persist_expanded_position(
            &mut stored,
            PhysicalPosition::new(300, 150),
            1.5,
            Some("DISPLAY-2".into()),
        );
        assert_eq!((stored.x, stored.y), (Some(200), Some(100)));
        assert_eq!(stored.monitor_identity.as_deref(), Some("DISPLAY-2"));
        assert_eq!((stored.width, stored.height), (620, 720));
        assert_eq!((stored.floating_orb_x, stored.floating_orb_y), (None, None));
    }

    /// Same rule for the Orb triple, in the other direction: the Expanded
    /// position fields stay untouched.
    #[test]
    fn orb_position_helper_writes_the_triple_together() {
        let mut stored = ProductSettings::default();
        super::persist_orb_position(&mut stored, PhysicalPosition::new(300, 150), 1.5, None);
        assert_eq!(
            (stored.floating_orb_x, stored.floating_orb_y),
            (Some(200), Some(100))
        );
        assert!(stored.floating_orb_monitor_identity.is_none());
        assert_eq!((stored.x, stored.y), (None, None));
        assert!(stored.monitor_identity.is_none());
    }

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
}
