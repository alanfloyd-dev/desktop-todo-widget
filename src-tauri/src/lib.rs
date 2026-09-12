mod appearance;
mod database;
mod diagnostics;
mod locale;
mod platform;
mod product_commands;
mod product_window;
mod qa_diagnostics;
mod reviews;
mod settings;
mod task_day;
mod tasks;
mod weather;
mod window_mode;

#[cfg(target_os = "windows")]
#[path = "../../tools/native-acrylic-poc/src/winappsdk.rs"]
mod winappsdk;

use product_window::ProductWindowRuntime;
use tauri::Manager;
use window_mode::NativeWindowState;

/// Application data directory, resolved before Tauri exists.
///
/// The rendering backend has to be read from the database before the Tauri
/// builder creates the config window, but `AppHandle::path()` is only available
/// afterwards. This reproduces the platform location Tauri uses for the
/// configured bundle identifier; `setup` asserts the two agree so a future
/// identifier or platform change cannot silently point the early read at the
/// wrong file.
#[cfg(target_os = "windows")]
fn app_data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|roaming| roaming.join("net.alanfloyd.desktop"))
}

#[cfg(not(target_os = "windows"))]
fn app_data_dir() -> Option<std::path::PathBuf> {
    None
}

/// Puts the product window into widget window semantics on Windows.
///
/// The product is a tray-resident widget in every mode (Sidebar, Floating
/// expanded, Floating collapsed/Orb, Desktop), so the window must never appear in
/// the taskbar and must not behave like an application window in Alt+Tab. Tauri's
/// `skip_taskbar` cannot express that: on Windows tao implements it with
/// `ITaskbarList::DeleteTab`, a Shell request that leaves `WS_EX_APPWINDOW` — the
/// style Windows documents as forcing a taskbar button — on the window.
///
/// `platform::windows::widget_frame` therefore applies the Win32 tool-window
/// semantics and keeps them applied through the window procedure, which is what
/// makes them survive the style rewrites each mode transition performs. This
/// bootstrap is the runtime authority; `tauri.conf.json` declares the matching
/// `skipTaskbar` baseline so no button can appear before `setup` runs.
#[cfg(target_os = "windows")]
fn install_widget_frame(window: &tauri::WebviewWindow) -> Result<(), std::io::Error> {
    use platform::windows::widget_frame;

    let hwnd = window.hwnd().map_err(std::io::Error::other)?;
    let report = widget_frame::enforce_on(hwnd.0 as isize).map_err(std::io::Error::other)?;
    eprintln!("[widget-frame] {}", report.summary());
    window
        .app_handle()
        .state::<qa_diagnostics::QaDiagnostics>()
        .record(report.summary());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let qa_diagnostics = qa_diagnostics::QaDiagnostics::from_process_args();
    qa_diagnostics.apply_webview_hosting_environment();
    qa_diagnostics::warn_if_release_without_embedded_frontend();

    // --- Rendering backend selection ---------------------------------------
    // The hosting backend is fixed when the WebView is created, so it must be
    // resolved before the Tauri builder does that below. The persisted product
    // setting is the source of truth; the QA flag only exists to force the
    // composition path in test runs.
    let early_data_dir = app_data_dir();
    let persisted_backend = match &early_data_dir {
        Some(dir) => settings::AppState::read_rendering_backend(dir),
        None => settings::RenderingBackend::default(),
    };
    let qa_forced = qa_diagnostics.composition_controller_requested();
    #[cfg(target_os = "windows")]
    let enhanced_requested = qa_forced || persisted_backend.uses_composition_hosting();
    #[cfg(not(target_os = "windows"))]
    let enhanced_requested = false;
    qa_diagnostics.record_rendering_backend(
        persisted_backend.as_str(),
        if enhanced_requested {
            "enhanced"
        } else {
            "standard"
        },
        if qa_forced { "qa_override" } else { "persisted_setting" },
    );

    // The runtime carries the hosting decision: the native material host may only
    // be driven when the WebView is actually composited into this app's visual
    // tree (see `apply_native_composition`).
    let product_runtime = ProductWindowRuntime::new(
        qa_diagnostics.native_material_off(),
        qa_diagnostics.startup_material_bypassed(),
        qa_diagnostics.window_to_visual_requested(),
        enhanced_requested,
    );

    #[cfg(target_os = "windows")]
    if enhanced_requested {
        use platform::windows::composition_host;
        // Load the Windows App Runtime before any window/WebView exists: the
        // composition factory later runs inside Wry's WebView creation call
        // stack, where LoadLibrary deadlocks on the Windows loader lock.
        composition_host::preload_windows_app_runtime()
            .expect("preload Phase 7C.2 Windows App Runtime");
        composition_host::register_wry_hooks(&product_runtime.native_material_store());
        eprintln!("[phase7c2] enabled=true scope=main default_windowed_unchanged=true");
    }
    tauri::Builder::default()
        .manage(NativeWindowState::default())
        .manage(product_runtime)
        .manage(qa_diagnostics)
        .manage(weather::WeatherRuntime::default())
        .on_page_load(|webview, payload| {
            let diagnostics = webview
                .app_handle()
                .state::<qa_diagnostics::QaDiagnostics>();
            let event = match payload.event() {
                tauri::webview::PageLoadEvent::Started => "started",
                tauri::webview::PageLoadEvent::Finished => "completed",
            };
            diagnostics.record(format!(
                "webview_navigation_{event}=true frontend_asset_mode={}",
                qa_diagnostics::frontend_asset_mode()
            ));
        })
        .on_menu_event(|app, event| {
            if let Err(error) = product_commands::dispatch_product_action(app, event.id().as_ref())
            {
                eprintln!("[product-command] menu action failed: {error}");
            }
        })
        .setup(|app| {
            let resolved_data_dir = app.path().app_data_dir()?;
            // The rendering backend was read before the WebView existed, using a
            // path derived without an AppHandle. Assert the two agree; if they
            // ever diverge the early read would silently target the wrong file.
            if let Some(early_dir) = app_data_dir() {
                if early_dir != resolved_data_dir {
                    eprintln!(
                        "[rendering] app_data_dir_mismatch early={} tauri={} — rendering backend was read from the wrong location",
                        early_dir.display(),
                        resolved_data_dir.display()
                    );
                }
            }
            let database = database::Database::open(resolved_data_dir.join("alan-desktop.sqlite3"))
                .map_err(std::io::Error::other)?;
            app.manage(settings::AppState::load(database).map_err(std::io::Error::other)?);
            product_commands::install_tray(app).map_err(std::io::Error::other)?;

            let window = app
                .get_webview_window("main")
                .ok_or_else(|| std::io::Error::other("main window unavailable"))?;
            // The product is a tray-resident widget: no window mode may carry a
            // taskbar button or an Alt+Tab entry. Installed here, on the main
            // thread and before the first mode transition, so every later
            // Tauri/tao style rewrite is already covered. See
            // `platform::windows::widget_frame` for why the style has to be
            // enforced at the window procedure instead of set once.
            #[cfg(target_os = "windows")]
            install_widget_frame(&window)?;
            let diagnostics = app.state::<qa_diagnostics::QaDiagnostics>();
            if diagnostics.window_to_visual_requested() {
                product_window::restore_window_to_visual_qa(
                    &window,
                    &app.state::<settings::AppState>(),
                    &app.state::<NativeWindowState>(),
                    &app.state::<ProductWindowRuntime>(),
                )
            } else {
                product_window::restore_product_window(
                    &window,
                    &app.state::<settings::AppState>(),
                    &app.state::<NativeWindowState>(),
                    &app.state::<ProductWindowRuntime>(),
                )
            }
            .map_err(std::io::Error::other)?;
            #[cfg(target_os = "windows")]
            app.state::<ProductWindowRuntime>()
                .attach_composition_controller(&window)
                .map_err(std::io::Error::other)?;
            qa_diagnostics::record_webview_state(
                &window,
                &app.state::<qa_diagnostics::QaDiagnostics>(),
            );
            Ok(())
        })
        .on_window_event(product_window::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            appearance::resolve_appearance_contrast,
            appearance::choose_local_asset,
            appearance::load_managed_asset,
            appearance::load_windows_wallpaper,
            appearance::discard_managed_asset,
            product_window::product_state,
            product_window::update_product_settings,
            product_window::request_window_drag,
            product_window::open_quick_link,
            product_window::quit_app,
            product_commands::product_action,
            product_commands::show_product_context_menu,
            qa_diagnostics::qa_frontend_ready,
            qa_diagnostics::qa_frontend_input,
            qa_diagnostics::qa_diagnostic_mode,
            product_window::qa_native_material_control,
            diagnostics::copyable_diagnostics,
            reviews::review_report,
            tasks::today_tasks,
            tasks::add_task,
            tasks::edit_task,
            tasks::toggle_task_completed,
            tasks::cancel_task,
            tasks::carry_task,
            tasks::delete_task,
            tasks::reorder_tasks,
            tasks::create_category,
            weather::weather_state,
            weather::search_weather_locations,
            weather::refresh_weather,
            weather::open_weather_attribution,
            window_mode::set_window_mode,
            window_mode::window_diagnostics,
            window_mode::start_win_d_trace
        ])
        .build(tauri::generate_context!())
        .expect("error while building desktop-todo-widget")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                app.state::<ProductWindowRuntime>()
                    .shutdown_native_material();
            }
        });
}
