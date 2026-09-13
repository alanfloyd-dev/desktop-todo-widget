//! Mouse-input target repair for the composition-hosted Desktop widget.
//!
//! WebView2's composition hosting still owns a real Win32 window for the WebView:
//! a Chromium window (class `Chrome_WidgetWin_*`, plus its render-host child) in
//! the msedgewebview2 browser process, kept exactly over the
//! `ICoreWebView2Controller` bounds so that WebView2 can serve input, IME, and
//! cursor state. While the product window is a top-level window that runtime
//! window sits *below* it, the product window is the mouse target, and the bridge
//! in [`super::input`] receives the messages it forwards with `SendMouseInput`.
//!
//! A Desktop widget is a `WS_CHILD` of `SHELLDLL_DefView` (`window_mode`), so it
//! lives inside the desktop's top-level band — the lowest one there is. The
//! WebView2 runtime window cannot be pushed below it (the shell keeps the desktop
//! at the bottom of the z-order), so it wins the hit test instead, the subclass on
//! the product HWND never sees a pointer message, and the widget is visible but
//! completely inert: no context menu, no drag, no wheel.
//!
//! Hiding that window does restore the hit test, but then WebView2 stops
//! delivering forwarded input to the page, so the repair has to keep it *visible*
//! and merely move it off the product client rectangle: it stays a live window for
//! WebView2, and the pointer over the widget reaches the product window again.
//! This is the only lever that satisfies both halves — the arrangement is exactly
//! the Floating one (runtime window fully clear of the product window, still
//! shown), just reached by moving the runtime window instead of covering it.
//!
//! Two filters keep the blast radius at exactly the intercepting runtime window:
//! it must belong to a **direct child process** of this process (the WebView2
//! browser process) and its class must be one of Chromium's window classes. A
//! window that does not also **cover** the product client rect cannot make the
//! product window unreachable and is left where it is.

use std::ffi::c_void;

use windows::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::ClientToScreen,
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Threading::GetCurrentProcessId,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, GetSystemMetrics, GetWindowRect,
        GetWindowThreadProcessId, IsWindowVisible, SetWindowPos, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
        SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    },
};
use windows_core::BOOL;

/// What one repair pass saw and did.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct InputTargetReport {
    /// Visible runtime windows that cover the product client rect.
    pub(crate) candidates: usize,
    /// Candidates successfully moved off the product client rect.
    pub(crate) relocated: usize,
    /// Chromium class names of the moved windows, for the diagnostic line.
    pub(crate) classes: Vec<String>,
}

impl InputTargetReport {
    /// Called out separately because "no candidate" is the healthy steady state.
    pub(crate) fn is_clear(&self) -> bool {
        self.candidates == 0
    }
}

/// Chromium window classes that can belong to the WebView2 runtime surface.
const WEBVIEW_CLASS_PREFIXES: [&str; 3] = [
    "Chrome_WidgetWin",
    "Chrome_RenderWidgetHostHWND",
    "Intermediate D3D Window",
];

/// Clears every WebView2 runtime window that covers the product client area off
/// that rectangle, so pointer input reaches the product window again.
///
/// # Safety
/// `hwnd` must be the live product window on the calling thread's desktop.
pub(crate) unsafe fn clear_covering_webview_windows(hwnd: HWND) -> InputTargetReport {
    let mut report = InputTargetReport::default();
    let Some(client) = client_rect_in_screen_space(hwnd) else {
        return report;
    };
    let Ok(candidates) = top_level_windows() else {
        return report;
    };
    let own_pid = GetCurrentProcessId();
    let webview_pids = direct_child_process_ids(own_pid);
    let Some(parking) = parking_spot(client) else {
        return report;
    };

    for candidate in candidates {
        if candidate == hwnd || !IsWindowVisible(candidate).as_bool() {
            continue;
        }
        let mut pid = 0_u32;
        GetWindowThreadProcessId(candidate, Some(&mut pid));
        if pid == own_pid || !webview_pids.contains(&pid) {
            continue;
        }
        let class = window_class(candidate);
        if !is_webview_class(&class) {
            continue;
        }
        let mut rect = RECT::default();
        if GetWindowRect(candidate, &mut rect).is_err() || !covers(rect, client) {
            continue;
        }
        report.candidates += 1;
        if SetWindowPos(
            candidate,
            None,
            parking.left,
            parking.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
        )
        .is_ok()
        {
            report.relocated += 1;
            report.classes.push(class);
        }
    }
    report
}

