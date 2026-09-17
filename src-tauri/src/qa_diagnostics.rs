use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use tauri::{Manager, WebviewWindow};

pub struct QaDiagnostics {
    log_path: Option<PathBuf>,
    write_lock: Mutex<()>,
    frontend_ready: AtomicBool,
}

impl QaDiagnostics {
    pub fn from_process_args() -> Self {
        let log_path = std::env::current_exe().ok().and_then(|executable| {
            executable.parent().map(|parent| parent.join("qa-diagnostics.log"))
        });
        if let Some(path) = &log_path {
            let _ = File::create(path);
        }
        install_panic_logging(log_path.clone());
        let diagnostics = Self {
            log_path,
            write_lock: Mutex::new(()),
            frontend_ready: AtomicBool::new(false),
        };
        diagnostics.record(format!(
            "qa_session_start=true frontend_asset_mode={}",
            frontend_asset_mode()
        ));
        diagnostics.record("webview_created=false webview_navigation_started=false webview_navigation_completed=false frontend_ready=false");
        diagnostics
    }

    /// Records which rendering backend the app decided to host with.
    ///
    /// Kept as a dedicated log line because "which backend did this build
    /// actually choose" must stay answerable from the log alone; it also
    /// records when a stored document still names the retired Enhanced value.
    pub fn record_rendering_backend(&self, requested: &str, effective: &str, source: &str) {
        self.record(format!(
            "[rendering] requested_backend={requested} effective_backend={effective} selection_source={source}"
        ));
    }

    pub fn record(&self, message: impl AsRef<str>) {
        eprintln!("[qa] {}", message.as_ref());
        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        let Some(path) = &self.log_path else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}", message.as_ref());
        }
    }

    pub fn record_frontend_ready(&self) {
        self.frontend_ready.store(true, Ordering::Release);
        self.record(format!(
            "frontend_ready=true timestamp_ms={}",
            timestamp_ms()
        ));
    }
}

/// Mirrors panic reports into the QA log beside the executable.
///
/// A release build is a GUI-subsystem process (see `main.rs`), so it owns no
/// console and the default panic hook's stderr write reaches nobody: a crash in
/// the shipped binary would be silent for the user *and* for a bug report. The
/// hook appends to the same log file the QA records already use, then delegates
/// to the previous hook, so nothing about panic behavior changes — the process
/// still aborts exactly as before, and debug builds keep their console output.
///
/// This is deliberately the *only* diagnostic that had to be re-homed: every
/// structured record is written by [`QaDiagnostics::record`], which has always
/// targeted the file rather than the console.
fn install_panic_logging(log_path: Option<PathBuf>) {
    let Some(path) = log_path else {
        return;
    };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_panic_record(&path, &sanitize_error(&info.to_string()));
        previous(info);
    }));
}

/// Appends one panic record, tolerating an unreadable or read-only log location.
///
/// A panic handler must never panic: a failed write is dropped, and the record
/// stays single-line so it cannot be mistaken for an unbounded log injection.
pub fn append_panic_record(path: &std::path::Path, message: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "panic={message}");
    }
}

