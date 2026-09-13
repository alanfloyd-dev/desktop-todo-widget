use serde::Serialize;
use std::{collections::VecDeque, sync::Mutex, time::SystemTime};
use tauri::WebviewWindow;

#[cfg(target_os = "windows")]
use std::sync::{
    atomic::{AtomicBool, AtomicIsize, Ordering},
    OnceLock,
};

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    Desktop,
    #[default]
    Normal,
    AlwaysOnTop,
}

impl TryFrom<&str> for WindowMode {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "desktop" => Ok(Self::Desktop),
            "normal" => Ok(Self::Normal),
            "always_on_top" => Ok(Self::AlwaysOnTop),
            _ => Err(format!("unknown window mode: {value}")),
        }
    }
}

struct StoredNativeState {
    mode: WindowMode,
    original_style: Option<isize>,
    original_ex_style: Option<isize>,
    shell_strategy: String,
    warning: Option<String>,
    last_detach: DetachDiagnostics,
    last_attach: AttachDiagnostics,
    events: VecDeque<String>,
}

impl Default for StoredNativeState {
    fn default() -> Self {
        Self {
            mode: WindowMode::Normal,
            original_style: None,
            original_ex_style: None,
            shell_strategy: "tauri-normal-window".into(),
            warning: None,
            last_detach: DetachDiagnostics::default(),
            last_attach: AttachDiagnostics::default(),
            events: VecDeque::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct DesktopWidgetRequest {
    pub bounds: Option<DesktopBounds>,
    pub default_width: u32,
    pub default_height: u32,
    pub margin: i32,
    pub tauri_scale_factor: f64,
    pub logical_bounds: String,
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
enum DesktopPlacement {
    PreserveCurrent,
    Widget(DesktopWidgetRequest),
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachDiagnostics {
    status: String,
    hwnd: String,
    parent_before: String,
    parent_target: String,
    parent_after: String,
    style_before: String,
    style_after: String,
    ex_style_before: String,
    ex_style_after: String,
    bounds_before: String,
    bounds_after_parent: String,
    bounds_final: String,
    desktop_target_rect: String,
    desktop_client_rect: String,
    tauri_scale_factor: String,
    window_dpi: u32,
    logical_bounds: String,
    physical_requested: String,
}

impl Default for AttachDiagnostics {
    fn default() -> Self {
        Self {
            status: "not-run".into(),
            hwnd: "not-run".into(),
            parent_before: "not-run".into(),
            parent_target: "not-run".into(),
            parent_after: "not-run".into(),
            style_before: "not-run".into(),
            style_after: "not-run".into(),
            ex_style_before: "not-run".into(),
            ex_style_after: "not-run".into(),
            bounds_before: "not-run".into(),
            bounds_after_parent: "not-run".into(),
            bounds_final: "not-run".into(),
            desktop_target_rect: "not-run".into(),
            desktop_client_rect: "not-run".into(),
            tauri_scale_factor: "not-run".into(),
            window_dpi: 0,
            logical_bounds: "not-run".into(),
            physical_requested: "not-run".into(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachDiagnostics {
    attempt: u32,
    status: String,
    set_parent_return: String,
    last_error_before: u32,
    last_error_after: u32,
    style_before: String,
    style_after: String,
    ex_style_before: String,
    ex_style_after: String,
    parent_before: String,
    parent_after_set_parent: String,
    parent_after: String,
    visible_before: bool,
    visible_after: bool,
    bounds_before: String,
    bounds_after: String,
}

impl Default for DetachDiagnostics {
    fn default() -> Self {
        Self {
            attempt: 0,
            status: "not-run".into(),
            set_parent_return: "not-run".into(),
            last_error_before: 0,
            last_error_after: 0,
            style_before: "not-run".into(),
            style_after: "not-run".into(),
            ex_style_before: "not-run".into(),
            ex_style_after: "not-run".into(),
            parent_before: "not-run".into(),
            parent_after_set_parent: "not-run".into(),
            parent_after: "not-run".into(),
            visible_before: false,
            visible_after: false,
            bounds_before: "not-run".into(),
            bounds_after: "not-run".into(),
        }
    }
}

#[derive(Default)]
pub struct NativeWindowState(Mutex<StoredNativeState>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    name: Option<String>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeReport {
    mode: WindowMode,
    hwnd: String,
    parent_hwnd: String,
    parent_class: String,
    shell_strategy: String,
    monitor_count: usize,
    monitors: Vec<MonitorInfo>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    always_on_top: bool,
    style: String,
    ex_style: String,
    parent_style: String,
    parent_ex_style: String,
    hit_target_hwnd: String,
    hit_target_class: String,
    hit_target_owned: bool,
    focus_hwnd: String,
    focus_class: String,
    foreground_hwnd: String,
    input_route: String,
    hwnd_valid: bool,
    current_desktop_hwnd: String,
    is_window_visible: bool,
    attachment_valid: bool,
    recovery_count: u32,
    recovery_reason: String,
    /// Widget window semantics: the taskbar/Alt+Tab membership of the product
    /// window. Must report `taskbarEligible: false` in every window mode.
    widget_frame: WidgetFrameFacts,
    win_d_trace: WinDTraceReport,
    detach: DetachDiagnostics,
    attach: AttachDiagnostics,
    warning: Option<String>,
    events: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinDTraceReport {
    watcher_running: bool,
    foreground_hook_installed: bool,
    object_hook_installed: bool,
    shell_hook_registered: bool,
    event_count: u64,
    last_event: String,
    trace_active: bool,
    trace_mode: String,
    observer_status: String,
    observer_passed: bool,
    observer_issues: Vec<String>,
    recovery_mutation_count: u64,
    current_snapshot: String,
    samples: Vec<String>,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TraceMode {
    #[default]
    Idle,
    ObserverControl,
    WinD,
}

#[cfg(target_os = "windows")]
impl TraceMode {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::ObserverControl => "observer-control",
            Self::WinD => "win-d",
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WidgetInvariant {
    hwnd: win32::Hwnd,
    parent: win32::Hwnd,
    style: isize,
    ex_style: isize,
    rect: (i32, i32, i32, i32),
    visible: bool,
    iconic: bool,
    enabled: bool,
    z_prev: win32::Hwnd,
    z_next: win32::Hwnd,
    hit_target: win32::Hwnd,
    hit_owned: bool,
}

struct NativeDiagnostics {
    hwnd: String,
    parent_hwnd: String,
    parent_class: String,
    style: String,
    ex_style: String,
    parent_style: String,
    parent_ex_style: String,
    hit_target_hwnd: String,
    hit_target_class: String,
    hit_target_owned: bool,
    focus_hwnd: String,
    focus_class: String,
    foreground_hwnd: String,
    input_route: String,
    hwnd_valid: bool,
    current_desktop_hwnd: String,
    is_window_visible: bool,
    attachment_valid: bool,
    recovery_count: u32,
    recovery_reason: String,
    /// Widget window semantics, verifiable from the report instead of inferred
    /// from a raw style dump. See `platform::windows::widget_frame`.
    widget_frame: WidgetFrameFacts,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Taskbar / Alt+Tab membership of the product HWND.
///
/// `taskbar_eligible` is the documented Shell placement rule evaluated against
/// the live window, so `false` is the widget contract holding. Alt+Tab is listed
/// separately because on Windows the two are decided by the same
/// `WS_EX_TOOLWINDOW` test but are otherwise unrelated mechanisms: a window can
/// be off the taskbar (owned, or `ITaskbarList`-removed) and still appear in
/// Alt+Tab.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WidgetFrameFacts {
    /// `WS_EX_TOOLWINDOW` is present, the Win32 "not an application window" bit.
    tool_window: bool,
    /// `WS_EX_APPWINDOW` is absent, the bit that forces a taskbar button.
    app_window_cleared: bool,
    taskbar_eligible: bool,
    /// Windows lists exactly the taskbar-eligible top-level windows in Alt+Tab,
    /// so this is the Alt+Tab answer for this window.
    alt_tab_eligible: bool,
    /// Window mode the facts were captured in, so a gate can assert per mode.
    mode: WindowMode,
}

#[cfg(not(target_os = "windows"))]
impl NativeDiagnostics {
    fn unsupported() -> Self {
        Self {
            hwnd: "unsupported".into(),
            parent_hwnd: "unsupported".into(),
            parent_class: "unsupported".into(),
            style: "unsupported".into(),
            ex_style: "unsupported".into(),
            parent_style: "unsupported".into(),
            parent_ex_style: "unsupported".into(),
            hit_target_hwnd: "unsupported".into(),
            hit_target_class: "unsupported".into(),
            hit_target_owned: false,
            focus_hwnd: "unsupported".into(),
            focus_class: "unsupported".into(),
            foreground_hwnd: "unsupported".into(),
            input_route: "unsupported".into(),
            hwnd_valid: false,
            current_desktop_hwnd: "unsupported".into(),
            is_window_visible: false,
            attachment_valid: false,
            recovery_count: 0,
            recovery_reason: "unsupported".into(),
            widget_frame: WidgetFrameFacts::default(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "unknown-time".to_string())
}

fn record_event(state: &mut StoredNativeState, event: impl Into<String>) {
    state
        .events
        .push_front(format!("{} · {}", timestamp(), event.into()));
    state.events.truncate(12);
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct DesktopLifecycleState {
    requested: bool,
    generation: u64,
    app_hwnd: win32::Hwnd,
    current_def_view: win32::Hwnd,
    last_bounds: Option<(i32, i32, i32, i32)>,
    attachment_valid: bool,
    recovery_count: u32,
    recovery_reason: String,
    pending_reason: String,
    events: VecDeque<String>,
    watcher_running: bool,
    foreground_hook_installed: bool,
    object_hook_installed: bool,
    win_event_count: u64,
    last_win_event: String,
    trace_generation: u64,
    trace_active: bool,
    trace_mode: TraceMode,
    trace_samples: VecDeque<String>,
    observer_status: String,
    observer_passed: bool,
    observer_issues: Vec<String>,
    observer_baseline: Option<WidgetInvariant>,
    observer_recovery_mutation_baseline: u64,
    recovery_mutation_count: u64,
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct DesktopLifecycleSnapshot {
    requested: bool,
    generation: u64,
    last_bounds: Option<(i32, i32, i32, i32)>,
    recovery_count: u32,
    recovery_reason: String,
}

#[cfg(target_os = "windows")]
static DESKTOP_LIFECYCLE: OnceLock<Mutex<DesktopLifecycleState>> = OnceLock::new();
#[cfg(target_os = "windows")]
static DESKTOP_MONITOR_STARTED: OnceLock<()> = OnceLock::new();
#[cfg(target_os = "windows")]
static DESKTOP_REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static DESKTOP_HWND: AtomicIsize = AtomicIsize::new(0);
#[cfg(target_os = "windows")]
static CURRENT_DEF_VIEW: AtomicIsize = AtomicIsize::new(0);
#[cfg(target_os = "windows")]
static DESKTOP_TRANSITION: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn desktop_lifecycle() -> &'static Mutex<DesktopLifecycleState> {
    DESKTOP_LIFECYCLE.get_or_init(|| Mutex::new(DesktopLifecycleState::default()))
}

#[cfg(target_os = "windows")]
fn desktop_transition() -> &'static Mutex<()> {
    DESKTOP_TRANSITION.get_or_init(|| Mutex::new(()))
}

#[cfg(target_os = "windows")]
fn lifecycle_events() -> Vec<String> {
    desktop_lifecycle()
        .lock()
        .map(|state| state.events.iter().cloned().collect())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn win_d_trace_report() -> WinDTraceReport {
    let current_snapshot = desktop_topology_snapshot("current");
    desktop_lifecycle()
        .lock()
        .map(|state| WinDTraceReport {
            watcher_running: state.watcher_running,
            foreground_hook_installed: state.foreground_hook_installed,
            object_hook_installed: state.object_hook_installed,
            // This PoC deliberately observes accessibility WinEvents. It does
            // not register a Shell hook or intercept the Win+D key chord.
            shell_hook_registered: false,
            event_count: state.win_event_count,
            last_event: state.last_win_event.clone(),
            trace_active: state.trace_active,
            trace_mode: state.trace_mode.label().into(),
            observer_status: state.observer_status.clone(),
            observer_passed: state.observer_passed,
            observer_issues: state.observer_issues.clone(),
            recovery_mutation_count: state.recovery_mutation_count,
            current_snapshot,
            samples: state.trace_samples.iter().cloned().collect(),
        })
        .unwrap_or(WinDTraceReport {
            watcher_running: false,
            foreground_hook_installed: false,
            object_hook_installed: false,
            shell_hook_registered: false,
            event_count: 0,
            last_event: "lifecycle state unavailable".into(),
            trace_active: false,
            trace_mode: "unavailable".into(),
            observer_status: "unavailable".into(),
            observer_passed: false,
            observer_issues: Vec::new(),
            recovery_mutation_count: 0,
            current_snapshot: "unavailable".into(),
            samples: Vec::new(),
        })
}

#[cfg(not(target_os = "windows"))]
fn win_d_trace_report() -> WinDTraceReport {
    WinDTraceReport {
        watcher_running: false,
        foreground_hook_installed: false,
        object_hook_installed: false,
        shell_hook_registered: false,
        event_count: 0,
        last_event: "unsupported".into(),
        trace_active: false,
        trace_mode: "unsupported".into(),
        observer_status: "unsupported".into(),
        observer_passed: false,
        observer_issues: Vec::new(),
        recovery_mutation_count: 0,
        current_snapshot: "unsupported".into(),
        samples: Vec::new(),
    }
}

#[cfg(target_os = "windows")]
fn record_lifecycle_event(event: impl Into<String>) {
    let rendered = format!("{} · {}", timestamp(), event.into());
    eprintln!("[desktop-lifecycle] {rendered}");
    if let Ok(mut state) = desktop_lifecycle().lock() {
        state.events.push_front(rendered);
        state.events.truncate(12);
    }
}

#[cfg(target_os = "windows")]
fn observer_control_active() -> bool {
    desktop_lifecycle()
        .lock()
        .map(|state| state.trace_active && state.trace_mode == TraceMode::ObserverControl)
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn lifecycle_snapshot() -> DesktopLifecycleSnapshot {
    desktop_lifecycle()
        .lock()
        .map(|state| DesktopLifecycleSnapshot {
            requested: state.requested,
            generation: state.generation,
            last_bounds: state.last_bounds,
            recovery_count: state.recovery_count,
            recovery_reason: state.recovery_reason.clone(),
        })
        .unwrap_or(DesktopLifecycleSnapshot {
            requested: false,
            generation: 0,
            last_bounds: None,
            recovery_count: 0,
            recovery_reason: "lifecycle state unavailable".into(),
        })
}

#[cfg(target_os = "windows")]
fn set_desktop_requested(requested: bool, hwnd: win32::Hwnd) {
    DESKTOP_REQUESTED.store(requested, Ordering::Release);
    DESKTOP_HWND.store(hwnd, Ordering::Release);
    if let Ok(mut state) = desktop_lifecycle().lock() {
        state.requested = requested;
        state.app_hwnd = hwnd;
        state.generation = state.generation.wrapping_add(1);
        if !requested {
            state.attachment_valid = true;
            state.pending_reason.clear();
        }
    }
}

#[tauri::command]
pub fn set_window_mode(
    window: WebviewWindow,
    state: tauri::State<'_, NativeWindowState>,
    mode: &str,
) -> Result<ModeReport, String> {
    let mode = WindowMode::try_from(mode)?;
    let mut native = state.0.lock().map_err(|_| "window state poisoned")?;

    #[cfg(target_os = "windows")]
    apply_windows_mode(
        &window,
        &mut native,
        mode,
        DesktopPlacement::PreserveCurrent,
    )?;

    #[cfg(not(target_os = "windows"))]
    {
        let _ = &window;
        native.warning = Some("Desktop mode is only implemented on Windows.".into());
        native.shell_strategy = "unsupported-platform".into();
        native.mode = mode;
    }

    record_event(&mut native, format!("mode changed to {mode:?}"));
    build_report(&window, &native)
}

#[tauri::command]
pub fn window_diagnostics(
    window: WebviewWindow,
    state: tauri::State<'_, NativeWindowState>,
) -> Result<ModeReport, String> {
    let native = state.0.lock().map_err(|_| "window state poisoned")?;
    build_report(&window, &native)
}

/// Privacy-minimized native fields used by the public issue diagnostic. The
/// larger Phase 1 report intentionally stays behind the Developer expander.
pub struct SupportWindowSnapshot {
    pub parent_class: String,
    pub attachment_valid: bool,
    pub always_on_top: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn support_window_snapshot(
    window: &WebviewWindow,
    state: &NativeWindowState,
) -> Result<SupportWindowSnapshot, String> {
    let native = state.0.lock().map_err(|_| "window state poisoned")?;
    let report = build_report(window, &native)?;
    Ok(SupportWindowSnapshot {
        parent_class: report.parent_class,
        attachment_valid: report.attachment_valid,
        always_on_top: report.always_on_top,
        x: report.x,
        y: report.y,
        width: report.width,
        height: report.height,
    })
}

/// Product-layer entry point for the proven Phase 1 native adapter.
/// `normal` means a regular top-level HWND; user-facing naming lives in the
/// Phase 2 product orchestrator.
pub fn apply_adapter_mode(
    window: &WebviewWindow,
    state: &NativeWindowState,
    mode: &str,
) -> Result<(), String> {
    let mode = WindowMode::try_from(mode)?;
    let mut native = state.0.lock().map_err(|_| "window state poisoned")?;

    #[cfg(target_os = "windows")]
    apply_windows_mode(window, &mut native, mode, DesktopPlacement::PreserveCurrent)?;

    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        native.mode = mode;
    }

    record_event(
        &mut native,
        format!("product mode adapter changed to {mode:?}"),
    );
    Ok(())
}

/// Product Desktop entry point. The Phase 1 adapter still owns Shell
/// discovery/reparenting. Product Desktop supplies an independent widget rect;
/// the adapter clamps it within the discovered SHELLDLL_DefView client area.
pub fn apply_adapter_desktop_mode(
    window: &WebviewWindow,
    state: &NativeWindowState,
    request: DesktopWidgetRequest,
) -> Result<DesktopBounds, String> {
    let mut native = state.0.lock().map_err(|_| "window state poisoned")?;

    #[cfg(target_os = "windows")]
    {
        apply_windows_mode(
            window,
            &mut native,
            WindowMode::Desktop,
            DesktopPlacement::Widget(request),
        )?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        native.mode = WindowMode::Desktop;
    }

    record_event(
        &mut native,
        "product mode adapter changed to Desktop with bounded widget geometry",
    );
    current_desktop_bounds(window)
}

pub fn current_desktop_bounds(window: &WebviewWindow) -> Result<DesktopBounds, String> {
    #[cfg(target_os = "windows")]
    {
        desktop_bounds_for_hwnd(hwnd_value(window)?)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let position = window.outer_position().map_err(|error| error.to_string())?;
        let size = window.outer_size().map_err(|error| error.to_string())?;
        Ok(DesktopBounds {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        })
    }
}

pub fn desktop_window_scale_factor(window: &WebviewWindow) -> Result<f64, String> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: the HWND is owned by the live Tauri window and
        // GetDpiForWindow performs a read-only query without retaining it.
        let dpi = unsafe { win32::GetDpiForWindow(hwnd_value(window)?) };
        if dpi == 0 {
            window.scale_factor().map_err(|error| error.to_string())
        } else {
            Ok(dpi as f64 / 96.0)
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        window.scale_factor().map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub fn start_win_d_trace(window: WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if !DESKTOP_REQUESTED.load(Ordering::Acquire) {
            return Err("Win+D trace can only be armed while Desktop mode is active".into());
        }
        let hwnd = hwnd_value(&window)?;
        // SAFETY: `hwnd` came from Tauri; IsWindow only validates the opaque
        // handle and does not dereference application-owned memory.
        if hwnd == 0 || unsafe { win32::IsWindow(hwnd) } == 0 {
            return Err("Win+D trace cannot resolve the Widget HWND".into());
        }

        // Serialize only the observer baseline setup with recovery. Any
        // recovery already in flight finishes before the baseline; subsequent
        // recovery sees ObserverControl and is forced down the read-only path.
        let observer_transition = desktop_transition()
            .lock()
            .map_err(|_| "desktop transition lock poisoned")?;
        let baseline = capture_widget_invariant(hwnd);
        let (generation, trace_mode) = {
            let mut state = desktop_lifecycle()
                .lock()
                .map_err(|_| "lifecycle state unavailable")?;
            if state.trace_active {
                return Err("a read-only trace is already active".into());
            }
            state.trace_generation = state.trace_generation.wrapping_add(1);
            state.trace_active = true;
            state.trace_samples.clear();
            state.observer_issues.clear();
            state.trace_mode = if state.observer_passed {
                TraceMode::WinD
            } else {
                TraceMode::ObserverControl
            };
            if state.trace_mode == TraceMode::ObserverControl {
                state.observer_status = "running · do not press Win+D for 10 seconds".into();
                state.observer_baseline = Some(baseline);
                state.observer_recovery_mutation_baseline = state.recovery_mutation_count;
            }
            (state.trace_generation, state.trace_mode)
        };
        drop(observer_transition);
        record_win_d_sample(generation, "armed-before");
        record_lifecycle_event(match trace_mode {
            TraceMode::ObserverControl => {
                "observer control armed · 10 seconds · recovery writes suspended"
            }
            TraceMode::WinD => "Win+D read-only trace armed · 10 seconds · no key interception",
            TraceMode::Idle => "read-only trace armed",
        });

        std::thread::spawn(move || {
            let schedule = [
                (250_u64, "t+250ms"),
                (500, "t+750ms"),
                (750, "t+1500ms"),
                (1_500, "t+3000ms"),
                (2_000, "t+5000ms"),
                (2_500, "t+7500ms"),
                (2_500, "t+10000ms-after"),
            ];
            for (delay_ms, label) in schedule {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                if !record_win_d_sample(generation, label) {
                    return;
                }
            }
            finalize_read_only_trace(generation);
        });
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        Err("Win+D trace is only available on Windows".into())
    }
}

fn build_report(window: &WebviewWindow, state: &StoredNativeState) -> Result<ModeReport, String> {
    let monitors = window
        .available_monitors()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|monitor| MonitorInfo {
            name: monitor.name().cloned(),
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
            scale_factor: monitor.scale_factor(),
        })
        .collect::<Vec<_>>();

    #[cfg(target_os = "windows")]
    let native_diagnostics = windows_diagnostics(window)?;

    #[cfg(not(target_os = "windows"))]
    let native_diagnostics = NativeDiagnostics::unsupported();

    #[cfg(target_os = "windows")]
    let (x, y, width, height) = (
        native_diagnostics.x,
        native_diagnostics.y,
        native_diagnostics.width,
        native_diagnostics.height,
    );

    #[cfg(not(target_os = "windows"))]
    let (x, y, width, height) = {
        let position = window.outer_position().map_err(|error| error.to_string())?;
        let size = window.outer_size().map_err(|error| error.to_string())?;
        (position.x, position.y, size.width, size.height)
    };

    let mut events = Vec::new();
    #[cfg(target_os = "windows")]
    events.extend(lifecycle_events());
    events.extend(state.events.iter().cloned());
    events.truncate(16);

    Ok(ModeReport {
        mode: state.mode,
        hwnd: native_diagnostics.hwnd,
        parent_hwnd: native_diagnostics.parent_hwnd,
        parent_class: native_diagnostics.parent_class,
        shell_strategy: state.shell_strategy.clone(),
        monitor_count: monitors.len(),
        monitors,
        x,
        y,
        width,
        height,
        always_on_top: window.is_always_on_top().unwrap_or(false),
        style: native_diagnostics.style,
        ex_style: native_diagnostics.ex_style,
        parent_style: native_diagnostics.parent_style,
        parent_ex_style: native_diagnostics.parent_ex_style,
        hit_target_hwnd: native_diagnostics.hit_target_hwnd,
        hit_target_class: native_diagnostics.hit_target_class,
        hit_target_owned: native_diagnostics.hit_target_owned,
        focus_hwnd: native_diagnostics.focus_hwnd,
        focus_class: native_diagnostics.focus_class,
        foreground_hwnd: native_diagnostics.foreground_hwnd,
        input_route: native_diagnostics.input_route,
        hwnd_valid: native_diagnostics.hwnd_valid,
        current_desktop_hwnd: native_diagnostics.current_desktop_hwnd,
        is_window_visible: native_diagnostics.is_window_visible,
        attachment_valid: native_diagnostics.attachment_valid,
        recovery_count: native_diagnostics.recovery_count,
        recovery_reason: native_diagnostics.recovery_reason,
        widget_frame: WidgetFrameFacts {
            mode: state.mode,
            ..native_diagnostics.widget_frame
        },
        win_d_trace: win_d_trace_report(),
        detach: state.last_detach.clone(),
        attach: state.last_attach.clone(),
        warning: state.warning.clone(),
        events,
    })
}

#[cfg(target_os = "windows")]
mod win32 {
    pub type Hwnd = isize;
    pub type Bool = i32;
    pub type Lparam = isize;
    pub type Wparam = usize;
    pub type Lresult = isize;
    pub type Dword = u32;
    pub type HwinEventHook = isize;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct GuiThreadInfo {
        pub size: Dword,
        pub flags: Dword,
        pub active: Hwnd,
        pub focus: Hwnd,
        pub capture: Hwnd,
        pub menu_owner: Hwnd,
        pub move_size: Hwnd,
        pub caret: Hwnd,
        pub caret_rect: Rect,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Msg {
        pub hwnd: Hwnd,
        pub message: u32,
        pub wparam: Wparam,
        pub lparam: Lparam,
        pub time: Dword,
        pub point: Point,
        pub private: Dword,
    }

    pub const GWL_STYLE: i32 = -16;
    pub const GWL_EXSTYLE: i32 = -20;
    pub const WS_CHILD: isize = 0x4000_0000;
    pub const WS_POPUP: isize = 0x8000_0000u32 as isize;
    pub const WS_CLIPSIBLINGS: isize = 0x0400_0000;
    pub const WS_TABSTOP: isize = 0x0001_0000;
    pub const WS_BORDER: isize = 0x0080_0000;
    pub const WS_DLGFRAME: isize = 0x0040_0000;
    pub const WS_CAPTION: isize = WS_BORDER | WS_DLGFRAME;
    pub const WS_THICKFRAME: isize = 0x0004_0000;
    pub const WS_SYSMENU: isize = 0x0008_0000;
    pub const WS_MINIMIZEBOX: isize = 0x0002_0000;
    pub const WS_EX_TOPMOST: isize = 0x0000_0008;
    pub const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    /// Win32 "this is not an application window": no taskbar button, no Alt+Tab
    /// entry. Applied to every product window mode by `widget_frame`.
    pub const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    pub const WS_EX_APPWINDOW: isize = 0x0004_0000;
    pub const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    pub const HWND_TOP: Hwnd = 0;
    pub const HWND_NOTOPMOST: Hwnd = -2;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const SWP_FRAMECHANGED: u32 = 0x0020;
    pub const SWP_SHOWWINDOW: u32 = 0x0040;
    pub const SMTO_ABORTIFHUNG: u32 = 0x0002;
    pub const PROGMAN_SPAWN_WORKERW: u32 = 0x052C;
    pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    pub const EVENT_OBJECT_CREATE: u32 = 0x8000;
    pub const EVENT_OBJECT_DESTROY: u32 = 0x8001;
    pub const EVENT_OBJECT_SHOW: u32 = 0x8002;
    pub const EVENT_OBJECT_HIDE: u32 = 0x8003;
    pub const EVENT_OBJECT_REORDER: u32 = 0x8004;
    pub const EVENT_OBJECT_PARENTCHANGE: u32 = 0x800F;
    pub const OBJID_WINDOW: i32 = 0;
    pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
    pub const GW_HWNDNEXT: u32 = 2;
    pub const GW_HWNDPREV: u32 = 3;
    pub const GW_OWNER: u32 = 4;
    pub const WM_TIMER: u32 = 0x0113;
    pub const LIFECYCLE_TIMER_ID: usize = 0xA1A0;
    pub const LIFECYCLE_DEBOUNCE_MS: u32 = 220;

    pub type EnumWindowsProc = Option<unsafe extern "system" fn(Hwnd, Lparam) -> Bool>;
    pub type WinEventProc =
        Option<unsafe extern "system" fn(HwinEventHook, Dword, Hwnd, i32, i32, Dword, Dword)>;

    #[link(name = "user32")]
    extern "system" {
        pub fn EnumWindows(callback: EnumWindowsProc, lparam: Lparam) -> Bool;
        pub fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
        pub fn FindWindowExW(
            parent: Hwnd,
            child_after: Hwnd,
            class_name: *const u16,
            window_name: *const u16,
        ) -> Hwnd;
        pub fn GetClassNameW(hwnd: Hwnd, class_name: *mut u16, max_count: i32) -> i32;
        pub fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        pub fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max_count: i32) -> i32;
        pub fn GetParent(hwnd: Hwnd) -> Hwnd;
        pub fn GetWindow(hwnd: Hwnd, command: u32) -> Hwnd;
        pub fn GetForegroundWindow() -> Hwnd;
        pub fn GetShellWindow() -> Hwnd;
        pub fn GetGUIThreadInfo(thread_id: Dword, info: *mut GuiThreadInfo) -> Bool;
        pub fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        pub fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
        pub fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
        pub fn GetDpiForWindow(hwnd: Hwnd) -> u32;
        pub fn ScreenToClient(hwnd: Hwnd, point: *mut Point) -> Bool;
        pub fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut Dword) -> Dword;
        pub fn IsWindow(hwnd: Hwnd) -> Bool;
        pub fn IsWindowVisible(hwnd: Hwnd) -> Bool;
        pub fn IsIconic(hwnd: Hwnd) -> Bool;
        pub fn IsChild(parent: Hwnd, child: Hwnd) -> Bool;
        pub fn IsWindowEnabled(hwnd: Hwnd) -> Bool;
        pub fn EnableWindow(hwnd: Hwnd, enable: Bool) -> Bool;
        pub fn SendMessageTimeoutW(
            hwnd: Hwnd,
            message: u32,
            wparam: Wparam,
            lparam: Lparam,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> Lresult;
        pub fn SetParent(child: Hwnd, new_parent: Hwnd) -> Hwnd;
        pub fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
        pub fn SetWindowPos(
            hwnd: Hwnd,
            insert_after: Hwnd,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> Bool;
        pub fn SetWinEventHook(
            event_min: Dword,
            event_max: Dword,
            module: isize,
            callback: WinEventProc,
            process_id: Dword,
            thread_id: Dword,
            flags: Dword,
        ) -> HwinEventHook;
        pub fn UnhookWinEvent(hook: HwinEventHook) -> Bool;
        pub fn GetMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> Bool;
        pub fn TranslateMessage(message: *const Msg) -> Bool;
        pub fn DispatchMessageW(message: *const Msg) -> Lresult;
        pub fn SetTimer(
            hwnd: Hwnd,
            id: usize,
            milliseconds: u32,
            callback: Option<unsafe extern "system" fn(Hwnd, u32, usize, Dword)>,
        ) -> usize;
        pub fn KillTimer(hwnd: Hwnd, id: usize) -> Bool;
        pub fn WindowFromPoint(point: Point) -> Hwnd;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLastError() -> Dword;
        pub fn SetLastError(error: Dword);
    }
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn hwnd_value(window: &WebviewWindow) -> Result<win32::Hwnd, String> {
    window
        .hwnd()
        .map(|hwnd| hwnd.0 as win32::Hwnd)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn sync_webview_input(window: &WebviewWindow, move_focus: bool) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;

    window
        // SAFETY: Tauri owns the WebView2 controller for the duration of this
        // callback. We only invoke COM methods on that live controller and do
        // not retain its pointer after the callback returns.
        .with_webview(move |webview| unsafe {
            let controller = webview.controller();
            let _ = controller.NotifyParentWindowPositionChanged();
            if move_focus {
                let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn class_name(hwnd: win32::Hwnd) -> String {
    if hwnd == 0 {
        return "none".into();
    }

    let mut buffer = [0_u16; 128];
    // SAFETY: the HWND is treated as an opaque value and `buffer` is writable
    // for the exact capacity passed to GetClassNameW.
    let length = unsafe {
        win32::GetClassNameW(
            hwnd,
            buffer.as_mut_ptr(),
            buffer.len().try_into().unwrap_or(128),
        )
    };
    if length <= 0 {
        "unknown".into()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

#[cfg(target_os = "windows")]
fn window_title(hwnd: win32::Hwnd) -> String {
    if hwnd == 0 {
        return "none".into();
    }
    // SAFETY: querying text length does not borrow user memory; an invalid
    // HWND is reported by Win32 as length zero.
    let length = unsafe { win32::GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return "(untitled)".into();
    }
    let mut buffer = vec![0_u16; length as usize + 1];
    // SAFETY: the UTF-16 vector is writable and its allocated length matches
    // the maximum count supplied to GetWindowTextW.
    let copied = unsafe {
        win32::GetWindowTextW(
            hwnd,
            buffer.as_mut_ptr(),
            buffer.len().try_into().unwrap_or(i32::MAX),
        )
    };
    if copied <= 0 {
        "(untitled)".into()
    } else {
        String::from_utf16_lossy(&buffer[..copied as usize])
            .replace(['\r', '\n'], " ")
            .chars()
            .take(96)
            .collect()
    }
}

#[cfg(target_os = "windows")]
fn format_hwnd(hwnd: win32::Hwnd) -> String {
    format!("0x{hwnd:X}")
}

#[cfg(target_os = "windows")]
fn window_summary(hwnd: win32::Hwnd) -> String {
    if hwnd == 0 {
        return "0x0/none".into();
    }
    // SAFETY: both calls are read-only HWND queries; `hwnd` is checked for
    // null above and Win32 safely returns false for a stale handle.
    let (visible, iconic) = unsafe {
        (
            win32::IsWindowVisible(hwnd) != 0,
            win32::IsIconic(hwnd) != 0,
        )
    };
    format!(
        "{}/{}/\"{}\"/visible={}/iconic={}",
        format_hwnd(hwnd),
        class_name(hwnd),
        window_title(hwnd),
        visible,
        iconic
    )
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn collect_worker_w(
    top_level: win32::Hwnd,
    lparam: win32::Lparam,
) -> win32::Bool {
    if class_name(top_level) == "WorkerW" {
        // SAFETY: EnumWindows receives a pointer to `workers`, which remains
        // alive and exclusively borrowed until the synchronous enumeration
        // returns. The callback never stores the pointer.
        let workers = &mut *(lparam as *mut Vec<win32::Hwnd>);
        workers.push(top_level);
    }
    1
}

#[cfg(target_os = "windows")]
fn worker_w_windows() -> Vec<win32::Hwnd> {
    // Diagnostics only. WorkerW is never a Desktop host candidate: it is reached
    // only through the raised-desktop transition, and a non-interactive WorkerW
    // is rejected outright by `find_interactive_desktop_parent`.
    let mut workers = Vec::new();
    // SAFETY: the callback contract and lifetime of the lparam pointer are
    // documented in `collect_worker_w`; EnumWindows is synchronous here.
    unsafe {
        win32::EnumWindows(
            Some(collect_worker_w),
            (&mut workers as *mut Vec<win32::Hwnd>) as win32::Lparam,
        );
    }
    workers
}

#[cfg(target_os = "windows")]
fn capture_widget_invariant(hwnd: win32::Hwnd) -> WidgetInvariant {
    let mut rect = win32::Rect::default();
    if hwnd != 0 {
        // SAFETY: `rect` is a valid writable RECT and the opaque HWND is only
        // queried; failure leaves the zero-initialized rectangle intact.
        unsafe { win32::GetWindowRect(hwnd, &mut rect) };
    }
    let has_size = rect.right > rect.left && rect.bottom > rect.top;
    // The centre point is the hit-test sample because a Desktop widget's failure
    // mode is "another window sits over the middle of it": that is the exact
    // shape of the inert-visible-widget regression, and it is invisible to a
    // style-only check. See docs/phase-7c3b3-product-interaction-regression.md.
    let hit_target = if hwnd != 0 && has_size {
        // SAFETY: WindowFromPoint takes the point by value and returns an
        // opaque HWND; the midpoint is derived from a successfully sized RECT.
        unsafe {
            win32::WindowFromPoint(win32::Point {
                x: rect.left + (rect.right - rect.left) / 2,
                y: rect.top + (rect.bottom - rect.top) / 2,
            })
        }
    } else {
        0
    };
    let (parent, style, ex_style, visible, iconic, enabled, z_prev, z_next) = if hwnd != 0 {
        // SAFETY: this block performs read-only queries against one opaque
        // non-null HWND. Win32 returns zero for stale handles; no returned
        // pointer is retained.
        unsafe {
            (
                win32::GetParent(hwnd),
                win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE),
                win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE),
                win32::IsWindowVisible(hwnd) != 0,
                win32::IsIconic(hwnd) != 0,
                win32::IsWindowEnabled(hwnd) != 0,
                win32::GetWindow(hwnd, win32::GW_HWNDPREV),
                win32::GetWindow(hwnd, win32::GW_HWNDNEXT),
            )
        }
    } else {
        (0, 0, 0, false, false, false, 0, 0)
    };
    // SAFETY: IsChild only compares two opaque HWND values returned by Win32.
    let hit_owned =
        hwnd != 0 && (hit_target == hwnd || unsafe { win32::IsChild(hwnd, hit_target) } != 0);
    WidgetInvariant {
        hwnd,
        parent: if hwnd != 0 { parent } else { 0 },
        style: if hwnd != 0 { style } else { 0 },
        ex_style: if hwnd != 0 { ex_style } else { 0 },
        rect: (rect.left, rect.top, rect.right, rect.bottom),
        visible: hwnd != 0 && visible,
        iconic: hwnd != 0 && iconic,
        enabled: hwnd != 0 && enabled,
        z_prev: if hwnd != 0 { z_prev } else { 0 },
        z_next: if hwnd != 0 { z_next } else { 0 },
        hit_target,
        hit_owned,
    }
}

#[cfg(target_os = "windows")]
fn widget_invariant_issues(
    baseline: WidgetInvariant,
    current: WidgetInvariant,
) -> Vec<&'static str> {
    let mut issues = Vec::new();
    if current.hwnd != baseline.hwnd {
        issues.push("hwnd-changed");
    }
    if current.parent != baseline.parent {
        issues.push("parent-changed");
    }
    if current.style != baseline.style {
        issues.push("style-changed");
    }
    if current.ex_style != baseline.ex_style {
        issues.push("ex-style-changed");
    }
    if current.rect != baseline.rect {
        issues.push("bounds-changed");
    }
    if current.z_prev != baseline.z_prev || current.z_next != baseline.z_next {
        issues.push("z-order-changed");
    }
    if !current.visible {
        issues.push("widget-hidden");
    }
    if current.iconic {
        issues.push("widget-iconic");
    }
    if !current.enabled {
        issues.push("widget-disabled");
    }
    if !current.hit_owned {
        issues.push("hit-route-not-owned");
    }
    issues
}

#[cfg(target_os = "windows")]
fn desktop_topology_snapshot(label: &str) -> String {
    // SAFETY: these are read-only Shell window lookups. The temporary UTF-16
    // class buffer remains alive for the duration of FindWindowW.
    let (foreground, progman) = unsafe {
        (
            win32::GetForegroundWindow(),
            win32::FindWindowW(wide("Progman").as_ptr(), std::ptr::null()),
        )
    };
    let def_view = locate_interactive_desktop_parent()
        .map(|(hwnd, _)| hwnd)
        .unwrap_or(0);
    let desktop_host = if def_view != 0 {
        // SAFETY: `def_view` was just discovered and validated by the Shell
        // lookup; GetParent returns another opaque handle.
        unsafe { win32::GetParent(def_view) }
    } else {
        0
    };
    let widget = DESKTOP_HWND.load(Ordering::Acquire);
    let invariant = capture_widget_invariant(widget);
    let workers = worker_w_windows()
        .into_iter()
        .map(window_summary)
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "{label} · foreground={} · Progman={} · DefView={} · WorkerW=[{}] · widget={}/parent={}/style=0x{:X}/ex=0x{:X}/bounds={},{} {}x{}/visible={}/iconic={}/enabled={}/hit-owned={} · z-prev={} · z-next={} · desktop-host={}",
        window_summary(foreground),
        window_summary(progman),
        window_summary(def_view),
        workers,
        format_hwnd(widget),
        window_summary(invariant.parent),
        invariant.style,
        invariant.ex_style,
        invariant.rect.0,
        invariant.rect.1,
        (invariant.rect.2 - invariant.rect.0).max(0),
        (invariant.rect.3 - invariant.rect.1).max(0),
        invariant.visible,
        invariant.iconic,
        invariant.enabled,
        invariant.hit_owned,
        window_summary(invariant.z_prev),
        window_summary(invariant.z_next),
        window_summary(desktop_host),
    )
}

#[cfg(target_os = "windows")]
fn record_win_d_sample(generation: u64, label: &str) -> bool {
    let hwnd = DESKTOP_HWND.load(Ordering::Acquire);
    let invariant = capture_widget_invariant(hwnd);
    let sample = format!("{} · {}", timestamp(), desktop_topology_snapshot(label));
    let Ok(mut state) = desktop_lifecycle().lock() else {
        return false;
    };
    if state.trace_generation != generation || !state.trace_active {
        return false;
    }
    if state.trace_mode == TraceMode::ObserverControl {
        if let Some(baseline) = state.observer_baseline {
            for issue in widget_invariant_issues(baseline, invariant) {
                if !state
                    .observer_issues
                    .iter()
                    .any(|existing| existing == issue)
                {
                    state.observer_issues.push(issue.into());
                }
            }
            if !state.observer_issues.is_empty() {
                state.observer_status = "running · invariant change detected".into();
            }
        }
    }
    state.trace_samples.push_front(sample);
    state.trace_samples.truncate(24);
    true
}

#[cfg(target_os = "windows")]
fn finalize_read_only_trace(generation: u64) {
    let outcome = desktop_lifecycle().lock().ok().and_then(|mut state| {
        if state.trace_generation != generation {
            return None;
        }
        let mode = state.trace_mode;
        if mode == TraceMode::ObserverControl {
            if state.recovery_mutation_count != state.observer_recovery_mutation_baseline
                && !state
                    .observer_issues
                    .iter()
                    .any(|issue| issue == "recovery-mutated-window")
            {
                state.observer_issues.push("recovery-mutated-window".into());
            }
            state.observer_passed = state.observer_issues.is_empty();
            state.observer_status = if state.observer_passed {
                "passed · stable for 10 seconds · Win+D trace unlocked".into()
            } else {
                format!("failed · {}", state.observer_issues.join("+"))
            };
        }
        state.trace_active = false;
        Some((mode, state.observer_passed, state.observer_status.clone()))
    });

    if let Some((mode, passed, status)) = outcome {
        match mode {
            TraceMode::ObserverControl => record_lifecycle_event(format!(
                "observer control complete · passed={passed} · {status}"
            )),
            TraceMode::WinD => record_lifecycle_event("Win+D read-only trace complete"),
            TraceMode::Idle => record_lifecycle_event("read-only trace complete"),
        }
    }
}

#[cfg(target_os = "windows")]
fn format_rect(rect: win32::Rect) -> String {
    format!(
        "{},{} · {}x{}",
        rect.left,
        rect.top,
        (rect.right - rect.left).max(0),
        (rect.bottom - rect.top).max(0)
    )
}

#[cfg(target_os = "windows")]
fn capture_detach_after(hwnd: win32::Hwnd, diagnostics: &mut DetachDiagnostics) {
    let mut rect = win32::Rect::default();
    // SAFETY: the mutable RECT is valid for the call and all other operations
    // are read-only queries on the transition's live HWND.
    unsafe {
        win32::GetWindowRect(hwnd, &mut rect);
        diagnostics.style_after =
            format!("0x{:X}", win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE));
        diagnostics.ex_style_after =
            format!("0x{:X}", win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE));
        diagnostics.parent_after = format_hwnd(win32::GetParent(hwnd));
        diagnostics.visible_after = win32::IsWindowVisible(hwnd) != 0;
    }
    diagnostics.bounds_after = format_rect(rect);
}

#[cfg(target_os = "windows")]
fn windows_diagnostics(window: &WebviewWindow) -> Result<NativeDiagnostics, String> {
    let hwnd = hwnd_value(window)?;
    let probe = read_attachment_probe(hwnd);
    update_lifecycle_probe(probe);
    // SAFETY: the probe contains opaque HWND values only. These calls read
    // style/thread state and do not retain pointers.
    let (parent_style, parent_ex_style, window_thread) = unsafe {
        (
            win32::GetWindowLongPtrW(probe.parent, win32::GWL_STYLE),
            win32::GetWindowLongPtrW(probe.parent, win32::GWL_EXSTYLE),
            win32::GetWindowThreadProcessId(hwnd, std::ptr::null_mut()),
        )
    };
    let mut gui = win32::GuiThreadInfo {
        size: std::mem::size_of::<win32::GuiThreadInfo>() as win32::Dword,
        ..Default::default()
    };
    // SAFETY: `gui.size` is initialized as required and `gui` remains a valid
    // writable buffer for the synchronous call.
    let focus = if unsafe { win32::GetGUIThreadInfo(window_thread, &mut gui) } != 0 {
        gui.focus
    } else {
        0
    };
    // SAFETY: both operations are read-only queries of opaque HWND state.
    let (foreground, window_enabled) = unsafe {
        (
            win32::GetForegroundWindow(),
            win32::IsWindowEnabled(hwnd) != 0,
        )
    };
    let input_route = if !window_enabled {
        "blocked-host-disabled".into()
    } else if probe.hit_target == 0 {
        "blocked-no-hit-target".into()
    } else if probe.hit_owned {
        format!("webview-owned · {}", class_name(probe.hit_target))
    } else {
        format!("blocked-by · {}", class_name(probe.hit_target))
    };
    let lifecycle = lifecycle_snapshot();
    let attachment_valid = if lifecycle.requested {
        probe.attachment_valid()
    } else {
        true
    };

    // SAFETY: read-only queries of the live product HWND. The child test uses the
    // style bit because `GetParent` reports the owner for a top-level window.
    let (owner, is_child) = unsafe {
        (
            win32::GetWindow(hwnd, win32::GW_OWNER),
            probe.style & win32::WS_CHILD != 0,
        )
    };
    // One rule, one implementation: the widget contract's own taskbar predicate.
    let taskbar_eligible = crate::platform::windows::widget_frame::taskbar_eligible_from(
        probe.ex_style,
        owner != 0,
        is_child,
    );
    let alt_tab_eligible = crate::platform::windows::widget_frame::alt_tab_eligible_from(
        probe.ex_style,
        owner != 0,
        is_child,
    );
    let tool_window = probe.ex_style & win32::WS_EX_TOOLWINDOW != 0;
    let app_window_cleared = probe.ex_style & win32::WS_EX_APPWINDOW == 0;

    Ok(NativeDiagnostics {
        hwnd: format_hwnd(hwnd),
        parent_hwnd: format_hwnd(probe.parent),
        parent_class: class_name(probe.parent),
        style: format!("0x{:X}", probe.style),
        ex_style: format!("0x{:X}", probe.ex_style),
        parent_style: format!("0x{parent_style:X}"),
        parent_ex_style: format!("0x{parent_ex_style:X}"),
        hit_target_hwnd: format_hwnd(probe.hit_target),
        hit_target_class: class_name(probe.hit_target),
        hit_target_owned: probe.hit_owned,
        focus_hwnd: format_hwnd(focus),
        focus_class: class_name(focus),
        foreground_hwnd: format_hwnd(foreground),
        input_route,
        hwnd_valid: probe.hwnd_valid,
        current_desktop_hwnd: format_hwnd(probe.current_def_view),
        is_window_visible: probe.visible,
        attachment_valid,
        recovery_count: lifecycle.recovery_count,
        recovery_reason: lifecycle.recovery_reason,
        widget_frame: WidgetFrameFacts {
            tool_window,
            app_window_cleared,
            taskbar_eligible,
            alt_tab_eligible,
            mode: WindowMode::default(),
        },
        x: probe.rect.left,
        y: probe.rect.top,
        width: (probe.rect.right - probe.rect.left).max(0) as u32,
        height: (probe.rect.bottom - probe.rect.top).max(0) as u32,
    })
}

#[cfg(target_os = "windows")]
struct DesktopHostSearch {
    def_view: win32::Hwnd,
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn find_desktop_host(
    top_level: win32::Hwnd,
    lparam: win32::Lparam,
) -> win32::Bool {
    let shell_view = wide("SHELLDLL_DefView");
    // SAFETY: EnumWindows supplies a valid top-level HWND. `shell_view` is a
    // live null-terminated buffer for the synchronous lookup.
    let def_view = win32::FindWindowExW(top_level, 0, shell_view.as_ptr(), std::ptr::null());
    if def_view != 0 {
        // SAFETY: lparam points to the caller-owned DesktopHostSearch for the
        // full synchronous EnumWindows call and is not retained afterward.
        let search = &mut *(lparam as *mut DesktopHostSearch);
        search.def_view = def_view;
        return 0;
    }
    1
}

#[cfg(target_os = "windows")]
fn locate_interactive_desktop_parent() -> Option<(win32::Hwnd, String)> {
    let progman_class = wide("Progman");
    let shell_view = wide("SHELLDLL_DefView");
    // SAFETY: both UTF-16 class buffers are null-terminated and remain alive;
    // the calls return opaque handles and retain no pointers.
    let progman = unsafe { win32::FindWindowW(progman_class.as_ptr(), std::ptr::null()) };
    if progman == 0 {
        return None;
    }

    // SAFETY: `progman` is non-null and `shell_view` remains alive through the
    // synchronous child lookup.
    let progman_def_view =
        unsafe { win32::FindWindowExW(progman, 0, shell_view.as_ptr(), std::ptr::null()) };
    // SAFETY: IsWindow validates only the opaque handle returned above.
    if progman_def_view != 0 && unsafe { win32::IsWindow(progman_def_view) } != 0 {
        return Some((progman_def_view, "shelldll-defview-child-of-progman".into()));
    }

    let mut search = DesktopHostSearch { def_view: 0 };
    // SAFETY: EnumWindows is synchronous, so `search` outlives the callback
    // and remains exclusively borrowed until enumeration returns.
    unsafe {
        win32::EnumWindows(
            Some(find_desktop_host),
            (&mut search as *mut DesktopHostSearch) as win32::Lparam,
        );
    }
    // SAFETY: the callback supplied this opaque handle; IsWindow validates it
    // before the subsequent parent query.
    if search.def_view != 0 && unsafe { win32::IsWindow(search.def_view) } != 0 {
        // SAFETY: `search.def_view` was validated immediately above.
        let host = unsafe { win32::GetParent(search.def_view) };
        return Some((
            search.def_view,
            format!("shelldll-defview-child-of-{}", class_name(host)),
        ));
    }

    None
}

#[cfg(target_os = "windows")]
fn find_interactive_desktop_parent() -> Result<(win32::Hwnd, String), String> {
    // Desktop hosting has to be a child of the *interactive* desktop view.
    // Substituting WorkerW, or inventing an owner window, produces a surface that
    // looks right but does not survive Explorer restarts, Win+D, or a wallpaper
    // change — so a missing SHELLDLL_DefView is a hard error, not a reason to
    // guess. See docs/desktop-mode.md for the attach/detach lifecycle.
    if let Some(parent) = locate_interactive_desktop_parent() {
        return Ok(parent);
    }

    let progman_class = wide("Progman");
    // SAFETY: the null-terminated UTF-16 class buffer remains valid for the
    // synchronous lookup and no pointer is retained.
    let progman = unsafe { win32::FindWindowW(progman_class.as_ptr(), std::ptr::null()) };
    if progman == 0 {
        return Err("Windows shell Progman window was not found".into());
    }

    let mut message_result = 0_usize;
    // SAFETY: Progman was found above; the result pointer is writable for the
    // duration of SendMessageTimeoutW. SMTO_ABORTIFHUNG bounds Shell stalls.
    unsafe {
        // Undocumented Shell message. 0xD/0x1 is required by the raised desktop
        // used on current Windows 11 builds; classic shells ignore it safely.
        // It is only a nudge to materialise the desktop host — this function
        // still fails rather than accepting a non-interactive WorkerW.
        win32::SendMessageTimeoutW(
            progman,
            win32::PROGMAN_SPAWN_WORKERW,
            0xD,
            0x1,
            win32::SMTO_ABORTIFHUNG,
            1_000,
            &mut message_result,
        );
    }

    if let Some(parent) = locate_interactive_desktop_parent() {
        return Ok(parent);
    }

    Err("Windows desktop SHELLDLL_DefView host was not found; refusing a non-interactive WorkerW fallback".into())
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct AttachmentProbe {
    hwnd_valid: bool,
    visible: bool,
    parent: win32::Hwnd,
    current_def_view: win32::Hwnd,
    rect: win32::Rect,
    style: isize,
    ex_style: isize,
    hit_target: win32::Hwnd,
    hit_owned: bool,
    desktop_foreground: bool,
    bounds_valid: bool,
}

#[cfg(target_os = "windows")]
impl AttachmentProbe {
    fn attachment_valid(self) -> bool {
        self.hwnd_valid
            && self.current_def_view != 0
            && self.parent == self.current_def_view
            && self.bounds_valid
            && self.style & win32::WS_CHILD != 0
            && self.style & win32::WS_POPUP == 0
            && self.ex_style & (win32::WS_EX_TRANSPARENT | win32::WS_EX_NOACTIVATE) == 0
            && (!self.desktop_foreground || self.visible)
            && (!self.desktop_foreground || self.hit_owned)
    }

    fn issues(self) -> Vec<&'static str> {
        let mut issues = Vec::new();
        if !self.hwnd_valid {
            issues.push("hwnd-invalid");
        }
        if self.current_def_view == 0 {
            issues.push("defview-missing");
        } else if self.parent != self.current_def_view {
            issues.push("parent-changed");
        }
        if self.desktop_foreground && !self.visible {
            issues.push("window-hidden");
        }
        if !self.bounds_valid {
            issues.push("bounds-invalid");
        }
        if self.style & win32::WS_CHILD == 0 || self.style & win32::WS_POPUP != 0 {
            issues.push("style-invalid");
        }
        if self.ex_style & (win32::WS_EX_TRANSPARENT | win32::WS_EX_NOACTIVATE) != 0 {
            issues.push("ex-style-input-blocked");
        }
        if self.desktop_foreground && !self.hit_owned {
            issues.push("desktop-z-order-invalid");
        }
        issues
    }
}

#[cfg(target_os = "windows")]
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "SHELLDLL_DefView" | "SysListView32"
    )
}

#[cfg(target_os = "windows")]
fn is_desktop_foreground() -> bool {
    // SAFETY: both functions return process-external opaque HWND values and
    // do not dereference caller memory.
    let (foreground, shell_window) =
        unsafe { (win32::GetForegroundWindow(), win32::GetShellWindow()) };
    let app_hwnd = DESKTOP_HWND.load(Ordering::Acquire);
    let def_view = CURRENT_DEF_VIEW.load(Ordering::Acquire);
    // SAFETY: IsChild compares two opaque handles; stale handles simply yield
    // false, and no pointer or lifetime crosses the call.
    let foreground_under_def_view =
        def_view != 0 && unsafe { win32::IsChild(def_view, foreground) } != 0;
    foreground != 0
        && (foreground == shell_window
            || foreground == app_hwnd
            || foreground == def_view
            || foreground_under_def_view
            || is_shell_class(&class_name(foreground)))
}

#[cfg(target_os = "windows")]
fn read_attachment_probe(hwnd: win32::Hwnd) -> AttachmentProbe {
    // SAFETY: IsWindow is a read-only validity probe for an opaque handle.
    let hwnd_valid = hwnd != 0 && unsafe { win32::IsWindow(hwnd) } != 0;
    let parent = if hwnd_valid {
        // SAFETY: the HWND was validated immediately above.
        unsafe { win32::GetParent(hwnd) }
    } else {
        0
    };
    let current_def_view = locate_interactive_desktop_parent()
        .map(|(handle, _)| handle)
        .unwrap_or(0);
    CURRENT_DEF_VIEW.store(current_def_view, Ordering::Release);

    let style = if hwnd_valid {
        // SAFETY: the HWND was validated above; this only reads style bits.
        unsafe { win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE) }
    } else {
        0
    };
    let ex_style = if hwnd_valid {
        // SAFETY: the HWND was validated above; this only reads style bits.
        unsafe { win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE) }
    } else {
        0
    };
    // SAFETY: IsWindowVisible is a read-only query on the validated HWND.
    let visible = hwnd_valid && unsafe { win32::IsWindowVisible(hwnd) } != 0;

    let mut rect = win32::Rect::default();
    if hwnd_valid {
        // SAFETY: `rect` is a valid writable RECT and the HWND was validated.
        unsafe { win32::GetWindowRect(hwnd, &mut rect) };
    }
    let mut parent_rect = win32::Rect::default();
    if current_def_view != 0 {
        // SAFETY: `parent_rect` is writable and the Shell lookup supplied the
        // non-null DefView handle; failure leaves the default RECT unchanged.
        unsafe { win32::GetWindowRect(current_def_view, &mut parent_rect) };
    }
    let has_size = rect.right > rect.left && rect.bottom > rect.top;
    let intersects_parent = current_def_view != 0
        && rect.right > parent_rect.left
        && rect.left < parent_rect.right
        && rect.bottom > parent_rect.top
        && rect.top < parent_rect.bottom;
    let bounds_valid = has_size && intersects_parent;

    let hit_target = if has_size {
        // SAFETY: WindowFromPoint takes a value and returns an opaque handle;
        // the point is the center of the measured widget rectangle.
        unsafe {
            win32::WindowFromPoint(win32::Point {
                x: rect.left + (rect.right - rect.left) / 2,
                y: rect.top + (rect.bottom - rect.top) / 2,
            })
        }
    } else {
        0
    };
    // SAFETY: IsChild compares opaque handles and retains no state.
    let hit_owned =
        hwnd_valid && (hit_target == hwnd || unsafe { win32::IsChild(hwnd, hit_target) } != 0);

    AttachmentProbe {
        hwnd_valid,
        visible,
        parent,
        current_def_view,
        rect,
        style,
        ex_style,
        hit_target,
        hit_owned,
        desktop_foreground: is_desktop_foreground(),
        bounds_valid,
    }
}

#[cfg(target_os = "windows")]
fn update_lifecycle_probe(probe: AttachmentProbe) {
    if let Ok(mut state) = desktop_lifecycle().lock() {
        state.current_def_view = probe.current_def_view;
        state.attachment_valid = if state.requested {
            probe.attachment_valid()
        } else {
            true
        };
        if probe.bounds_valid {
            state.last_bounds = Some((
                probe.rect.left,
                probe.rect.top,
                probe.rect.right,
                probe.rect.bottom,
            ));
        }
    }
}

#[cfg(target_os = "windows")]
fn probe_summary(hwnd: win32::Hwnd, probe: AttachmentProbe) -> String {
    format!(
        "hwnd={} · parent={} · defview={} · visible={} · bounds={},{} {}x{} · valid={}",
        format_hwnd(hwnd),
        format_hwnd(probe.parent),
        format_hwnd(probe.current_def_view),
        probe.visible,
        probe.rect.left,
        probe.rect.top,
        (probe.rect.right - probe.rect.left).max(0),
        (probe.rect.bottom - probe.rect.top).max(0),
        probe.attachment_valid()
    )
}

#[cfg(target_os = "windows")]
fn event_name(event: u32) -> &'static str {
    match event {
        win32::EVENT_SYSTEM_FOREGROUND => "foreground",
        win32::EVENT_OBJECT_CREATE => "shell-create",
        win32::EVENT_OBJECT_DESTROY => "shell-destroy",
        win32::EVENT_OBJECT_SHOW => "shell-show",
        win32::EVENT_OBJECT_HIDE => "shell-hide",
        win32::EVENT_OBJECT_REORDER => "shell-reorder",
        win32::EVENT_OBJECT_PARENTCHANGE => "shell-parent-change",
        _ => "shell-event",
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn desktop_win_event(
    _hook: win32::HwinEventHook,
    event: win32::Dword,
    hwnd: win32::Hwnd,
    object_id: i32,
    _child_id: i32,
    _event_thread: win32::Dword,
    _event_time: win32::Dword,
) {
    // SAFETY: Win32 invokes this callback with opaque handle values. The
    // callback never dereferences caller pointers, does not retain handles as
    // borrowed memory, and only schedules recovery through shared state.
    if !DESKTOP_REQUESTED.load(Ordering::Acquire) {
        return;
    }

    let app_hwnd = DESKTOP_HWND.load(Ordering::Acquire);
    let current_def_view = CURRENT_DEF_VIEW.load(Ordering::Acquire);
    let class = class_name(hwnd);
    let event_description = format!(
        "{} · hwnd={} · class={} · object={} · child={} · eventThread={} · eventTime={}",
        event_name(event),
        format_hwnd(hwnd),
        class,
        object_id,
        _child_id,
        _event_thread,
        _event_time
    );
    let (event_count, trace_generation, trace_active) = desktop_lifecycle()
        .lock()
        .map(|mut state| {
            state.win_event_count = state.win_event_count.saturating_add(1);
            state.last_win_event = event_description.clone();
            (
                state.win_event_count,
                state.trace_generation,
                state.trace_active,
            )
        })
        .unwrap_or((0, 0, false));
    if trace_active {
        let _ = record_win_d_sample(
            trace_generation,
            &format!("WinEvent#{event_count}-{}", event_name(event)),
        );
    }
    let is_foreground = event == win32::EVENT_SYSTEM_FOREGROUND;
    let is_object_event = matches!(
        event,
        win32::EVENT_OBJECT_CREATE
            | win32::EVENT_OBJECT_DESTROY
            | win32::EVENT_OBJECT_SHOW
            | win32::EVENT_OBJECT_HIDE
            | win32::EVENT_OBJECT_REORDER
            | win32::EVENT_OBJECT_PARENTCHANGE
    );
    let relevant = is_foreground
        || (is_object_event
            && object_id == win32::OBJID_WINDOW
            && (hwnd == app_hwnd
                || hwnd == current_def_view
                || is_shell_class(&class)
                || (app_hwnd != 0 && win32::GetParent(app_hwnd) == hwnd)));

    if !relevant {
        return;
    }

    record_lifecycle_event(format!("WinEvent #{event_count} · {event_description}"));
    if let Ok(mut state) = desktop_lifecycle().lock() {
        state.pending_reason = format!("{} · {}", event_name(event), class);
    }
    win32::KillTimer(0, win32::LIFECYCLE_TIMER_ID);
    win32::SetTimer(
        0,
        win32::LIFECYCLE_TIMER_ID,
        win32::LIFECYCLE_DEBOUNCE_MS,
        None,
    );
}

#[cfg(target_os = "windows")]
fn ensure_desktop_lifecycle_monitor(window: WebviewWindow) {
    // Only ever started once, by the first successful Desktop attach. Recovery
    // runs on this thread's own message loop, so the timers below are
    // thread-scoped (hwnd 0) and the hook callbacks stay off the UI thread.
    if DESKTOP_MONITOR_STARTED.set(()).is_err() {
        return;
    }

    // SAFETY: the watcher thread owns its hook handles and message buffer.
    // Callbacks are static functions, hooks are unregistered on loop exit, and
    // no stack pointer is shared with Win32 beyond a synchronous call.
    std::thread::spawn(move || unsafe {
        let foreground_hook = win32::SetWinEventHook(
            win32::EVENT_SYSTEM_FOREGROUND,
            win32::EVENT_SYSTEM_FOREGROUND,
            0,
            Some(desktop_win_event),
            0,
            0,
            win32::WINEVENT_OUTOFCONTEXT,
        );
        let object_hook = win32::SetWinEventHook(
            win32::EVENT_OBJECT_CREATE,
            win32::EVENT_OBJECT_PARENTCHANGE,
            0,
            Some(desktop_win_event),
            0,
            0,
            win32::WINEVENT_OUTOFCONTEXT,
        );

        if let Ok(mut state) = desktop_lifecycle().lock() {
            state.foreground_hook_installed = foreground_hook != 0;
            state.object_hook_installed = object_hook != 0;
            state.watcher_running = foreground_hook != 0 || object_hook != 0;
        }

        if foreground_hook == 0 && object_hook == 0 {
            record_lifecycle_event("lifecycle monitor failed to install WinEvent hooks");
            return;
        }
        record_lifecycle_event("lifecycle monitor installed · event-driven · debounce 220ms");

        if let Ok(mut state) = desktop_lifecycle().lock() {
            state.pending_reason = "monitor-start".into();
        }
        win32::SetTimer(0, win32::LIFECYCLE_TIMER_ID, 40, None);

        let mut message = win32::Msg::default();
        while win32::GetMessageW(&mut message, 0, 0, 0) > 0 {
            if message.message == win32::WM_TIMER && message.wparam == win32::LIFECYCLE_TIMER_ID {
                win32::KillTimer(0, win32::LIFECYCLE_TIMER_ID);
                let reason = desktop_lifecycle()
                    .lock()
                    .map(|mut state| std::mem::take(&mut state.pending_reason))
                    .unwrap_or_else(|_| "lifecycle-state-error".into());
                revalidate_desktop_attachment(&window, &reason);
                continue;
            }
            win32::TranslateMessage(&message);
            win32::DispatchMessageW(&message);
        }

        if foreground_hook != 0 {
            win32::UnhookWinEvent(foreground_hook);
        }
        if object_hook != 0 {
            win32::UnhookWinEvent(object_hook);
        }
        if let Ok(mut state) = desktop_lifecycle().lock() {
            state.watcher_running = false;
            state.foreground_hook_installed = false;
            state.object_hook_installed = false;
        }
    });
}

#[cfg(target_os = "windows")]
fn revalidate_desktop_attachment(window: &WebviewWindow, trigger: &str) {
    // Deliberately expensive and conservative. Every check below re-reads live
    // window state instead of trusting a cached flag, because the events that
    // reach here (Explorer restart, Win+D, desktop composition change, z-order
    // degradation) each leave the shell in a *different* intermediate topology.
    // A cheap "is the parent still right" test misses the cases where parent is
    // unchanged but hit-testing or input has been lost.
    let snapshot = lifecycle_snapshot();
    if !snapshot.requested || !DESKTOP_REQUESTED.load(Ordering::Acquire) {
        return;
    }

    let Ok(hwnd) = hwnd_value(window) else {
        record_lifecycle_event(format!("attachment invalid · {trigger} · hwnd unavailable"));
        return;
    };
    let initial = read_attachment_probe(hwnd);
    update_lifecycle_probe(initial);
    let issues = initial.issues();
    if issues.is_empty() {
        record_lifecycle_event(format!("attachment valid · {trigger}"));
        return;
    }
    record_lifecycle_event(format!(
        "attachment invalid · {} · trigger {trigger} · {}",
        issues.join("+"),
        probe_summary(hwnd, initial)
    ));
    if observer_control_active() {
        record_lifecycle_event(format!(
            "observer control · recovery write suppressed · trigger {trigger}"
        ));
        return;
    }

    // A Win+D recovery and an explicit mode transition must never mutate the
    // HWND concurrently. A recovery that passed an earlier generation check
    // could otherwise reparent the window immediately after SetParent(NULL).
    let Ok(_transition) = desktop_transition().lock() else {
        record_lifecycle_event(format!(
            "recovery deferred · {trigger} · transition lock poisoned"
        ));
        return;
    };

    let snapshot = lifecycle_snapshot();
    if !snapshot.requested || !DESKTOP_REQUESTED.load(Ordering::Acquire) {
        record_lifecycle_event(format!("recovery cancelled · mode changed · {trigger}"));
        return;
    }

    // Re-probe after taking the transition lock: the shell or a manual mode
    // change may have stabilized the hierarchy while recovery was waiting.
    // Without this second probe a recovery can "repair" an attachment that
    // Explorer has already restored, which is what produced a visible flash and
    // an unnecessary SetParent during the Explorer-restart round.
    let initial = read_attachment_probe(hwnd);
    update_lifecycle_probe(initial);
    let issues = initial.issues();
    if issues.is_empty() {
        record_lifecycle_event(format!("recovery skipped · attachment valid · {trigger}"));
        return;
    }
    if observer_control_active() {
        record_lifecycle_event(format!(
            "observer control · queued recovery write suppressed · trigger {trigger}"
        ));
        return;
    }

    if !DESKTOP_REQUESTED.load(Ordering::Acquire)
        || lifecycle_snapshot().generation != snapshot.generation
    {
        record_lifecycle_event(format!("recovery cancelled · mode changed · {trigger}"));
        return;
    }

    let desktop_parent = if initial.current_def_view != 0 {
        initial.current_def_view
    } else {
        match find_interactive_desktop_parent() {
            Ok((parent, _)) => parent,
            Err(error) => {
                if let Ok(mut state) = desktop_lifecycle().lock() {
                    state.attachment_valid = false;
                    state.recovery_reason = format!("{} · {error}", issues.join("+"));
                }
                record_lifecycle_event(format!("recovery deferred · {trigger} · {error}"));
                return;
            }
        }
    };

    let bounds = if initial.bounds_valid {
        Some((
            initial.rect.left,
            initial.rect.top,
            initial.rect.right,
            initial.rect.bottom,
        ))
    } else {
        snapshot.last_bounds
    };
    let Some((left, top, right, bottom)) = bounds else {
        record_lifecycle_event(format!("recovery deferred · {trigger} · no valid bounds"));
        return;
    };

    if !DESKTOP_REQUESTED.load(Ordering::Acquire)
        || lifecycle_snapshot().generation != snapshot.generation
    {
        record_lifecycle_event(format!("recovery cancelled · mode changed · {trigger}"));
        return;
    }

    let mut parent_rect = win32::Rect::default();
    if observer_control_active() {
        record_lifecycle_event(format!(
            "observer control · late recovery write suppressed · trigger {trigger}"
        ));
        return;
    }
    if let Ok(mut state) = desktop_lifecycle().lock() {
        state.recovery_mutation_count = state.recovery_mutation_count.saturating_add(1);
    }
    // SAFETY: the transition mutex is held, the HWND and DefView were probed
    // after locking, bounds are valid, and all style/parent/position mutations
    // complete before another manual or recovery transition can proceed.
    // Do not collapse the generation re-checks above into one: a recovery that
    // passed an earlier check can otherwise reparent the window immediately after
    // a manual mode change has already detached it.
    let recovered = unsafe {
        win32::GetWindowRect(desktop_parent, &mut parent_rect);
        win32::SetWindowLongPtrW(
            hwnd,
            win32::GWL_STYLE,
            interactive_child_style(initial.style),
        );
        win32::SetWindowLongPtrW(
            hwnd,
            win32::GWL_EXSTYLE,
            interactive_child_ex_style(initial.ex_style),
        );
        if win32::GetParent(hwnd) != desktop_parent {
            win32::SetParent(hwnd, desktop_parent);
        }
        win32::EnableWindow(hwnd, 1);
        win32::SetWindowPos(
            hwnd,
            win32::HWND_TOP,
            left - parent_rect.left,
            top - parent_rect.top,
            (right - left).max(360),
            (bottom - top).max(500),
            win32::SWP_FRAMECHANGED | win32::SWP_SHOWWINDOW | win32::SWP_NOACTIVATE,
        ) != 0
    };

    if recovered {
        let _ = sync_webview_input(window, false);
    }
    let after = read_attachment_probe(hwnd);
    update_lifecycle_probe(after);
    let reason = format!("{} · trigger {trigger}", issues.join("+"));
    if recovered && after.attachment_valid() {
        let count = if let Ok(mut state) = desktop_lifecycle().lock() {
            state.recovery_count = state.recovery_count.saturating_add(1);
            state.recovery_reason = reason.clone();
            state.recovery_count
        } else {
            0
        };
        record_lifecycle_event(format!(
            "recovered #{count} · {reason} · {}",
            probe_summary(hwnd, after)
        ));
    } else {
        if let Ok(mut state) = desktop_lifecycle().lock() {
            state.attachment_valid = false;
            state.recovery_reason = format!("recovery failed · {reason}");
        }
        record_lifecycle_event(format!("recovery failed · {reason}"));
    }
}

#[cfg(target_os = "windows")]
fn detach_from_desktop(hwnd: win32::Hwnd, native: &mut StoredNativeState) -> Result<(), String> {
    let mut before_rect = win32::Rect::default();
    let attempt = native.last_detach.attempt.saturating_add(1);
    // SAFETY: the transition mutex is held by the caller and `hwnd` is the
    // live Tauri window.
    // Do not reorder the operations in this block. SetParent(NULL), restore
    // top-level styles, frame-change, then verify the final parent: reordering
    // recreates the Phase 1 detach failure, where the HWND keeps WS_CHILD and
    // stays unreachable behind every top-level window.
    unsafe {
        win32::GetWindowRect(hwnd, &mut before_rect);
        let parent_before = win32::GetParent(hwnd);
        let style_before = win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE);
        let ex_style_before = win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE);
        let visible_before = win32::IsWindowVisible(hwnd) != 0;

        let mut diagnostics = DetachDiagnostics {
            attempt,
            status: "set-parent-pending".into(),
            set_parent_return: "pending".into(),
            last_error_before: 0,
            last_error_after: 0,
            style_before: format!("0x{style_before:X}"),
            style_after: format!("0x{style_before:X}"),
            ex_style_before: format!("0x{ex_style_before:X}"),
            ex_style_after: format!("0x{ex_style_before:X}"),
            parent_before: format_hwnd(parent_before),
            parent_after_set_parent: "pending".into(),
            parent_after: format_hwnd(parent_before),
            visible_before,
            visible_after: visible_before,
            bounds_before: format_rect(before_rect),
            bounds_after: format_rect(before_rect),
        };

        win32::SetLastError(0);
        diagnostics.last_error_before = win32::GetLastError();
        let previous_parent = win32::SetParent(hwnd, 0);
        diagnostics.set_parent_return = format_hwnd(previous_parent);
        diagnostics.last_error_after = win32::GetLastError();
        let parent_after_set_parent = win32::GetParent(hwnd);
        diagnostics.parent_after_set_parent = format!(
            "{} · {}",
            format_hwnd(parent_after_set_parent),
            class_name(parent_after_set_parent)
        );

        // SetParent(NULL) can transiently make GetParent report the desktop
        // root while WS_CHILD is still present. A non-null return is the old
        // parent and therefore proves success; validate Parent == 0 only after
        // the top-level style and non-client frame have been restored.
        if previous_parent == 0 && diagnostics.last_error_after != 0 {
            diagnostics.status = "set-parent-failed".into();
            capture_detach_after(hwnd, &mut diagnostics);
            native.last_detach = diagnostics.clone();
            return Err(format!(
                "SetParent(NULL) failed: return={}, lastError={}, parentBefore={}, parentAfter={}",
                diagnostics.set_parent_return,
                diagnostics.last_error_after,
                diagnostics.parent_before,
                diagnostics.parent_after
            ));
        }

        // SetParent does not update WS_CHILD/WS_POPUP. For a NULL parent,
        // Microsoft requires the style transition after SetParent succeeds, and
        // success is judged only by the final observable parent — not by the
        // return value and not by the styles we asked for. A detach that reports
        // success while the HWND is still a child leaves the widget unreachable
        // behind every top-level window, which is worse than a failed detach.
        let normal_style = native.original_style.unwrap_or(style_before) & !win32::WS_CHILD;
        let normal_ex_style = native.original_ex_style.unwrap_or(ex_style_before);

        win32::SetLastError(0);
        let style_result = win32::SetWindowLongPtrW(hwnd, win32::GWL_STYLE, normal_style);
        let style_error = win32::GetLastError();
        if style_result == 0 && style_error != 0 {
            diagnostics.status = format!("style-restore-failed · lastError={style_error}");
            capture_detach_after(hwnd, &mut diagnostics);
            native.last_detach = diagnostics.clone();
            return Err(diagnostics.status.clone());
        }

        win32::SetLastError(0);
        let ex_style_result = win32::SetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE, normal_ex_style);
        let ex_style_error = win32::GetLastError();
        if ex_style_result == 0 && ex_style_error != 0 {
            diagnostics.status = format!("ex-style-restore-failed · lastError={ex_style_error}");
            capture_detach_after(hwnd, &mut diagnostics);
            native.last_detach = diagnostics.clone();
            return Err(diagnostics.status.clone());
        }

        let width = (before_rect.right - before_rect.left).max(360);
        let height = (before_rect.bottom - before_rect.top).max(500);
        win32::SetLastError(0);
        if win32::SetWindowPos(
            hwnd,
            win32::HWND_NOTOPMOST,
            before_rect.left,
            before_rect.top,
            width,
            height,
            win32::SWP_FRAMECHANGED | win32::SWP_SHOWWINDOW | win32::SWP_NOACTIVATE,
        ) == 0
        {
            let error = win32::GetLastError();
            diagnostics.status = format!("frame-change-failed · lastError={error}");
            capture_detach_after(hwnd, &mut diagnostics);
            native.last_detach = diagnostics.clone();
            return Err(diagnostics.status.clone());
        }

        capture_detach_after(hwnd, &mut diagnostics);
        diagnostics.status = if win32::GetParent(hwnd) == 0 {
            "detached".into()
        } else {
            "post-frame-parent-invalid".into()
        };
        native.last_detach = diagnostics.clone();

        if diagnostics.status != "detached" {
            return Err(format!(
                "detach verification failed: parentAfter={}",
                diagnostics.parent_after
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn interactive_child_style(style: isize) -> isize {
    (style | win32::WS_CHILD | win32::WS_CLIPSIBLINGS | win32::WS_TABSTOP) & !win32::WS_POPUP
}

#[cfg(target_os = "windows")]
fn frameless_product_child_style(style: isize) -> isize {
    // Do not "clean up" the bits below; each exception is load-bearing.
    //
    // WS_MAXIMIZEBOX and WS_TABSTOP share the same numeric bit. Once the HWND
    // becomes a child, that bit is the input-critical TABSTOP invariant and
    // must not be cleared with the other top-level frame controls.
    //
    // WS_THICKFRAME is deliberately *kept* even though the child is frameless:
    // it is the sizing-border bit the undecorated resize borders test for
    // (`TAURI_DRAG_RESIZE_BORDERS` only reports `HT*` when the parent carries
    // `WS_SIZEBOX`). A child window has no non-client area, so the bit does not
    // draw a frame — it only makes the resize hit zones reachable, which is what
    // lets the Desktop widget be resized through the same verified path as
    // Sidebar instead of a bespoke Win32 sizing loop.
    (interactive_child_style(style) & !(win32::WS_CAPTION | win32::WS_SYSMENU | win32::WS_MINIMIZEBOX))
        | win32::WS_THICKFRAME
}

#[cfg(target_os = "windows")]
fn interactive_child_ex_style(ex_style: isize) -> isize {
    // Do not drop any of these masks. WS_EX_APPWINDOW would put the taskbar
    // button back, WS_EX_TOPMOST would lift a desktop child above normal apps,
    // WS_EX_TRANSPARENT/WS_EX_NOACTIVATE would make the widget visible but
    // unclickable — the exact inert-widget regression.
    ex_style
        & !(win32::WS_EX_TOPMOST
            | win32::WS_EX_TRANSPARENT
            | win32::WS_EX_APPWINDOW
            | win32::WS_EX_NOACTIVATE)
}

#[cfg(target_os = "windows")]
fn constrain_desktop_widget(request: &DesktopWidgetRequest, client: win32::Rect) -> DesktopBounds {
    let host_width = (client.right - client.left).max(1) as u32;
    let host_height = (client.bottom - client.top).max(1) as u32;
    let requested = request.bounds.unwrap_or(DesktopBounds {
        x: client.right - request.default_width as i32 - request.margin,
        y: client.top + request.margin,
        width: request.default_width,
        height: request.default_height,
    });
    let width = requested.width.max(360).min(host_width);
    let height = requested.height.max(500).min(host_height);
    let max_x = client.right - width as i32;
    let max_y = client.bottom - height as i32;
    DesktopBounds {
        x: requested.x.clamp(client.left, max_x.max(client.left)),
        y: requested.y.clamp(client.top, max_y.max(client.top)),
        width,
        height,
    }
}

#[cfg(target_os = "windows")]
fn desktop_request_for_window_dpi(
    request: &DesktopWidgetRequest,
    window_dpi: u32,
) -> DesktopWidgetRequest {
    // The request already carries DIP-converted values, but a child window
    // inherits the DPI of the desktop host it was just reparented under, which
    // can differ from the Tauri scale factor captured before the reparent. A
    // monitor boundary or a mixed-DPI layout therefore needs this retarget or
    // the widget lands at the wrong physical size. Do not collapse this into the
    // DIP helpers in `product_window`: those convert for a known scale factor,
    // this one reconciles two different ones.
    let target_scale = if window_dpi == 0 {
        request.tauri_scale_factor
    } else {
        window_dpi as f64 / 96.0
    };
    let source_scale = if request.tauri_scale_factor.is_finite() && request.tauri_scale_factor > 0.0
    {
        request.tauri_scale_factor
    } else {
        1.0
    };
    let ratio = target_scale / source_scale;
    let scale_i32 = |value: i32| (value as f64 * ratio).round() as i32;
    let scale_u32 = |value: u32| (value as f64 * ratio).round().max(1.0) as u32;
    DesktopWidgetRequest {
        bounds: request.bounds.map(|bounds| DesktopBounds {
            x: scale_i32(bounds.x),
            y: scale_i32(bounds.y),
            width: scale_u32(bounds.width),
            height: scale_u32(bounds.height),
        }),
        default_width: scale_u32(request.default_width),
        default_height: scale_u32(request.default_height),
        margin: scale_i32(request.margin),
        tauri_scale_factor: request.tauri_scale_factor,
        logical_bounds: request.logical_bounds.clone(),
    }
}

#[cfg(target_os = "windows")]
fn format_physical_request(request: &DesktopWidgetRequest) -> String {
    request
        .bounds
        .map(|bounds| {
            format!(
                "{},{},{}x{}",
                bounds.x, bounds.y, bounds.width, bounds.height
            )
        })
        .unwrap_or_else(|| {
            format!(
                "auto-right,{}x{},margin={}",
                request.default_width, request.default_height, request.margin
            )
        })
}

#[cfg(target_os = "windows")]
fn desktop_bounds_for_hwnd(hwnd: win32::Hwnd) -> Result<DesktopBounds, String> {
    let mut rect = win32::Rect::default();
    // SAFETY: these are read-only queries on the live Tauri HWND. A Desktop
    // child reports screen coordinates through GetWindowRect, so its origin is
    // converted back into parent-client coordinates before persistence.
    unsafe {
        let parent = win32::GetParent(hwnd);
        if parent == 0 || win32::GetWindowRect(hwnd, &mut rect) == 0 {
            return Err("Desktop child bounds are unavailable".into());
        }
        let mut origin = win32::Point {
            x: rect.left,
            y: rect.top,
        };
        if win32::ScreenToClient(parent, &mut origin) == 0 {
            return Err("ScreenToClient failed while reading Desktop bounds".into());
        }
        Ok(DesktopBounds {
            x: origin.x,
            y: origin.y,
            width: (rect.right - rect.left).max(1) as u32,
            height: (rect.bottom - rect.top).max(1) as u32,
        })
    }
}

#[cfg(target_os = "windows")]
fn attach_to_desktop(
    hwnd: win32::Hwnd,
    native: &mut StoredNativeState,
    placement: DesktopPlacement,
) -> Result<(), String> {
    let (desktop_parent, strategy) = find_interactive_desktop_parent()?;
    let (tauri_scale_factor, logical_bounds, physical_requested) = match &placement {
        DesktopPlacement::Widget(request) => (
            format!("{:.3}", request.tauri_scale_factor),
            request.logical_bounds.clone(),
            format_physical_request(request),
        ),
        DesktopPlacement::PreserveCurrent => (
            "phase-one-preserve".into(),
            "phase-one-preserve".into(),
            "phase-one-preserve".into(),
        ),
    };
    let mut before_rect = win32::Rect::default();
    let mut parent_rect = win32::Rect::default();
    let mut parent_client_rect = win32::Rect::default();
    let mut after_parent_rect = win32::Rect::default();
    let mut final_rect = win32::Rect::default();

    // SAFETY: the transition mutex is held, both HWNDs were discovered in the
    // current Shell topology, and RECT/POINT pointers are valid. A child HWND's
    // SetWindowPos coordinates belong to the parent's client area. Product
    // Desktop clamps its independent widget rect within that client area; the
    // Phase 1 diagnostic path preserves its old screen rect and converts its
    // origin only after SetParent, keeping the proven PoC lifecycle unchanged.
    unsafe {
        win32::GetWindowRect(hwnd, &mut before_rect);
        win32::GetWindowRect(desktop_parent, &mut parent_rect);
        win32::GetClientRect(desktop_parent, &mut parent_client_rect);

        let style = win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE);
        let ex_style = win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE);
        let parent_before = win32::GetParent(hwnd);
        let mut diagnostics = AttachDiagnostics {
            status: "attach-pending".into(),
            hwnd: format_hwnd(hwnd),
            parent_before: format!(
                "{} · {}",
                format_hwnd(parent_before),
                class_name(parent_before)
            ),
            parent_target: format!(
                "{} · {}",
                format_hwnd(desktop_parent),
                class_name(desktop_parent)
            ),
            parent_after: "pending".into(),
            style_before: format!("0x{style:X}"),
            style_after: "pending".into(),
            ex_style_before: format!("0x{ex_style:X}"),
            ex_style_after: "pending".into(),
            bounds_before: format_rect(before_rect),
            bounds_after_parent: "pending".into(),
            bounds_final: "pending".into(),
            desktop_target_rect: format_rect(parent_rect),
            desktop_client_rect: format_rect(parent_client_rect),
            tauri_scale_factor,
            window_dpi: win32::GetDpiForWindow(hwnd),
            logical_bounds,
            physical_requested,
        };
        native.original_style.get_or_insert(style);
        native.original_ex_style.get_or_insert(ex_style);

        let desktop_style = if matches!(&placement, DesktopPlacement::Widget(_)) {
            frameless_product_child_style(style)
        } else {
            interactive_child_style(style)
        };
        let desktop_ex_style = interactive_child_ex_style(ex_style);
        win32::SetWindowLongPtrW(hwnd, win32::GWL_STYLE, desktop_style);
        win32::SetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE, desktop_ex_style);
        win32::SetParent(hwnd, desktop_parent);
        win32::EnableWindow(hwnd, 1);
        win32::GetWindowRect(hwnd, &mut after_parent_rect);

        let parent_after = win32::GetParent(hwnd);
        diagnostics.window_dpi = win32::GetDpiForWindow(hwnd);
        diagnostics.parent_after = format!(
            "{} · {}",
            format_hwnd(parent_after),
            class_name(parent_after)
        );
        diagnostics.bounds_after_parent = format_rect(after_parent_rect);

        if parent_after != desktop_parent {
            diagnostics.status = "parent-verification-failed".into();
            native.last_attach = diagnostics.clone();
            return Err("SetParent did not attach the window to SHELLDLL_DefView".into());
        }

        let applied = match placement {
            DesktopPlacement::Widget(ref request) => {
                let effective = desktop_request_for_window_dpi(request, diagnostics.window_dpi);
                diagnostics.physical_requested = format_physical_request(&effective);
                constrain_desktop_widget(&effective, parent_client_rect)
            }
            DesktopPlacement::PreserveCurrent => {
                let mut client_origin = win32::Point {
                    x: before_rect.left,
                    y: before_rect.top,
                };
                if win32::ScreenToClient(desktop_parent, &mut client_origin) == 0 {
                    diagnostics.status = "screen-to-client-failed".into();
                    native.last_attach = diagnostics.clone();
                    return Err("ScreenToClient failed while entering Desktop mode".into());
                }
                DesktopBounds {
                    x: client_origin.x,
                    y: client_origin.y,
                    width: (before_rect.right - before_rect.left).max(360) as u32,
                    height: (before_rect.bottom - before_rect.top).max(500) as u32,
                }
            }
        };
        if win32::SetWindowPos(
            hwnd,
            win32::HWND_TOP,
            applied.x,
            applied.y,
            applied.width as i32,
            applied.height as i32,
            win32::SWP_FRAMECHANGED | win32::SWP_SHOWWINDOW | win32::SWP_NOACTIVATE,
        ) == 0
        {
            diagnostics.status = "final-set-window-pos-failed".into();
            native.last_attach = diagnostics.clone();
            return Err("SetWindowPos failed while entering Desktop mode".into());
        }

        win32::GetWindowRect(hwnd, &mut final_rect);
        let applied_style = win32::GetWindowLongPtrW(hwnd, win32::GWL_STYLE);
        let applied_ex_style = win32::GetWindowLongPtrW(hwnd, win32::GWL_EXSTYLE);
        diagnostics.style_after = format!("0x{applied_style:X}");
        diagnostics.ex_style_after = format!("0x{applied_ex_style:X}");
        diagnostics.bounds_final = format_rect(final_rect);

        if applied_style & win32::WS_CHILD == 0 || applied_style & win32::WS_POPUP != 0 {
            diagnostics.status = "desktop-style-invalid".into();
            native.last_attach = diagnostics.clone();
            return Err(format!(
                "desktop style invalid after reparent: 0x{applied_style:X}"
            ));
        }
        if applied_ex_style & (win32::WS_EX_TRANSPARENT | win32::WS_EX_NOACTIVATE) != 0 {
            diagnostics.status = "desktop-ex-style-invalid".into();
            native.last_attach = diagnostics.clone();
            return Err(format!(
                "desktop extended style blocks input: 0x{applied_ex_style:X}"
            ));
        }

        diagnostics.status = "attached".into();
        native.last_attach = diagnostics.clone();
        let event = format!(
            "attach succeeded · hwnd={} · parent before={} target={} after={} · style {} → {} · ex-style {} → {} · logical={} · scale={} dpi={} · physical requested={} · bounds before={} after-parent={} final={} · desktop target={} client={}",
            diagnostics.hwnd,
            diagnostics.parent_before,
            diagnostics.parent_target,
            diagnostics.parent_after,
            diagnostics.style_before,
            diagnostics.style_after,
            diagnostics.ex_style_before,
            diagnostics.ex_style_after,
            diagnostics.logical_bounds,
            diagnostics.tauri_scale_factor,
            diagnostics.window_dpi,
            diagnostics.physical_requested,
            diagnostics.bounds_before,
            diagnostics.bounds_after_parent,
            diagnostics.bounds_final,
            diagnostics.desktop_target_rect,
            diagnostics.desktop_client_rect,
        );
        eprintln!("[desktop-lifecycle] {event}");
        record_lifecycle_event(event);
    }

    native.shell_strategy = strategy;
    native.warning = Some(
        "Desktop mode uses the SHELLDLL_DefView host discovered through Progman/WorkerW. It remains experimental until the complete manual gate passes."
            .into(),
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_windows_mode(
    window: &WebviewWindow,
    native: &mut StoredNativeState,
    mode: WindowMode,
    desktop_placement: DesktopPlacement,
) -> Result<(), String> {
    let _transition = desktop_transition()
        .lock()
        .map_err(|_| "desktop transition lock poisoned")?;
    let hwnd = hwnd_value(window)?;

    if matches!(native.mode, WindowMode::Desktop) && !matches!(mode, WindowMode::Desktop) {
        set_desktop_requested(false, hwnd);
        if let Err(error) = detach_from_desktop(hwnd, native) {
            // A failed detach means the native window remains in Desktop mode;
            // keep lifecycle ownership aligned with the actual HWND state.
            set_desktop_requested(true, hwnd);
            update_lifecycle_probe(read_attachment_probe(hwnd));
            record_lifecycle_event(format!("detach failed · {error}"));
            return Err(error);
        }
        record_lifecycle_event(format!(
            "detach succeeded · {} → {} · bounds {} → {}",
            native.last_detach.parent_before,
            native.last_detach.parent_after,
            native.last_detach.bounds_before,
            native.last_detach.bounds_after
        ));
    }

    match mode {
        WindowMode::Desktop => {
            window
                .set_always_on_top(false)
                .map_err(|error| error.to_string())?;
            window
                .set_focusable(true)
                .map_err(|error| error.to_string())?;
            attach_to_desktop(hwnd, native, desktop_placement)?;
            set_desktop_requested(true, hwnd);
            let probe = read_attachment_probe(hwnd);
            update_lifecycle_probe(probe);
            ensure_desktop_lifecycle_monitor(window.clone());
        }
        WindowMode::Normal => {
            set_desktop_requested(false, hwnd);
            window
                .set_focusable(true)
                .map_err(|error| error.to_string())?;
            window
                .set_always_on_top(false)
                .map_err(|error| error.to_string())?;
            native.shell_strategy = "tauri-normal-window".into();
            native.warning = None;
        }
        WindowMode::AlwaysOnTop => {
            set_desktop_requested(false, hwnd);
            window
                .set_focusable(true)
                .map_err(|error| error.to_string())?;
            window
                .set_always_on_top(true)
                .map_err(|error| error.to_string())?;
            native.shell_strategy = "tauri-always-on-top".into();
            native.warning = Some(
                "Always-on-top is a stable fallback, but it can overlay fullscreen applications by design."
                    .into(),
            );
        }
    }

    // Taskbar membership is a property of the *product* (a tray-resident widget),
    // not of a window mode, so it is enforced once for every mode by
    // `platform::windows::widget_frame` instead of being toggled here. Tauri's own
    // `set_skip_taskbar` used to be called in these branches; on Windows tao backs
    // it with `ITaskbarList::AddTab`/`DeleteTab`, which fights the tool-window
    // style the widget contract relies on — `AddTab` on a `WS_EX_TOOLWINDOW`
    // window puts the taskbar button back even though the Win32 style says the
    // window is not an application window. Re-asserting the frame after every
    // transition keeps the authoritative style state and the Shell state aligned.
    //
    // It also runs *after* the per-mode tao flag setters above on purpose: each
    // setter recomputes the whole Win32 style from tao's flags, so this is the
    // only ordering in which the widget ex-style is the last word.
    #[cfg(target_os = "windows")]
    if let Err(error) = crate::platform::windows::widget_frame::enforce_on(hwnd) {
        record_lifecycle_event(format!("widget frame re-assert failed · {error}"));
        return Err(error);
    }

    sync_webview_input(window, true)?;
    // `native.mode` is the product's record of what was applied, not a request.
    // Recovery compares against it, so it must only advance once the whole
    // transition has actually landed.
    native.mode = mode;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::WindowMode;

    #[cfg(target_os = "windows")]
    use super::{
        constrain_desktop_widget, desktop_request_for_window_dpi, frameless_product_child_style,
        interactive_child_ex_style, interactive_child_style, widget_invariant_issues, win32,
        AttachmentProbe, DesktopBounds, DesktopWidgetRequest, WidgetInvariant,
    };

    #[cfg(target_os = "windows")]
    fn widget_request(bounds: Option<DesktopBounds>) -> DesktopWidgetRequest {
        DesktopWidgetRequest {
            bounds,
            default_width: 630,
            default_height: 1050,
            margin: 48,
            tauri_scale_factor: 1.5,
            logical_bounds: "420x700 DIP".into(),
        }
    }

    #[test]
    fn parses_supported_modes() {
        assert!(matches!(
            WindowMode::try_from("desktop"),
            Ok(WindowMode::Desktop)
        ));
        assert!(matches!(
            WindowMode::try_from("normal"),
            Ok(WindowMode::Normal)
        ));
        assert!(matches!(
            WindowMode::try_from("always_on_top"),
            Ok(WindowMode::AlwaysOnTop)
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn product_desktop_defaults_to_a_right_aligned_widget() {
        let client = win32::Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1600,
        };
        assert_eq!(
            constrain_desktop_widget(&widget_request(None), client),
            DesktopBounds {
                x: 1882,
                y: 48,
                width: 630,
                height: 1050,
            }
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn product_desktop_geometry_is_clamped_inside_host_client_rect() {
        let client = win32::Rect {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 720,
        };
        assert_eq!(
            constrain_desktop_widget(
                &widget_request(Some(DesktopBounds {
                    x: 4000,
                    y: -80,
                    width: 1600,
                    height: 900,
                })),
                client,
            ),
            DesktopBounds {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            }
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn desktop_request_retargets_to_dpi_reported_after_reparent() {
        let request = DesktopWidgetRequest {
            bounds: Some(DesktopBounds {
                x: 40,
                y: 32,
                width: 420,
                height: 700,
            }),
            default_width: 420,
            default_height: 700,
            margin: 32,
            tauri_scale_factor: 1.0,
            logical_bounds: "40,32,420x700 DIP".into(),
        };
        let retargeted = desktop_request_for_window_dpi(&request, 144);
        assert_eq!(
            retargeted.bounds,
            Some(DesktopBounds {
                x: 60,
                y: 48,
                width: 630,
                height: 1050,
            })
        );
        assert_eq!(
            (retargeted.default_width, retargeted.default_height),
            (630, 1050)
        );
        assert_eq!(retargeted.margin, 48);
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(WindowMode::try_from("floating").is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn desktop_style_is_focusable_child_not_popup() {
        let style = interactive_child_style(win32::WS_POPUP);
        assert_ne!(style & win32::WS_CHILD, 0);
        assert_ne!(style & win32::WS_TABSTOP, 0);
        assert_eq!(style & win32::WS_POPUP, 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn product_desktop_style_is_frameless_and_keeps_child_input_bits() {
        let style = frameless_product_child_style(
            win32::WS_POPUP
                | win32::WS_CAPTION
                | win32::WS_THICKFRAME
                | win32::WS_SYSMENU
                | win32::WS_MINIMIZEBOX,
        );
        assert_ne!(style & win32::WS_CHILD, 0);
        assert_ne!(style & win32::WS_TABSTOP, 0);
        assert_eq!(style & win32::WS_POPUP, 0);
        assert_eq!(style & win32::WS_CAPTION, 0);
        assert_eq!(style & win32::WS_SYSMENU, 0);
        assert_eq!(style & win32::WS_MINIMIZEBOX, 0);
        // Sizing-border bit: retained for the undecorated resize hit zones. It
        // is not a visible frame on a child window, which has no non-client area.
        assert_ne!(style & win32::WS_THICKFRAME, 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn product_desktop_style_adds_the_sizing_bit_even_without_it() {
        // Desktop is entered from a window whose WS_THICKFRAME may already have
        // been cleared (unlocked Floating keeps it, locked Floating and Orb do
        // not), so the resize hit zones must not depend on the incoming style.
        let style = frameless_product_child_style(win32::WS_POPUP | win32::WS_CAPTION);
        assert_ne!(style & win32::WS_THICKFRAME, 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn desktop_extended_style_does_not_block_input_or_activation() {
        let original = win32::WS_EX_TOPMOST
            | win32::WS_EX_TRANSPARENT
            | win32::WS_EX_APPWINDOW
            | win32::WS_EX_NOACTIVATE;
        let ex_style = interactive_child_ex_style(original);
        assert_eq!(ex_style, 0);
    }

    #[cfg(target_os = "windows")]
    fn valid_attachment_probe() -> AttachmentProbe {
        AttachmentProbe {
            hwnd_valid: true,
            visible: true,
            parent: 10,
            current_def_view: 10,
            rect: win32::Rect {
                left: 100,
                top: 100,
                right: 560,
                bottom: 780,
            },
            style: win32::WS_CHILD,
            ex_style: 0,
            hit_target: 11,
            hit_owned: true,
            desktop_foreground: true,
            bounds_valid: true,
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn covered_widget_does_not_trigger_z_order_recovery_over_normal_apps() {
        let mut probe = valid_attachment_probe();
        probe.desktop_foreground = false;
        probe.hit_owned = false;
        probe.visible = false;
        assert!(probe.attachment_valid());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn desktop_hit_test_loss_requires_recovery() {
        let mut probe = valid_attachment_probe();
        probe.hit_owned = false;
        assert!(!probe.attachment_valid());
        assert!(probe.issues().contains(&"desktop-z-order-invalid"));
    }

    #[cfg(target_os = "windows")]
    fn stable_widget_invariant() -> WidgetInvariant {
        WidgetInvariant {
            hwnd: 20,
            parent: 10,
            style: win32::WS_CHILD,
            ex_style: 0,
            rect: (100, 100, 560, 780),
            visible: true,
            iconic: false,
            enabled: true,
            z_prev: 21,
            z_next: 22,
            hit_target: 23,
            hit_owned: true,
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn observer_control_accepts_an_unchanged_interactive_widget() {
        let invariant = stable_widget_invariant();
        assert!(widget_invariant_issues(invariant, invariant).is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn observer_control_reports_window_and_input_mutations() {
        let baseline = stable_widget_invariant();
        let mut changed = baseline;
        changed.parent = 99;
        changed.rect.0 += 1;
        changed.hit_owned = false;
        let issues = widget_invariant_issues(baseline, changed);
        assert!(issues.contains(&"parent-changed"));
        assert!(issues.contains(&"bounds-changed"));
        assert!(issues.contains(&"hit-route-not-owned"));
    }
}