/// Where the runtime window is parked: fully outside the virtual screen, one
/// window width to the left of it.
///
/// Off-screen is load-bearing, and so is keeping the window visible.
/// Do not "simplify" this to `ShowWindow(SW_HIDE)`: hiding does restore the hit
/// test, but WebView2 then stops delivering forwarded input to the page, so the
/// widget renders and does nothing. Any on-screen rectangle would take the
/// pointer away from whatever is under it instead, so the window has to leave the
/// desktop entirely while keeping its `WS_VISIBLE` bit.
fn parking_spot(client: RECT) -> Option<RECT> {
    let virtual_left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let virtual_top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return None;
    }
    let host_width = (client.right - client.left).max(1);
    Some(RECT {
        left: virtual_left - host_width - 16,
        top: virtual_top,
        right: virtual_left - 16,
        bottom: virtual_top + (client.bottom - client.top).max(1),
    })
}

/// True when `class` names a Chromium window that the WebView2 runtime owns.
fn is_webview_class(class: &str) -> bool {
    WEBVIEW_CLASS_PREFIXES
        .iter()
        .any(|prefix| class.starts_with(prefix))
}

/// True when `outer` fully contains `inner`; only such a window can make the
/// product window unreachable for the pointer.
fn covers(outer: RECT, inner: RECT) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

/// The client area of `hwnd`, expressed in screen coordinates.
///
/// # Safety
/// `hwnd` must be a live window.
unsafe fn client_rect_in_screen_space(hwnd: HWND) -> Option<RECT> {
    let mut client = RECT::default();
    GetClientRect(hwnd, &mut client).ok()?;
    let mut origin = POINT { x: 0, y: 0 };
    if !ClientToScreen(hwnd, &mut origin).as_bool() {
        return None;
    }
    Some(RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + (client.right - client.left),
        bottom: origin.y + (client.bottom - client.top),
    })
}

/// Every top-level window on the desktop.
///
/// # Safety
/// Called on a thread with a message queue; the callback only appends to a
/// vector that outlives the enumeration.
unsafe fn top_level_windows() -> windows::core::Result<Vec<HWND>> {
    unsafe extern "system" fn collect(window: HWND, state: LPARAM) -> BOOL {
        let windows = &mut *(state.0 as *mut Vec<HWND>);
        windows.push(window);
        BOOL(1)
    }

    let mut windows: Vec<HWND> = Vec::new();
    EnumWindows(
        Some(collect),
        LPARAM((&mut windows as *mut Vec<HWND>).cast::<c_void>() as isize),
    )?;
    Ok(windows)
}

/// Process ids whose parent is `parent_pid`, i.e. the processes this app started.
fn direct_child_process_ids(parent_pid: u32) -> Vec<u32> {
    let mut children = Vec::new();
    // SAFETY: the snapshot handle is closed on every path, and the entry struct
    // is initialised with the size the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return children;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut has_entry = Process32FirstW(snapshot, &mut entry).is_ok();
        while has_entry {
            if entry.th32ParentProcessID == parent_pid {
                children.push(entry.th32ProcessID);
            }
            has_entry = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    children
}

/// The window class name of `hwnd`, or an empty string when unavailable.
fn window_class(hwnd: HWND) -> String {
    let mut buffer = [0_u16; 256];
    // SAFETY: the buffer is writable for the length passed to the API.
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn cover_requires_the_whole_client_area() {
        let client = rect(100, 100, 300, 400);
        assert!(covers(rect(0, 0, 2560, 1600), client));
        assert!(covers(rect(100, 100, 300, 400), client));
        // Partial overlaps and containment the wrong way round cannot make the
        // product window unreachable, so they must be left alone.
        assert!(!covers(rect(150, 100, 300, 400), client));
        assert!(!covers(rect(100, 0, 300, 300), client));
        assert!(!covers(rect(200, 200, 400, 500), client));
    }

    #[test]
    fn only_chromium_window_classes_are_eligible() {
        assert!(is_webview_class("Chrome_WidgetWin_1"));
        assert!(is_webview_class("Chrome_WidgetWin_0"));
        assert!(is_webview_class("Chrome_RenderWidgetHostHWND"));
        assert!(is_webview_class("Intermediate D3D Window"));
        assert!(!is_webview_class("Tauri Window"));
        assert!(!is_webview_class("Progman"));
        assert!(!is_webview_class("SHELLDLL_DefView"));
        assert!(!is_webview_class(""));
    }

    #[test]
    fn parking_spot_is_outside_the_virtual_screen() {
        let client = rect(1930, 48, 2560, 1098);
        let spot = parking_spot(client).expect("virtual screen metrics available");
        let virtual_left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        assert!(spot.right <= virtual_left, "parked spot must not overlap any monitor");
        assert!(spot.left < spot.right);
        // Same footprint, so the runtime window only loses its on-screen position.
        assert_eq!(spot.right - spot.left, client.right - client.left);
        assert_eq!(spot.bottom - spot.top, client.bottom - client.top);
    }

    #[test]
    fn empty_report_is_clear() {
        assert!(InputTargetReport::default().is_clear());
        let report = InputTargetReport {
            candidates: 1,
            relocated: 1,
            classes: vec!["Chrome_WidgetWin_1".into()],
        };
        assert!(!report.is_clear());
    }
}
