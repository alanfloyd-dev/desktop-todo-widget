use crate::{
    product_window::{self, ProductViewState, ProductWindowRuntime},
    settings::{AppState, FloatingPresentation, ProductWindowMode, SidebarSide},
    window_mode::NativeWindowState,
};
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, SubmenuBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewWindow,
};

const TRAY_ID: &str = "alan-desktop-tray";

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
            let resizable = !locked
                && updated.mode == ProductWindowMode::Floating
                && updated.floating_presentation == FloatingPresentation::Expanded;
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
            window
                .emit("open-settings", ())
                .map_err(|error| error.to_string())?;
        }
        "quit" => app.exit(0),
        _ => return Err(format!("unknown product action: {action}")),
    }

    refresh_tray_menu(app)?;
    let settings = state.snapshot()?;
    Ok(ProductViewState::new(&state, settings))
}

#[tauri::command]
pub fn show_product_context_menu(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    let menu = build_tray_menu(&app)?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

pub fn install_tray(app: &tauri::App) -> Result<(), String> {
    let menu = build_tray_menu(app.handle())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Alan Desktop")
        .show_menu_on_left_click(true);
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

fn build_tray_menu(app: &tauri::AppHandle) -> Result<tauri::menu::Menu<tauri::Wry>, String> {
    let settings = app.state::<AppState>().snapshot()?;
    let sidebar = CheckMenuItemBuilder::with_id("mode.sidebar", "Sidebar")
        .checked(settings.mode == ProductWindowMode::Sidebar)
        .build(app)
        .map_err(|error| error.to_string())?;
    let floating = CheckMenuItemBuilder::with_id("mode.floating", "Floating")
        .checked(settings.mode == ProductWindowMode::Floating)
        .build(app)
        .map_err(|error| error.to_string())?;
    let desktop = CheckMenuItemBuilder::with_id("mode.desktop", "Desktop — Experimental")
        .checked(settings.mode == ProductWindowMode::Desktop)
        .build(app)
        .map_err(|error| error.to_string())?;
    let mode = SubmenuBuilder::with_id(app, "window-mode", "Window mode")
        .items(&[&sidebar, &floating, &desktop])
        .build()
        .map_err(|error| error.to_string())?;

    let left = CheckMenuItemBuilder::with_id("side.left", "Left")
        .checked(settings.sidebar_side == SidebarSide::Left)
        .build(app)
        .map_err(|error| error.to_string())?;
    let right = CheckMenuItemBuilder::with_id("side.right", "Right")
        .checked(settings.sidebar_side == SidebarSide::Right)
        .build(app)
        .map_err(|error| error.to_string())?;
    let side = SubmenuBuilder::with_id(app, "sidebar-side", "Side")
        .enabled(settings.mode == ProductWindowMode::Sidebar)
        .items(&[&left, &right])
        .build()
        .map_err(|error| error.to_string())?;

    let locked = CheckMenuItemBuilder::with_id("lock.toggle", "Lock position")
        .checked(settings.locked)
        .build(app)
        .map_err(|error| error.to_string())?;
    let always_on_top = CheckMenuItemBuilder::with_id("always_on_top.toggle", "Always on top")
        .enabled(settings.mode != ProductWindowMode::Desktop)
        .checked(settings.always_on_top && settings.mode != ProductWindowMode::Desktop)
        .build(app)
        .map_err(|error| error.to_string())?;
    let presentation_label = if settings.floating_presentation == FloatingPresentation::Collapsed {
        "Expand Floating"
    } else {
        "Collapse to Avatar Orb"
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

    MenuBuilder::new(app)
        .items(&[&mode, &side])
        .separator()
        .items(&[&locked, &always_on_top, &presentation])
        .separator()
        .text("settings", "Settings")
        .text("quit", "Quit")
        .build()
        .map_err(|error| error.to_string())
}
