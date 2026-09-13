use crate::{
    locale,
    product_window::{self, ProductViewState, ProductWindowRuntime},
    settings::{AppState, FloatingPresentation, ProductWindowMode, SidebarSide},
    window_mode::NativeWindowState,
};
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WebviewWindow,
};

const TRAY_ID: &str = "alan-desktop-tray";

/// The project page opened by the tray/context-menu `GitHub ↗` action.
///
/// A constant, not a setting: v1 has exactly one project identity and no updater,
/// so the link cannot drift from what the product actually is. It is validated by
/// [`validated_project_url`] before it reaches the shell.
const PROJECT_URL: &str = "https://github.com/alanfloyd-dev/desktop-todo-widget";

/// Only this project page may be launched by the native menus.
///
/// The URL is a compile-time constant, so this is a structural guard rather than
/// input validation: it keeps a future edit from turning a menu item into a
/// generic "open whatever string is here" command.
fn validated_project_url(url: &str) -> bool {
    url == PROJECT_URL && url.starts_with("https://")
}

/// Event name for the authoritative product-state broadcast.
///
/// Native menus — the tray menu and the Orb's context menu, which share
/// `build_tray_menu` — run a product action on the backend and hand *nothing*
/// back to the frontend: the popup command resolves with `()` and the tray
/// handler ignores the dispatch result. Without a publication step the DOM keeps
/// rendering the mode it last fetched (an Orb inside a Sidebar-sized window, for
/// example). Every action dispatched through [`dispatch_product_action`]
/// therefore publishes the resulting `ProductViewState` under this name, and the
/// product window replaces its reactive state with exactly that payload.
///
/// The backend stays the single source of truth: the payload is the same
/// `ProductViewState` the `product_state` command returns, so a native action and
/// a frontend-initiated one cannot produce different views.
pub const PRODUCT_STATE_EVENT: &str = "product-state";

/// Brings the widget back in its current mode.
///
/// With the taskbar exclusion in place the widget has no taskbar button, so this
/// is the only way back to a window the user has minimized or that is covered.
/// It deliberately does **not** change the mode or the presentation: restoring is
/// not a mode switch, so an Orb stays an Orb and a Sidebar stays on its edge — and
/// because it never touches the window flags, it cannot desynchronize the persisted
/// geometry or the native composition state.
fn show_widget(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window unavailable".to_string())?;
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    // A Desktop widget is a `WS_CHILD` of the desktop, so activating it would be
    // meaningless; every other mode is a real top-level window and should take
    // focus, which is what the user expects from "Show widget".
    if app.state::<AppState>().snapshot()?.mode != ProductWindowMode::Desktop {
        window.set_focus().map_err(|error| error.to_string())?;
    }
    // `show`/`set_focus` are tao flag setters and recompute the extended style, so
    // the widget frame is re-asserted here as it is after every product mode
    // transition.
    #[cfg(target_os = "windows")]
    {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        crate::platform::windows::widget_frame::enforce_on(hwnd.0 as isize)?;
    }
    Ok(())
}

/// Opens the project page in the system default browser.
///
/// Reuses the mechanism the product already uses for user-configured links
/// (`product_window::open_shortcut`): the URL is handed to the Shell's own URL
/// handler rather than an HTTP client, so the Windows default browser decides how
/// to open it and no new dependency or in-app web view is introduced.
fn open_project_page() -> Result<(), String> {
    if !validated_project_url(PROJECT_URL) {
        return Err("project URL is not allowed".into());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(PROJECT_URL)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows builds have no product tray today; the action is inert
        // rather than panicking so the command contract stays uniform.
        return Err("opening the project page is only implemented on Windows".into());
    }
    Ok(())
}

#[tauri::command]
pub fn product_action(app: tauri::AppHandle, action: &str) -> Result<ProductViewState, String> {
    dispatch_product_action(&app, action)
}

