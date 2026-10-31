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
// v1.3.0-G1 GitHub dashboard module: the only place this build speaks to
// api.github.com. Crate-private like the updater — the network capability is
// not part of the product's public API, and the frontend reaches it only
// through the three commands registered below.
mod github;
mod qa_diagnostics;
#[cfg(test)]
mod qa_fixture;
mod reviews;
mod settings;
mod task_day;
mod tasks;
// Phase 2 updater: provider discovery and bounded metadata fetch. The
// verification/eligibility core is the shared `desktop-todo-update-core`
// crate; this module only transports attacker-controlled bytes to it.
//
// Deliberately crate-private: the network capability is not part of the
// product's public API. `#[allow(dead_code)]` remains scoped to the module
// because parts of the engine (explicit per-provider sources, the QA
// visibility seam) still have no production caller; the staged-update
// install half (`install_ready_update`) is wired since Phase 2D-B and the
// manual check half (`check_for_updates`/`download_update`) since v1.2.1-A.
#[allow(dead_code)]
mod updater;
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
///
/// Debug/test builds additionally honor an explicit `DTW_QA_DATA_DIR`
/// override (see [`select_data_root`]); the branch is compiled out of release
/// builds entirely, so no environment variable can redirect production data.
///
/// The return is a hard failure, not `None`, when an explicit QA override
/// cannot be honored: `None` means "this platform has no known root" and
/// `setup` tolerates it by falling back to Tauri's answer, which is the real
/// user profile — exactly what a QA session must never do.
#[cfg(target_os = "windows")]
fn app_data_dir() -> Option<std::path::PathBuf> {
    #[cfg(any(debug_assertions, test))]
    let qa_override = std::env::var_os("DTW_QA_DATA_DIR");
    #[cfg(any(debug_assertions, test))]
    let selection = select_data_root(
        qa_override,
        desktop_todo_maintenance::paths::canonical_data_root(),
    );
    // Release builds have no override: the selection layer is not called and
    // the production root is the only possible answer (its `Err` becomes the
    // same `None` the pre-override code produced).
    #[cfg(not(any(debug_assertions, test)))]
    let selection = desktop_todo_maintenance::paths::canonical_data_root()
        .map(Some)
        .map_err(|error| error.to_string());
    match selection {
        Ok(Some(dir)) => Some(dir),
        // `Ok(None)` is the legacy "no trusted root on this platform"
        // answer — with no override set, the caller's Tauri fallback is
        // unchanged.
        Ok(None) => None,
        Err(error) => {
            // An error means an explicit QA override could not be honored
            // safely (debug/test only — release never reads the variable).
            // Fail closed: abort the process here — before the
            // rendering-backend read, before any database open, before any
            // write. A panic is the honest failure for a misconfigured QA
            // launch: this code only exists in builds with a console to
            // show the message, and the message no longer claims the QA
            // variable itself was invalid when it may have been the
            // trusted-root resolution that failed.
            #[cfg(any(debug_assertions, test))]
            panic!("QA data-root selection failed (refusing to touch real app data): {error}");
            // Release: the only reachable error is the production root's own
            // resolution failure, which the caller has always tolerated as
            // the legacy `None` (Tauri fallback).
            #[cfg(not(any(debug_assertions, test)))]
            {
                let _ = error;
                None
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn app_data_dir() -> Option<std::path::PathBuf> {
    None
}

/// Selects the effective data root: the pure, testable core of
/// [`app_data_dir`] in debug/test builds.
///
/// The raw override value is a parameter (already read by the caller), so
/// tests pass explicit inputs and never mutate process environment state.
/// The decision matrix is the fail-closed contract:
///
/// | override | production root | result                                    |
/// |----------|-----------------|-------------------------------------------|
/// | unset    | success         | `Ok(Some(canonical))`                     |
/// | unset    | failure         | `Ok(None)` — legacy Tauri fallback        |
/// | set      | success         | validated QA root, or `Err` if unsafe     |
/// | set      | failure         | `Err` — startup aborts                    |
/// | set      | invalid (any)   | `Err` — startup aborts                    |
///
/// An explicit override **requires** a successfully resolved trusted
/// production root: the containment check cannot be performed without one,
/// and no environment-derived stand-in may replace the Known Folder result.
/// When the trusted resolver fails, a QA session refuses to start rather
/// than risk selecting the real profile it can no longer prove distance
/// from.
///
/// Release builds never call this function: their only caller compiles the
/// call out, so no environment variable can redirect production data.
#[cfg(all(target_os = "windows", any(debug_assertions, test)))]
fn select_data_root(
    qa_raw: Option<std::ffi::OsString>,
    canonical: Result<std::path::PathBuf, desktop_todo_maintenance::Error>,
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(raw) = qa_raw else {
        // No override: the production root's own failure is the legacy
        // "no trusted root" answer, not an error — the caller falls back to
        // Tauri exactly as the pre-override code did.
        return Ok(canonical.ok());
    };
    // An override demands a trusted production root to validate containment
    // against. Without one there is nothing to prove distance from, and no
    // substitute authority (environment variables included) may stand in.
    let canonical = canonical.map_err(|error| {
        format!(
            "the trusted production data root could not be resolved, so the \
             QA directory cannot be proven safe: {error}"
        )
    })?;
    // Lossless: the resolver consumes the OsStr value directly, so no
    // invalid-Unicode path can be silently transformed by a lossy string
    // conversion before validation.
    resolve_qa_data_root(&raw, Some(&canonical)).map(Some)
}

/// Resolves the effective data root for a debug/test QA launch.
///
/// Pure and platform-neutral: callers pass the raw override value and the
/// trusted production root explicitly, so tests never mutate process
/// environment state and never touch real user data. The override is
/// opt-in and validated strictly — any doubt fails with `Err`, never a
/// fallback to the canonical root:
///
/// * the value (a lossless `OsStr`, never a lossy string conversion) must be
///   non-empty, absolute, and resolve through `std::fs::canonicalize`, which
///   both rejects paths that do not exist (the directory must be pre-created,
///   so there is no ambiguity about who creates it) and resolves
///   symlinks/reparse points to their real target;
/// * the resolved target must be a directory, not a file;
/// * it must not equal or sit inside of the canonical production root
///   (case-insensitive component comparison — Windows paths are
///   case-insensitive — so a differently-cased or linked path cannot sneak
///   the real profile back in).
///
/// The `canonical` parameter is a trusted, already-resolved production root:
/// the selection layer refuses to call this without one, so the containment
/// check is never skipped. Release builds never call this: the only caller
/// is compiled out, and the canonical root is used unchanged.
fn resolve_qa_data_root(
    qa_raw: &std::ffi::OsStr,
    canonical: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    let lossy_for_message = qa_raw.to_string_lossy();
    let message_form = lossy_for_message.trim();
    // Whitespace is checked through the string form; the path itself is
    // always built from the original OsStr, so no non-Unicode byte is lost.
    if message_form.is_empty() {
        return Err("the variable is set but empty".to_string());
    }
    let requested = std::path::PathBuf::from(qa_raw);
    if !requested.is_absolute() {
        return Err(format!("`{message_form}` is not an absolute path"));
    }
    // Canonicalize or fail: a missing directory is an error on purpose — the
    // QA harness pre-creates and owns the directory, and canonicalization is
    // also what resolves symlinks/reparse points to their real target so a
    // linked path cannot escape the checks below.
    let resolved = std::fs::canonicalize(&requested)
        .map_err(|error| format!("`{message_form}` cannot be resolved as an existing directory: {error}"))?;
    if !resolved.is_dir() {
        return Err(format!("`{message_form}` is not a directory"));
    }
    if let Some(canonical) = canonical {
        // Compare against the canonical root *resolved* the same way, so a
        // junction/reparse point inside the production path cannot make a
        // nested QA directory look lexically unrelated to its real location.
        // A missing production directory (fresh machine) keeps its lexical
        // form: there is nothing on disk to resolve through, and the
        // descendant rule must still apply.
        let canonical_resolved =
            std::fs::canonicalize(canonical).unwrap_or_else(|_| canonical.to_path_buf());
        if is_same_or_inside(&resolved, &canonical_resolved) {
            return Err(format!(
                "`{message_form}` is the real application data directory or a location inside it"
            ));
        }
    }
    Ok(resolved)
}

/// Whether `candidate` is `ancestor` itself or a path inside it, compared
/// case-insensitively component-wise (Windows path semantics; a component
/// prefix comparison also refuses sibling names that merely share a prefix).
fn is_same_or_inside(candidate: &std::path::Path, ancestor: &std::path::Path) -> bool {
    fn lower_components(path: &std::path::Path) -> Vec<String> {
        path.components()
            .filter_map(|component| component.as_os_str().to_str())
            .map(|text| text.to_lowercase())
            .collect()
    }
    let (candidate_parts, ancestor_parts) = (lower_components(candidate), lower_components(ancestor));
    !ancestor_parts.is_empty() && candidate_parts.starts_with(&ancestor_parts)
}

/// Whether this process is running under the debug/test QA data-root
/// override. Compiled to a constant `false` in release builds, where the
/// override does not exist and the diagnostics note can never fire.
#[cfg(any(debug_assertions, test))]
fn qa_data_override_active() -> bool {
    std::env::var_os("DTW_QA_DATA_DIR").is_some()
}

#[cfg(not(any(debug_assertions, test)))]
fn qa_data_override_active() -> bool {
    false
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
        .manage(weather::WeatherRuntime::default())
        .manage(github::GithubRuntime::default());
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
            // The database targets the same resolver the early read used —
            // `app_data_dir()`, which debug/test builds may have redirected to
            // a validated QA root. Using Tauri's own `app_data_dir()` here
            // instead would open the real user profile in a QA session. A
            // resolver failure falls through to Tauri's answer exactly as the
            // pre-override code tolerated it for the early read.
            let resolved_data_dir = match app_data_dir() {
                Some(dir) => dir,
                None => app.path().app_data_dir()?,
            };
            let diagnostics = app.state::<qa_diagnostics::QaDiagnostics>();
            // The rendering backend was read before the WebView existed, using a
            // path derived without an AppHandle. Assert the two agree; if they
            // ever diverge the early read would silently target the wrong file.
            //
            // A debug/test QA session diverges by design — the override
            // redirects the data root away from Tauri's Known-Folder answer —
            // so it records an honest informational note instead. The
            // production mismatch message is unchanged.
            //
            // Reported through the diagnostics log rather than stderr: this path
            // only matters in an optimized build, which is a GUI-subsystem process
            // with no console to print to.
            let tauri_dir = app.path().app_data_dir()?;
            if tauri_dir != resolved_data_dir {
                if qa_data_override_active() {
                    diagnostics.record(format!(
                        "[qa-isolation] data_root={} tauri_app_data={} (override active)",
                        resolved_data_dir.display(),
                        tauri_dir.display()
                    ));
                } else {
                    diagnostics.record(format!(
                        "[rendering] app_data_dir_mismatch early={} tauri={} — rendering backend was read from the wrong location",
                        resolved_data_dir.display(),
                        tauri_dir.display()
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
            // The probation child's single health obligation: once core
            // initialization succeeded (backend, database, settings, tray,
            // main window), write the durable HealthAck marker. Ordinary
            // launches never write health; a failed write exits so the
            // update rolls back instead of faking health.
            #[cfg(target_os = "windows")]
            maintenance_admission::acknowledge_update_health(
                app.state::<maintenance_admission::Admission>().inner(),
                app.state::<qa_diagnostics::QaDiagnostics>().inner(),
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
            github::github_state,
            github::refresh_github,
            github::open_github_profile,
            window_mode::set_window_mode,
            window_mode::window_diagnostics,
            window_mode::start_win_d_trace,
            maintenance_admission::maintenance_admission_state,
            maintenance_admission::start_uninstall,
            updater::install::install_ready_update,
            updater::check::check_for_updates,
            updater::check::download_update
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

    // --- QA data-root override (v1.3.0-C4-A.1) --------------------------------

    /// The resolver is pure: every test passes explicit inputs, so no test
    /// mutates process environment state, and the "production root" below is
    /// a temp directory standing in for the real one — the user's actual
    /// `%APPDATA%` is never read or written.
    mod qa_data_root {
        use crate::{resolve_qa_data_root, select_data_root};
        use std::ffi::OsStr;

        /// A unique scratch directory under the system temp dir, following the
        /// same convention as the settings tests' `temp_db_path` (no new
        /// dependencies); removed best-effort on drop.
        struct Scratch(std::path::PathBuf);
        impl Scratch {
            fn new(tag: &str) -> Self {
                let suffix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos();
                let path = std::env::temp_dir().join(format!(
                    "dtw-qa-root-{tag}-{}-{suffix}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&path).expect("scratch dir");
                Scratch(path)
            }
            fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        /// Creates `prod/` (the stand-in production root) plus `qa/` (a valid
        /// pre-created QA directory) under one fresh scratch dir.
        fn fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, Scratch) {
            let scratch = Scratch::new(tag);
            let prod = scratch.path().join("prod");
            let qa = scratch.path().join("qa");
            std::fs::create_dir_all(&prod).expect("prod dir");
            std::fs::create_dir_all(&qa).expect("qa dir");
            (prod, qa, scratch)
        }

        #[test]
        fn decision_matrix_unset_override_matches_legacy_behavior() {
            // Unset + production success → the canonical production root.
            let (prod, _qa, _temp) = fixture("solo");
            assert_eq!(
                select_data_root(None, Ok(prod.clone())).expect("passes"),
                Some(prod)
            );
            // Unset + production failure → `Ok(None)` — the legacy "no
            // trusted root" answer the caller turns into the Tauri
            // fallback. Not an error: production never panics over its own
            // resolver.
            assert_eq!(
                select_data_root(None, Err(broken_root())).expect("legacy none"),
                None
            );
        }

        #[test]
        fn decision_matrix_override_set_follows_the_fail_closed_policy() {
            let (prod, qa, _temp) = fixture("x");
            // Valid override + production success → the QA root.
            let selected = select_data_root(Some(qa.as_os_str().to_owned()), Ok(prod.clone()))
                .expect("valid override selects the qa root")
                .expect("some root");
            assert_eq!(selected, std::fs::canonicalize(&qa).unwrap());

            // Valid override + production failure → Err: the containment
            // check cannot run without a trusted root, and no stand-in
            // authority may replace it. THE security-review corner: the
            // previous implementation selected the QA root here, leaving the
            // real profile reachable if the operator had named it.
            let error = select_data_root(Some(qa.as_os_str().to_owned()), Err(broken_root()))
                .expect_err("a production-root failure must abort a QA session");
            assert!(error.contains("cannot be proven safe"), "unexpected error: {error}");

            // Invalid override + production failure → Err (same refusal).
            let missing = qa.parent().unwrap().join("no-such-dir");
            assert!(
                select_data_root(Some(missing.as_os_str().to_owned()), Err(broken_root())).is_err()
            );
            // Invalid override + production success → Err (no fallback in
            // either direction).
            let error = select_data_root(Some(missing.as_os_str().to_owned()), Ok(prod.clone()))
                .expect_err("invalid override must fail closed");
            assert!(error.contains("cannot be resolved"));

            // Override equal to / inside the production root → Err, with the
            // trusted root available for the containment check.
            assert!(
                select_data_root(Some(prod.as_os_str().to_owned()), Ok(prod.clone())).is_err(),
                "the production root itself must be refused as a QA directory"
            );
            let nested = prod.join("nested-qa");
            std::fs::create_dir_all(&nested).expect("nested dir");
            assert!(
                select_data_root(Some(nested.as_os_str().to_owned()), Ok(prod.clone())).is_err(),
                "a QA directory inside the production root must be refused"
            );
        }

        #[test]
        fn an_override_linking_into_the_production_root_is_refused_where_links_can_be_created() {
            // Symlink creation needs SeCreateSymbolicLinkPrivilege (or
            // Developer Mode). The test runs where the capability allows and
            // records a skip otherwise; the mechanism under test —
            // `fs::canonicalize` resolving the link to its real target
            // before the containment comparison — is what the permission
            // gates, not the check itself.
            let (prod, qa, temp) = fixture("link");
            let real_target = prod.join("linked-qa");
            std::fs::create_dir_all(&real_target).expect("target dir");
            let link = temp.path().join("qa-link");
            #[cfg(target_os = "windows")]
            let created = std::os::windows::fs::symlink_dir(&real_target, &link).is_ok();
            #[cfg(not(target_os = "windows"))]
            let created = std::os::unix::fs::symlink(&real_target, &link).is_ok();
            if !created {
                eprintln!("skipped: this process may not create symlinks");
                return;
            }
            // The link is outside `prod` lexically but resolves inside it:
            // canonicalization must see through it and refuse.
            assert!(
                select_data_root(Some(link.as_os_str().to_owned()), Ok(prod.clone())).is_err(),
                "a link into the production root must be refused"
            );
            // The honest QA directory still passes alongside the link case.
            assert!(select_data_root(Some(qa.as_os_str().to_owned()), Ok(prod)).is_ok());
        }

        /// A production-root resolution failure to feed the selection layer.
        fn broken_root() -> desktop_todo_maintenance::Error {
            std::io::Error::other("simulated known-folder failure").into()
        }

        #[test]
        fn valid_pre_created_directory_resolves_and_canonicalizes() {
            let (prod, qa, _temp) = fixture("f");
            // An input with redundant segments must come back canonicalized
            // through the standard path API, not hand-rolled normalization.
            let messy = qa
                .parent()
                .unwrap()
                .join("qa")
                .join("..")
                .join("qa");
            let resolved = resolve_qa_data_root(OsStr::new(messy.to_str().unwrap()), Some(&prod))
                .expect("valid qa dir");
            assert_eq!(resolved, std::fs::canonicalize(&qa).unwrap());
        }

        #[test]
        fn empty_and_whitespace_values_are_refused() {
            let (prod, _qa, _temp) = fixture("solo");
            for raw in ["", "   ", "\t"] {
                assert!(
                    resolve_qa_data_root(OsStr::new(raw), Some(&prod)).is_err(),
                    "`{raw}` must be refused"
                );
            }
        }

        #[test]
        fn relative_paths_are_refused() {
            let (prod, _qa, _temp) = fixture("solo");
            assert!(resolve_qa_data_root(OsStr::new("qa-relative"), Some(&prod)).is_err());
            assert!(resolve_qa_data_root(OsStr::new(".\\qa-relative"), Some(&prod)).is_err());
        }

        #[test]
        fn the_production_directory_itself_is_refused() {
            let (prod, _qa, _temp) = fixture("solo");
            assert!(resolve_qa_data_root(OsStr::new(prod.to_str().unwrap()), Some(&prod)).is_err());
        }

        #[test]
        fn a_descendant_of_the_production_directory_is_refused() {
            let (prod, _qa, temp) = fixture("x");
            let nested = prod.join("nested-qa");
            std::fs::create_dir_all(&nested).expect("nested dir");
            assert!(resolve_qa_data_root(OsStr::new(nested.to_str().unwrap()), Some(&prod)).is_err());
            // The temp root itself (the production root's parent) is fine —
            // only equality and containment in the production root are unsafe.
            let sibling = temp.path().join("qa2");
            std::fs::create_dir_all(&sibling).expect("sibling dir");
            assert!(resolve_qa_data_root(OsStr::new(sibling.to_str().unwrap()), Some(&prod)).is_ok());
        }

        #[test]
        fn windows_case_insensitivity_cannot_reintroduce_the_production_root() {
            let (prod, _qa, temp) = fixture("x");
            let upper = temp.path().join("PROD");
            std::fs::create_dir_all(&upper).expect("upper dir");
            // Same directory under a different casing (legal on Windows) must
            // still be recognized as the production root.
            assert!(resolve_qa_data_root(OsStr::new(upper.to_str().unwrap()), Some(&prod)).is_err());
            let upper_nested = prod.join("NESTED-QA");
            std::fs::create_dir_all(&upper_nested).expect("upper nested dir");
            assert!(resolve_qa_data_root(OsStr::new(upper_nested.to_str().unwrap()), Some(&prod)).is_err());
        }

        #[test]
        fn a_file_is_refused_even_though_it_exists() {
            let (prod, _qa, temp) = fixture("x");
            let file = temp.path().join("a-file.txt");
            std::fs::write(&file, b"not a directory").expect("file");
            assert!(resolve_qa_data_root(OsStr::new(file.to_str().unwrap()), Some(&prod)).is_err());
        }

        #[test]
        fn a_missing_directory_is_refused_instead_of_created() {
            let (prod, _qa, temp) = fixture("x");
            let missing = temp.path().join("never-created");
            assert!(resolve_qa_data_root(OsStr::new(missing.to_str().unwrap()), Some(&prod)).is_err());
        }

        #[test]
        fn sibling_names_that_merely_share_a_prefix_stay_allowed() {
            let (prod, _qa, temp) = fixture("x");
            // "prod-2" shares a string prefix with "prod" but is a different
            // directory: the component-wise comparison must not over-refuse.
            let sibling = temp.path().join("prod-2");
            std::fs::create_dir_all(&sibling).expect("sibling dir");
            assert!(resolve_qa_data_root(OsStr::new(sibling.to_str().unwrap()), Some(&prod)).is_ok());
        }
    }
}
