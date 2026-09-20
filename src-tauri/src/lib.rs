mod appearance;
mod database;
mod diagnostics;
mod locale;
#[cfg(target_os = "windows")]
mod maintenance_admission;
// Non-Windows compilation stub: the product ships on Windows only. The
// commands exist so the invoke handler stays one list; they always refuse,
// and there is no admission state behind them.
#[cfg(not(target_os = "windows"))]
mod maintenance_admission {
    #[derive(serde::Serialize)]
    pub struct AdmissionView {
        pub mode: &'static str,
    }
    #[tauri::command]
    pub fn maintenance_admission_state() -> AdmissionView {
        AdmissionView { mode: "unmanaged" }
    }
    #[tauri::command]
    pub fn start_uninstall() -> Result<(), &'static str> {
        Err("unsupported-platform")
    }
}
mod platform;
mod product_commands;
mod product_settings_commands;
mod product_window;
mod qa_diagnostics;
#[cfg(test)]
mod qa_fixture;
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
/// afterwards. This delegates to the shared canonical data-root resolver — the
/// same Known Folder derivation the maintenance helper and the launch
/// admission use, and the same platform location Tauri uses for the configured
/// bundle identifier. `setup` asserts the two agree so a future identifier or
/// platform change cannot silently point the early read at the wrong file.
#[cfg(target_os = "windows")]
fn app_data_dir() -> Option<std::path::PathBuf> {
    desktop_todo_maintenance::paths::canonical_data_root().ok()
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
    // Maintenance admission precedes every diagnostics write and the early
    // rendering-backend database read: while a maintenance transaction holds
    // the gate or durable maintenance state is active, business
    // initialization — including any database open — must not run
    // (docs/application-lifecycle.md §11). Admission is check-only: it never
    // repairs a receipt, and a present-but-invalid receipt refuses the
    // launch instead of degrading to unmanaged.
    #[cfg(target_os = "windows")]
    let admission = maintenance_admission::admit_or_report();

    let qa_diagnostics = qa_diagnostics::QaDiagnostics::from_process_args();
    qa_diagnostics::warn_if_release_without_embedded_frontend(&qa_diagnostics);
    #[cfg(target_os = "windows")]
    qa_diagnostics.record(format!(
        "maintenance_admission={:?} installationId={}",
        admission.context,
        admission.installation_id.as_deref().unwrap_or("-")
    ));

    // --- Rendering backend selection ---------------------------------------
    // Standard-only retirement: the hosting decision is permanently Standard
    // and the composition path is gone. The persisted backend is still read
    // so the log records when a stored document named Enhanced — that line is
    // the only trace such a document leaves, and it must stay answerable from
    // the log alone.
    //
    // A database that cannot be opened here is a real failure, not a reason
    // to pretend the user chose Standard: it is reported through the
    // controlled fatal path (diagnostics log + native message, non-zero exit)
    // exactly like the later full load, because continuing would mean running
    // a half-initialized product whose data layer is known-bad. Admission has
    // already passed at this point, so this check never bypasses it.
    let early_data_dir = app_data_dir();
    let persisted_backend = match &early_data_dir {
        Some(dir) => match settings::AppState::read_rendering_backend(dir) {
            Ok(backend) => backend,
            Err(error) => fatal_startup_failure(
                &qa_diagnostics,
                format!("the database could not be opened at startup: {error}"),
            ),
        },
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

    let builder = tauri::Builder::default()
        .manage(NativeWindowState::default())
        .manage(product_runtime)
        .manage(qa_diagnostics)
        .manage(weather::WeatherRuntime::default());
    // The admission record — and with it the shared application lease — must
    // live until process exit so maintenance cannot start mid-session.
    #[cfg(target_os = "windows")]
    let builder = builder.manage(admission);
    let app = match builder
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
                .map_err(|error| {
                    diagnostics.record(format!("[startup-fatal] database open failed: {error}"));
                    std::io::Error::other(error)
                })?;
            app.manage(settings::AppState::load(database).map_err(|error| {
                diagnostics.record(format!("[startup-fatal] settings load failed: {error}"));
                std::io::Error::other(error)
            })?);
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
            window_mode::start_win_d_trace,
            maintenance_admission::maintenance_admission_state,
            maintenance_admission::start_uninstall
        ])
        .build(tauri::generate_context!())
    {
        // A setup failure (database, tray, window) reaches here as a build
        // error. It must end as a controlled failure — diagnostics were
        // recorded where the state was available, the user gets a native
        // message, and the process exits non-zero — never a panic with no UI.
        Ok(app) => app,
        Err(error) => fatal_dialog(&format!("desktop-todo-widget failed to start: {error}")),
    };
    app.run(|_, _| {});
}

/// The controlled startup-failure path: record through the diagnostics log
/// while the instance is still available, then report and exit non-zero.
/// Never a panic; never a half-initialized product UI.
fn fatal_startup_failure(diagnostics: &qa_diagnostics::QaDiagnostics, message: String) -> ! {
    diagnostics.record(format!("[startup-fatal] {message}"));
    fatal_dialog(&message)
}

fn fatal_dialog(message: &str) -> ! {
    #[cfg(target_os = "windows")]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
        };
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(message),
                &HSTRING::from("desktop-todo-widget"),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    eprintln!("desktop-todo-widget: {message}");
    std::process::exit(4);
}

#[cfg(test)]
mod startup_failure_tests {
    /// A startup failure — database unavailable, settings load failure, tray
    /// or window failure — must end in the controlled fatal path (diagnostics
    /// log, native message, non-zero exit), never in a panic from an
    /// `.expect` on the Tauri build. Source-pinned: the full Tauri process
    /// path is not unit-testable, but the contract is structural.
    #[test]
    fn startup_build_failure_is_controlled_not_a_panic() {
        let source = include_str!("lib.rs");
        assert!(
            !source.contains(".expect(\"error while building"),
            "the Tauri build must not panic on failure"
        );
        assert!(source.contains("fatal_dialog("), "controlled exit must exist");
        assert!(
            source.contains("fatal_startup_failure("),
            "the early database failure path must exist"
        );
        // The database failure at startup must be recorded, not swallowed.
        assert!(
            source.contains("[startup-fatal] database open failed"),
            "setup must record the database failure before returning"
        );
        assert!(
            source.contains("[startup-fatal] settings load failed"),
            "setup must record the settings failure before returning"
        );
    }
}