pub fn dispatch_product_action(
    app: &tauri::AppHandle,
    action: &str,
) -> Result<ProductViewState, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window unavailable".to_string())?;
    let state = app.state::<AppState>();
    let native = app.state::<NativeWindowState>();
    let runtime = app.state::<ProductWindowRuntime>();

    match action {
        "mode.sidebar" => product_window::apply_product_mode(
            &window,
            &state,
            &native,
            &runtime,
            ProductWindowMode::Sidebar,
        )?,
        "mode.floating" => product_window::apply_product_mode(
            &window,
            &state,
            &native,
            &runtime,
            ProductWindowMode::Floating,
        )?,
        "mode.desktop" => product_window::apply_product_mode(
            &window,
            &state,
            &native,
            &runtime,
            ProductWindowMode::Desktop,
        )?,
        "floating.expand" => product_window::set_floating_presentation(
            &window,
            &state,
            &runtime,
            FloatingPresentation::Expanded,
        )?,
        "floating.collapse" => product_window::set_floating_presentation(
            &window,
            &state,
            &runtime,
            FloatingPresentation::Collapsed,
        )?,
        "side.left" | "side.right" => {
            let side = if action == "side.left" {
                SidebarSide::Left
            } else {
                SidebarSide::Right
            };
            state.update(|settings| settings.sidebar_side = side)?;
            if state.snapshot()?.mode == ProductWindowMode::Sidebar {
                product_window::apply_product_mode(
                    &window,
                    &state,
                    &native,
                    &runtime,
                    ProductWindowMode::Sidebar,
                )?;
            }
        }
        "lock.toggle" => {
            let locked = !state.snapshot()?.locked;
            let updated = state.update(|settings| settings.locked = locked)?;
            // Lock guards free positioning and native resizing. Sidebar is always
            // edge-anchored, and its window width *is* the persisted
            // `sidebarWidth`, so it stays resizable regardless of the lock —
            // matching `apply_product_mode`. Desktop follows Floating: resizable
            // exactly while unlocked.
            let resizable = product_window::product_window_resizable(updated.mode, &updated);
            window
                .set_resizable(resizable)
                .map_err(|error| error.to_string())?;
        }
        "always_on_top.toggle" => {
            let snapshot = state.snapshot()?;
            if snapshot.mode == ProductWindowMode::Desktop {
                return Err("Always on top is unavailable in Desktop mode".into());
            }
            let enabled = !snapshot.always_on_top;
            state.update(|settings| settings.always_on_top = enabled)?;
            window
                .set_always_on_top(enabled)
                .map_err(|error| error.to_string())?;
        }
        "settings" => {
            let settings = state.snapshot()?;
            if settings.mode == ProductWindowMode::Floating
                && settings.floating_presentation == FloatingPresentation::Collapsed
            {
                product_window::set_floating_presentation(
                    &window,
                    &state,
                    &runtime,
                    FloatingPresentation::Expanded,
                )?;
            }
            // Settings only exists inside the widget, so reaching it from the tray
            // has to bring the widget back first — the window has no taskbar
            // button to restore it from.
            show_widget(&app)?;
            window
                .emit("open-settings", ())
                .map_err(|error| error.to_string())?;
        }
        "show" => {
            show_widget(&app)?;
        }
        "github" => {
            open_project_page()?;
        }
        "quit" => app.exit(0),
        _ => return Err(format!("unknown product action: {action}")),
    }

    refresh_tray_menu(app)?;
    let settings = state.snapshot()?;
    let view = ProductViewState::new(&state, settings);
    // Single publication point for every product action, whichever entry point
    // ran it: the frontend command, the tray menu, or the Orb's context menu.
    // The frontend replaces its reactive state with this payload, which is what
    // keeps a natively triggered mode/presentation change from desyncing the DOM.
    // Do not remove this emission. Without it the DOM keeps rendering the previous
    // mode/presentation inside the new native geometry, which is the
    // stretched-Orb/state-desync regression.
    if let Err(error) = window.emit(PRODUCT_STATE_EVENT, &view) {
        eprintln!("[product-command] product-state emit failed: {error}");
    }
    Ok(view)
}