pub fn record_webview_state(window: &WebviewWindow, diagnostics: &QaDiagnostics) {
    let bounds = window.as_ref().bounds();
    let window_visible = window.is_visible();
    diagnostics.record(format!(
        "webview_created=true webview_bounds={} webview_window_visible={}",
        format_bounds(bounds),
        format_result(window_visible)
    ));

    #[cfg(target_os = "windows")]
    {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Controller, ICoreWebView2_2, COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN,
        };
        use windows::{core::Interface, Win32::Foundation::RECT};
        use windows_core::BOOL;

        let app = window.app_handle().clone();
        let outcome = window.with_webview(move |webview| unsafe {
            let diagnostics = app.state::<QaDiagnostics>();
            let controller: ICoreWebView2Controller = webview.controller();
            let mut visible = BOOL(0);
            let mut controller_bounds = RECT::default();
            let visible_result = controller.IsVisible(&mut visible);
            let bounds_result = controller.Bounds(&mut controller_bounds);
            diagnostics.record(format!(
                "webview2_controller_exists=true controller_api=ICoreWebView2Controller webview2_visible={} webview2_bounds={} controller_query_ok={}",
                visible.0 != 0,
                format_rect(controller_bounds),
                visible_result.is_ok() && bounds_result.is_ok()
            ));

            let core = controller.CoreWebView2();
            let runtime_version = core
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|core| core.cast::<ICoreWebView2_2>())
                .and_then(|core| core.Environment())
                .and_then(|environment| {
                    let mut version = windows_core::PWSTR::null();
                    environment.BrowserVersionString(&mut version)?;
                    Ok(webview2_com::take_pwstr(version))
                });
            match runtime_version {
                Ok(version) => diagnostics.record(format!(
                    "[webview2] selected_webview2_runtime=system-selected runtime_version={version}"
                )),
                Err(error) => diagnostics.record(format!(
                    "[webview2] selected_webview2_runtime=system-selected runtime_version=unavailable hresult=0x{:08X}",
                    error.code().0 as u32
                )),
            }
            // Phase 7C.3-B1: `completed_url` is what pinned the release blocker —
            // it showed the release binary navigating to `build.devUrl` instead of
            // the `tauri://localhost` embedded asset protocol. Kept because a
            // dev/production URL mix-up is otherwise invisible: navigation simply
            // fails and the window renders nothing.
            let callback_app = app.clone();
            let handler = webview2_com::NavigationCompletedEventHandler::create(
                Box::new(move |sender, args| {
                    let diagnostics = callback_app.state::<QaDiagnostics>();
                    let mut success = BOOL(0);
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                    let query_ok = if let Some(args) = args {
                        args.IsSuccess(&mut success).is_ok()
                            && args.WebErrorStatus(&mut status).is_ok()
                    } else {
                        false
                    };
                    diagnostics.record(format!(
                        "webview2_navigation_completed=true success={} web_error_status={} status_query_ok={query_ok} completed_url={}",
                        success.0 != 0,
                        status.0,
                        webview_source(sender.as_ref())
                    ));
                    Ok(())
                }),
            );
            let mut token = 0;
            match &core {
                Ok(core) => match core.add_NavigationCompleted(&handler, &mut token) {
                    Ok(()) => diagnostics.record("webview2_navigation_status_hook=true"),
                    Err(error) => diagnostics.record(format!(
                        "webview2_navigation_status_hook=false hresult=0x{:08X}",
                        error.code().0 as u32
                    )),
                },
                Err(error) => diagnostics.record(format!(
                    "webview2_navigation_status_hook=false core_unavailable hresult=0x{:08X}",
                    error.code().0 as u32
                )),
            }

            // Records the navigation target chosen by Tauri and whether any
            // previously registered NavigationStarting handler cancelled it.
            // Observation only; it must never call `put_Cancel`.
            if let Ok(core) = &core {
                diagnostics.record(format!(
                    "webview2_source_url_at_setup={}",
                    webview_source(Some(core))
                ));
            }
            let starting_app = app.clone();
            let starting_handler = webview2_com::NavigationStartingEventHandler::create(Box::new(
                move |_sender, args| {
                    let diagnostics = starting_app.state::<QaDiagnostics>();
                    let mut cancel = BOOL(0);
                    let starting_url = args
                        .as_ref()
                        .and_then(|args| {
                            let mut uri = windows_core::PWSTR::null();
                            args.Uri(&mut uri).ok()?;
                            Some(webview2_com::take_pwstr(uri))
                        })
                        .unwrap_or_else(|| "<unavailable>".into());
                    if let Some(args) = args.as_ref() {
                        let _ = args.Cancel(&mut cancel);
                    }
                    diagnostics.record(format!(
                        "webview2_navigation_starting=true starting_url={starting_url} already_cancelled_by_other_handler={}",
                        cancel.0 != 0
                    ));
                    Ok(())
                },
            ));
            let mut starting_token = 0;
            match &core {
                Ok(core) => {
                    match core.add_NavigationStarting(&starting_handler, &mut starting_token) {
                        Ok(()) => diagnostics.record("webview2_navigation_starting_hook=true"),
                        Err(error) => diagnostics.record(format!(
                            "webview2_navigation_starting_hook=false hresult=0x{:08X}",
                            error.code().0 as u32
                        )),
                    }
                }
                Err(error) => diagnostics.record(format!(
                    "webview2_navigation_starting_hook=false core_unavailable hresult=0x{:08X}",
                    error.code().0 as u32
                )),
            }
        });
        if let Err(error) = outcome {
            diagnostics.record(format!(
                "webview2_controller_query_failed={}",
                sanitize_error(&error.to_string())
            ));
        }
        record_child_hwnd_structure(window, diagnostics);
    }
}

