mod appearance;
mod database;
mod diagnostics;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let qa_diagnostics = qa_diagnostics::QaDiagnostics::from_process_args();
    qa_diagnostics.apply_webview_hosting_environment();
    qa_diagnostics::warn_if_release_without_embedded_frontend();
    let product_runtime = ProductWindowRuntime::new(
        qa_diagnostics.native_material_off(),
        qa_diagnostics.startup_material_bypassed(),
        qa_diagnostics.window_to_visual_requested(),
    );
    #[cfg(target_os = "windows")]
    if qa_diagnostics.composition_controller_requested() {
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
            let data_dir = app.path().app_data_dir()?;
            let database = database::Database::open(data_dir.join("alan-desktop.sqlite3"))
                .map_err(std::io::Error::other)?;
            app.manage(settings::AppState::load(database).map_err(std::io::Error::other)?);
            product_commands::install_tray(app).map_err(std::io::Error::other)?;

            let window = app
                .get_webview_window("main")
                .ok_or_else(|| std::io::Error::other("main window unavailable"))?;
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
            product_window::open_shortcut,
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
        .expect("error while building Alan Desktop")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                app.state::<ProductWindowRuntime>()
                    .shutdown_native_material();
            }
        });
}