#[tauri::command]
pub fn show_product_context_menu(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    let menu = build_tray_menu(&app)?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

/// Installs the resident tray icon.
///
/// The tray is a primary entry point, not a fallback: because no widget mode has a
/// taskbar button or an Alt+Tab entry, the notification area is where the product
/// is found and switched. It stays resident for the whole process lifetime — mode
/// changes only rebuild the menu ([`refresh_tray_menu`]) and never remove or
/// re-create the icon.
pub fn install_tray(app: &tauri::App) -> Result<(), String> {
    let menu = build_tray_menu(app.handle())?;
    let settings = app.state::<AppState>().snapshot().map_err(|error| error.to_string())?;
    let labels = settings
        .language
        .resolve(locale::system_locale())
        .labels();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(labels.product_name)
        // Left click restores the widget, right click opens the menu. That is the
        // standard Windows tray convention and it gives the user a direct way back
        // to a window that has no taskbar button; the menu is still one right
        // click away, and the product context menu inside the widget keeps using
        // the very same menu.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Err(error) = show_widget(tray.app_handle()) {
                    eprintln!("[tray] show widget failed: {error}");
                }
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn refresh_tray_menu(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(build_tray_menu(app)?))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Builds the native menu shared by the tray icon and the widget's own
/// right-click context menu.
///
/// One menu for both surfaces: the tray and the in-widget context menu must never
/// disagree about the current mode, side or lock state, and the product identity
/// heading is useful in both places — in the tray so the user knows what is
/// resident there, and in the context menu so the widget states what it is.
///
/// The headings are disabled items rather than enabled ones: they are identity and
/// state text, not commands.
fn build_tray_menu(app: &tauri::AppHandle) -> Result<tauri::menu::Menu<tauri::Wry>, String> {
    let settings = app.state::<AppState>().snapshot()?;
    let labels = settings
        .language
        .resolve(locale::system_locale())
        .labels();
    let identity = tauri::menu::MenuItemBuilder::with_id("identity", labels.product_name)
        .enabled(false)
        .build(app)
        .map_err(|error| error.to_string())?;
    let github = tauri::menu::MenuItemBuilder::with_id("github", labels.github)
        .build(app)
        .map_err(|error| error.to_string())?;
    let show_widget = tauri::menu::MenuItemBuilder::with_id("show", labels.show_widget)
        .build(app)
        .map_err(|error| error.to_string())?;
    let sidebar = CheckMenuItemBuilder::with_id("mode.sidebar", labels.sidebar)
        .checked(settings.mode == ProductWindowMode::Sidebar)
        .build(app)
        .map_err(|error| error.to_string())?;
    let floating = CheckMenuItemBuilder::with_id("mode.floating", labels.floating)
        .checked(settings.mode == ProductWindowMode::Floating)
        .build(app)
        .map_err(|error| error.to_string())?;
    let desktop = CheckMenuItemBuilder::with_id("mode.desktop", labels.desktop)
        .checked(settings.mode == ProductWindowMode::Desktop)
        .build(app)
        .map_err(|error| error.to_string())?;
    let mode = SubmenuBuilder::with_id(app, "window-mode", labels.window_mode)
        .items(&[&sidebar, &floating, &desktop])
        .build()
        .map_err(|error| error.to_string())?;

    let left = CheckMenuItemBuilder::with_id("side.left", labels.left)
        .checked(settings.sidebar_side == SidebarSide::Left)
        .build(app)
        .map_err(|error| error.to_string())?;
    let right = CheckMenuItemBuilder::with_id("side.right", labels.right)
        .checked(settings.sidebar_side == SidebarSide::Right)
        .build(app)
        .map_err(|error| error.to_string())?;
    let side = SubmenuBuilder::with_id(app, "sidebar-side", labels.side)
        .enabled(settings.mode == ProductWindowMode::Sidebar)
        .items(&[&left, &right])
        .build()
        .map_err(|error| error.to_string())?;

    let locked = CheckMenuItemBuilder::with_id("lock.toggle", labels.lock_position)
        .checked(settings.locked)
        .build(app)
        .map_err(|error| error.to_string())?;
    let always_on_top = CheckMenuItemBuilder::with_id("always_on_top.toggle", labels.always_on_top)
        .enabled(settings.mode != ProductWindowMode::Desktop)
        .checked(settings.always_on_top && settings.mode != ProductWindowMode::Desktop)
        .build(app)
        .map_err(|error| error.to_string())?;
    let presentation_label = if settings.floating_presentation == FloatingPresentation::Collapsed {
        labels.expand_floating
    } else {
        labels.collapse_floating
    };
    let presentation_action = if settings.floating_presentation == FloatingPresentation::Collapsed {
        "floating.expand"
    } else {
        "floating.collapse"
    };
    let presentation =
        tauri::menu::MenuItemBuilder::with_id(presentation_action, presentation_label)
            .enabled(settings.mode == ProductWindowMode::Floating)
            .build(app)
            .map_err(|error| error.to_string())?;

    // Identity heading, then the widget's own restore/support entries. `Show
    // widget` matters most in the tray: no widget mode has a taskbar button, so
    // bringing the window back is a tray responsibility.
    MenuBuilder::new(app)
        .items(&[&identity])
        .separator()
        .items(&[&show_widget, &mode, &side])
        .separator()
        .items(&[&locked, &always_on_top, &presentation])
        .separator()
        .items(&[&github])
        .separator()
        .text("settings", labels.settings)
        .text("quit", labels.quit)
        .build()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{validated_project_url, PROJECT_URL};

    /// The menu action must open the project page this build belongs to, and must
    /// stay a single-purpose command rather than a generic URL opener.
    #[test]
    fn only_the_project_page_is_launchable() {
        assert!(validated_project_url(PROJECT_URL));
        assert_eq!(PROJECT_URL, "https://github.com/alanfloyd-dev/desktop-todo-widget");
        for rejected in [
            "",
            "https://github.com/alanfloyd-dev/desktop-todo-widget/issues",
            "https://github.com/alanfloyd-dev/desktop-todo-widget/",
            "https://example.com",
            "http://github.com/alanfloyd-dev/desktop-todo-widget",
            "file:///C:/Windows/System32/cmd.exe",
            "https://github.com/alanfloyd-dev/desktop-todo-widget & calc.exe",
        ] {
            assert!(
                !validated_project_url(rejected),
                "{rejected} must not be launchable from the native menu"
            );
        }
    }
}