#[tauri::command]
pub fn qa_frontend_ready(state: tauri::State<'_, QaDiagnostics>) {
    state.record_frontend_ready();
}

#[tauri::command]
pub fn qa_frontend_input(state: tauri::State<'_, QaDiagnostics>, kind: &str) {
    // Fixed vocabulary only: this must never carry user-entered text, task
    // titles, or ids. Unknown values collapse to "other" so the command cannot
    // be used as a content channel.
    //
    // `key:<name>` is the one parametrised form. It carries only the DOM
    // `KeyboardEvent.key` value — never the input's contents — because the B3
    // investigation needed to know which key the WebView actually delivered.
    if let Some(key) = kind.strip_prefix("key:") {
        state.record(format!("frontend_key_received={key}"));
        return;
    }
    let kind = match kind {
        "pointerdown" => "pointerdown",
        "contextmenu" => "contextmenu",
        // Phase 7C.3-B3 interaction tracing.
        "add_submit_enter" => "add_submit_enter",
        "add_submit_click" => "add_submit_click",
        "add_empty_title" => "add_empty_title",
        "add_invoke_start" => "add_invoke_start",
        "add_invoke_ok" => "add_invoke_ok",
        "add_invoke_err" => "add_invoke_err",
        "add_reload_ok" => "add_reload_ok",
        "drag_pointerdown" => "drag_pointerdown",
        "drag_pointermove" => "drag_pointermove",
        "drag_pointerup" => "drag_pointerup",
        "drag_reorder_start" => "drag_reorder_start",
        "drag_reorder_ok" => "drag_reorder_ok",
        "drag_reorder_err" => "drag_reorder_err",
        "drag_pointercancel" => "drag_pointercancel",
        _ => "other",
    };
    state.record(format!("frontend_input_received={kind}"));
}

pub fn frontend_asset_mode() -> &'static str {
    if cfg!(dev) {
        "dev-server"
    } else {
        "embedded"
    }
}

/// True when this binary serves the embedded frontend from the `tauri://localhost`
/// asset protocol rather than navigating to `build.devUrl`.
///
/// Tauri derives this from the `custom-protocol` cargo feature at compile time.
pub fn serves_embedded_frontend() -> bool {
    !cfg!(dev)
}

/// Warns when an optimized build will navigate to the dev server.
///
/// Tauri decides dev-vs-production from the `custom-protocol` feature, **not** from
/// the cargo profile: any build without that feature is a dev build even with
/// `--release`. Such a binary renders nothing whenever the dev server is absent,
/// which is exactly how a release artifact silently shipped broken once. The
/// condition is detected here so it is loud instead.
///
/// It goes through [`QaDiagnostics::record`] rather than a bare `eprintln!`: a
/// release build is a GUI-subsystem process with no console (see `main.rs`), so
/// stderr alone would make this warning invisible in exactly the build it exists
/// to protect.
pub fn warn_if_release_without_embedded_frontend(diagnostics: &QaDiagnostics) {
    if cfg!(debug_assertions) || serves_embedded_frontend() {
        return;
    }
    diagnostics.record(
        "[frontend] WARNING release_profile_without_custom_protocol=true \
         frontend_asset_mode=dev-server \
         this binary will navigate to build.devUrl and render nothing without a dev server; \
         build with: cargo build --release --features custom-protocol",
    );
}

fn format_bounds(bounds: tauri::Result<tauri::Rect>) -> String {
    match bounds {
        Ok(bounds) => format!("{:?}", bounds),
        Err(error) => format!("unavailable({})", sanitize_error(&error.to_string())),
    }
}

fn format_result(result: tauri::Result<bool>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(error) => format!("unavailable({})", sanitize_error(&error.to_string())),
    }
}

#[cfg(target_os = "windows")]
fn format_rect(rect: windows::Win32::Foundation::RECT) -> String {
    format!(
        "{},{},{}x{}",
        rect.left,
        rect.top,
        (rect.right - rect.left).max(0),
        (rect.bottom - rect.top).max(0)
    )
}

