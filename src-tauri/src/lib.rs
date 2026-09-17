mod appearance;
mod database;
mod diagnostics;
mod locale;
mod platform;
mod product_commands;
mod product_settings_commands;
mod product_window;
mod qa_diagnostics;
mod reviews;
mod settings;
mod task_day;
mod tasks;
mod weather;
mod window_mode;

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
    qa_diagnostics::warn_if_release_without_embedded_frontend(&qa_diagnostics);

    // --- Rendering backend selection ---------------------------------------
    // Standard-only retirement: the hosting decision is permanently Standard
    // and the composition path is gone. The persisted backend is still read
    // so the log records when a stored document named Enhanced — that line is
    // the only trace such a document leaves, and it must stay answerable from
    // the log alone.
    let early_data_dir = app_data_dir();
    let persisted_backend = match &early_data_dir {
        Some(dir) => settings::AppState::read_rendering_backend(dir),
        None => settings::RenderingBackend::default(),
    };
    qa_diagnostics.record_rendering_backend(
        persisted_backend.as_str(),
        "standard",
        if persisted_backend.uses_composition_hosting() {
            "standard_only_ignored_enhanced_request"
        } else {
            "standard_only"
        },
    );

    let product_runtime = ProductWindowRuntime::new();

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
            let diagnostics = app.state::<qa_diagnostics::QaDiagnostics>();
            // The rendering backend was read before the WebView existed, using a
            // path derived without an AppHandle. Assert the two agree; if they
            // ever diverge the early read would silently target the wrong file.
            //
            // Reported through the diagnostics log rather than stderr: this path
            // only matters in an optimized build, which is a GUI-subsystem process
            // with no console to print to.
            if let Some(early_dir) = app_data_dir() {
                if early_dir != resolved_data_dir {
                    diagnostics.record(format!(
                        "[rendering] app_data_dir_mismatch early={} tauri={} — rendering backend was read from the wrong location",
                        early_dir.display(),
                        resolved_data_dir.display()
                    ));
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
            product_window::restore_product_window(
                &window,
                &app.state::<settings::AppState>(),
                &app.state::<NativeWindowState>(),
                &app.state::<ProductWindowRuntime>(),
            )
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
            appearance::store_managed_asset,
            appearance::load_managed_asset,
            appearance::load_windows_wallpaper,
            appearance::discard_managed_asset,
            product_settings_commands::product_state,
            product_settings_commands::update_product_settings,
            product_window::request_window_drag,
            product_window::open_quick_link,
            product_window::quit_app,
            product_commands::product_action,
            product_commands::show_product_context_menu,
            qa_diagnostics::qa_frontend_ready,
            qa_diagnostics::qa_frontend_input,
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
        .run(|_, _| {});
}
