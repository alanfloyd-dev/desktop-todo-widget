mod appearance;
mod database;
mod diagnostics;
mod product_commands;
mod product_window;
mod reviews;
mod settings;
mod task_day;
mod tasks;
mod weather;
mod window_mode;

use product_window::ProductWindowRuntime;
use tauri::Manager;
use window_mode::NativeWindowState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(NativeWindowState::default())
        .manage(ProductWindowRuntime::default())
        .manage(weather::WeatherRuntime::default())
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
            product_window::restore_product_window(
                &window,
                &app.state::<settings::AppState>(),
                &app.state::<NativeWindowState>(),
                &app.state::<ProductWindowRuntime>(),
            )
            .map_err(std::io::Error::other)?;
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
        .run(tauri::generate_context!())
        .expect("error while running Alan Desktop");
}