fn sanitize_error(error: &str) -> String {
    error.replace(['\r', '\n'], " ")
}

/// Reads `ICoreWebView2::Source` for navigation diagnostics.
#[cfg(target_os = "windows")]
fn webview_source(core: Option<&webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2>) -> String {
    let Some(core) = core else {
        return "<unavailable>".into();
    };
    let mut source = windows_core::PWSTR::null();
    match unsafe { core.Source(&mut source) } {
        Ok(()) => {
            let value = webview2_com::take_pwstr(source);
            if value.is_empty() {
                "<empty>".into()
            } else {
                value
            }
        }
        Err(error) => format!("<error 0x{:08X}>", error.code().0 as u32),
    }
}

pub fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn record_child_hwnd_structure(window: &WebviewWindow, diagnostics: &QaDiagnostics) {
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumChildWindows, GetClassNameW, GetParent, GetWindow, IsWindowVisible, GW_HWNDNEXT,
            GW_HWNDPREV,
        },
    };
    use windows_core::BOOL;

    unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
        let children = &mut *(state.0 as *mut Vec<HWND>);
        children.push(hwnd);
        BOOL(1)
    }

    let Ok(root) = window.hwnd() else {
        diagnostics.record("hwnd_structure_available=false");
        return;
    };
    let root = HWND(root.0);
    let mut descendants = Vec::new();
    unsafe {
        let _ = EnumChildWindows(
            Some(root),
            Some(collect),
            LPARAM((&mut descendants as *mut Vec<HWND>).cast::<c_void>() as isize),
        );
    }
    diagnostics.record(format!(
        "top_level_hwnd=0x{:X} immediate_child_count={}",
        root.0 as usize,
        descendants
            .iter()
            .filter(|child| unsafe { GetParent(**child).ok() == Some(root) })
            .count()
    ));
    for (z_index, child) in descendants
        .into_iter()
        .filter(|child| unsafe { GetParent(*child).ok() == Some(root) })
        .enumerate()
    {
        let mut class_name = [0_u16; 256];
        let length = unsafe { GetClassNameW(child, &mut class_name) }.max(0) as usize;
        let class_name = String::from_utf16_lossy(&class_name[..length]);
        let previous = unsafe { GetWindow(child, GW_HWNDPREV).ok() };
        let next = unsafe { GetWindow(child, GW_HWNDNEXT).ok() };
        diagnostics.record(format!(
            "child_hwnd=0x{:X} class={} parent=0x{:X} visible={} z_index={} z_previous={} z_next={}",
            child.0 as usize,
            class_name,
            root.0 as usize,
            unsafe { IsWindowVisible(child).as_bool() },
            z_index,
            format_optional_hwnd(previous),
            format_optional_hwnd(next)
        ));
    }
}

#[cfg(target_os = "windows")]
fn format_optional_hwnd(hwnd: Option<windows::Win32::Foundation::HWND>) -> String {
    hwnd.map_or_else(
        || "none".into(),
        |value| format!("0x{:X}", value.0 as usize),
    )
}

#[cfg(test)]
mod tests {
    use super::append_panic_record;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_log(name: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "alan-desktop-{name}-{}-{suffix}.log",
            std::process::id()
        ))
    }

    /// Release builds have no console, so a panic has to reach the log file or it
    /// reaches nobody. Records append (a crash after other records must not
    /// truncate them) and stay single-line.
    #[test]
    fn panic_records_append_to_the_qa_log() {
        let path = temp_log("qa-panic");
        append_panic_record(&path, "first");
        append_panic_record(&path, "second\nline");
        let content = std::fs::read_to_string(&path).expect("log file");
        assert_eq!(content, "panic=first\npanic=second\nline\n");
        let _ = std::fs::remove_file(&path);
    }

    /// The handler must survive an unusable log location and must not create
    /// directories on its own: a panic while reporting a panic is unacceptable.
    #[test]
    fn panic_records_tolerate_an_unwritable_location() {
        let directory = temp_log("qa-panic-dir");
        std::fs::create_dir_all(&directory).expect("temp directory");
        append_panic_record(&directory, "a directory is not a log file");
        append_panic_record(&directory.join("missing").join("nested.log"), "no parent");
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }
}
