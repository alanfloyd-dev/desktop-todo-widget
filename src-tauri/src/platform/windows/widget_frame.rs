//! Widget window semantics: the product HWND is never an application window.
//!
//! The product is a tray-resident widget. Every window mode — Sidebar, Floating
//! expanded, Floating collapsed (Orb) and Desktop — is a widget presentation, so
//! none of them may appear in the Windows taskbar, and none of them should behave
//! like a regular application window in Alt+Tab. The tray icon in
//! `product_commands::install_tray` is the resident entry point that switches
//! modes and brings the widget back.
//!
//! # Why the product window still reached the taskbar
//!
//! Two independent causes, both in third-party style handling rather than in the
//! product layer:
//!
//! 1. `tauri.conf.json` declared `skipTaskbar: false` (the Tauri default), so
//!    tao's `WindowFlags::ON_TASKBAR` was set. `WindowFlags::to_window_styles`
//!    turns exactly that flag into `WS_EX_APPWINDOW`, the style bit Windows
//!    documents as *"forces the window to appear on the taskbar"*.
//! 2. Tauri's runtime `set_skip_taskbar` does **not** touch the extended style.
//!    tao implements it with `ITaskbarList::DeleteTab` / `AddTab`, a Shell request
//!    that only removes the *existing* taskbar button. It leaves `WS_EX_APPWINDOW`
//!    in place, so the button comes back as soon as the Shell re-evaluates the
//!    window (`explorer.exe` restart, session change, a later show/activate) — and
//!    it never affected Alt+Tab at all.
//!
//! # The mechanism used instead
//!
//! `WS_EX_TOOLWINDOW` on a window that is not owned by another window is the
//! documented Win32 way to say "this window is not an application window": the
//! Shell neither creates a taskbar button for it nor lists it in Alt+Tab. It is
//! persistent state on the window itself rather than a Shell request, so it
//! survives an `explorer.exe` restart.
//!
//! Setting the bit once is not enough, because tao rewrites the **whole**
//! extended style from its own flag set on every window-flag change
//! (`WindowFlags::update` → `SetWindowLongW(GWL_EXSTYLE, style_ex)`), and the
//! product mode transitions call several of those setters (`set_resizable`,
//! `set_always_on_top`, `set_shadow`, …). Rather than re-asserting the bit after
//! every call — a list that silently rots as soon as a new setter is added — this
//! module subclasses the product HWND with `SetWindowSubclass` and rewrites the
//! proposed extended style in `WM_STYLECHANGING`. Windows sends that message
//! before it applies any `SetWindowLong*` style change, so the widget style is
//! enforced *by the window itself* at the OS level, whoever is changing it and in
//! whichever order.
//!
//! This is normal Win32 window semantics. It deliberately does not hide the
//! window, create an owner window, or use `ITaskbarList`/Shell hooks.
//!
//! # Scope
//!
//! Only the extended-style bits that decide taskbar and Alt+Tab membership are
//! touched: `WS_EX_TOOLWINDOW` is set and `WS_EX_APPWINDOW` is cleared. Nothing
//! else in the style, geometry, composition hosting, acrylic material, input
//! routing or persisted geometry is read or changed here.

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, SetWindowSubclass, SUBCLASSPROC},
        WindowsAndMessaging::{
            GetWindow, GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, GW_OWNER, STYLESTRUCT,
            WM_STYLECHANGING, WS_CHILD, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        },
    },
};

/// The product window handle, as the raw value Tauri exposes.
///
/// Kept a plain `isize` so no caller outside this module needs the `windows`
/// crate: upper layers only carry the value through, they never interpret it.
pub(crate) type WidgetHwnd = isize;

/// Identity of the subclass installed on the product window.
///
/// `SetWindowSubclass` keys on the `(pfnSubclass, uIdSubclass)` pair, so a fixed
/// id keeps a repeated install idempotent instead of stacking procedures.
const WIDGET_FRAME_SUBCLASS_ID: usize = 0x414C_414E; // 'ALAN'

/// Whether the subclass is already on the product window.
///
/// The product window is created once and destroyed with the process, and
/// `SetWindowSubclass` unregisters itself on `WM_NCDESTROY`, so one flag is enough
/// to keep repeated enforcement idempotent. A second frame-carrying window would
/// need this to become per-handle state.
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wraps a raw Tauri window handle for the Win32 calls in this module.
fn window(value: WidgetHwnd) -> HWND {
    HWND(value as *mut std::ffi::c_void)
}

/// The widget extended style: a tool window, never an application window.
///
/// Pure so it can be asserted directly in tests. `WS_EX_APPWINDOW` and
/// `WS_EX_TOOLWINDOW` are mutually exclusive in practice; Windows gives
/// `WS_EX_APPWINDOW` priority when both are present, so the application-window bit
/// is cleared rather than merely left alone.
pub(crate) fn widget_ex_style(ex_style: isize) -> isize {
    (ex_style & !app_window_bit()) | tool_window_bit()
}

/// True when the tool-window bit this module enforces is present.
pub(crate) fn is_widget_frame(ex_style: isize) -> bool {
    ex_style & tool_window_bit() != 0
}

/// Whether Windows would give a window with these facts a taskbar button.
///
/// This is the documented Shell placement rule, used by the mode diagnostics so
/// the claim "no widget mode is on the taskbar" is verifiable from the app itself
/// instead of inferred from a style dump:
///
/// * a child window is never on the taskbar;
/// * `WS_EX_TOOLWINDOW` takes the window off the taskbar;
/// * `WS_EX_APPWINDOW` forces the window onto the taskbar;
/// * otherwise an unowned top-level window has a taskbar button and an owned one
///   does not.
///
/// Pure in the three facts the rule needs, so it is unit-testable without a live
/// window; [`taskbar_eligible`] supplies them from an HWND.
pub(crate) fn taskbar_eligible_from(ex_style: isize, has_owner: bool, is_child: bool) -> bool {
    if is_child {
        return false;
    }
    let app_window = ex_style & app_window_bit() != 0;
    if is_widget_frame(ex_style) {
        // `WS_EX_APPWINDOW` wins when both bits are set, which is the one case
        // where a tool window still gets a button.
        return app_window;
    }
    app_window || !has_owner
}

/// Whether Windows lists such a window in Alt+Tab.
///
/// Alt+Tab and the taskbar are decided by the same `WS_EX_TOOLWINDOW` /
/// `WS_EX_APPWINDOW` tests but are otherwise separate mechanisms: a window can be
/// kept off the taskbar only through `ITaskbarList` or an owner and still appear in
/// the switcher. The product has no owner and no Shell tricks, so for the widget
/// the two answers coincide — reported as two fields anyway so a future divergence
/// is visible instead of assumed away.
pub(crate) fn alt_tab_eligible_from(ex_style: isize, has_owner: bool, is_child: bool) -> bool {
    taskbar_eligible_from(ex_style, has_owner, is_child)
}

/// Whether Windows would give the live `hwnd` a taskbar button.
///
/// # Safety
/// `hwnd` must be a window handle; it is only queried.
pub(crate) unsafe fn taskbar_eligible(hwnd: HWND) -> bool {
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    // `GetWindow` fails on an invalid handle, which cannot happen for the live
    // product window; treat that as "no owner".
    let has_owner = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|owner| !owner.is_invalid());
    // A child window carries `WS_CHILD`; for a top-level window `GetParent` reports
    // the owner instead, so the style bit is the reliable discriminator.
    let is_child = ex_style & (WS_CHILD.0 as isize) != 0;
    taskbar_eligible_from(ex_style, has_owner, is_child)
}

/// Enforces the widget extended style on `hwnd` right now.
///
/// Returns the extended style before and after the change so the caller can log
/// the transition. `SetWindowLongPtrW` does not raise `WM_STYLECHANGING`, so this
/// cannot recurse into [`widget_subclass_proc`].
///
/// # Safety
/// `hwnd` must be a live window owned by the calling thread.
unsafe fn apply_widget_ex_style(hwnd: HWND) -> (isize, isize) {
    let before = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let wanted = widget_ex_style(before);
    if wanted != before {
        unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted) };
    }
    (before, unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) })
}

/// Subclass procedure that keeps the widget style on every style change.
///
/// Only `WM_STYLECHANGING` for `GWL_EXSTYLE` is inspected: `lParam` points at the
/// `STYLESTRUCT` Windows is about to apply, and rewriting `styleNew` in place is
/// what the message exists for. Every other message, and every other style index,
/// is forwarded to the original procedure untouched.
unsafe extern "system" fn widget_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 {
        // SAFETY: for WM_STYLECHANGING Windows passes a valid STYLESTRUCT pointer
        // in lParam and keeps it alive for the duration of the call.
        let structure = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        structure.styleNew = widget_ex_style(structure.styleNew as isize) as u32;
    }
    // SAFETY: forwarding the untouched message to the original procedure is the
    // contract of DefSubclassProc.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// What [`enforce_on`] observed, for the diagnostic line and the QA probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WidgetFrameReport {
    /// Whether this call installed the subclass (false when it only re-asserted).
    pub(crate) installed: bool,
    pub(crate) ex_style_before: String,
    pub(crate) ex_style_after: String,
    pub(crate) enforces_tool_window: bool,
    pub(crate) clearing_succeeded: bool,
    /// What Windows' taskbar rule says about the window right now.
    pub(crate) taskbar_eligible: bool,
}

impl WidgetFrameReport {
    /// One-line record for the QA diagnostics stream.
    pub(crate) fn summary(&self) -> String {
        format!(
            "widget_frame installed={} ex_style {} -> {} tool_window={} app_window_cleared={} taskbar_eligible={}",
            self.installed,
            self.ex_style_before,
            self.ex_style_after,
            self.enforces_tool_window,
            self.clearing_succeeded,
            self.taskbar_eligible
        )
    }
}

/// Installs the widget frame on the product window and applies it immediately.
///
/// Must be called on the thread that owns `hwnd` (Tauri's main thread — the same
/// thread `setup` runs on), which is also the thread the product mode transitions
/// run on, so there is no cross-thread window-procedure race.
///
/// Idempotent: a second call only re-asserts the style.
pub(crate) fn enforce_on(raw: WidgetHwnd) -> Result<WidgetFrameReport, String> {
    let hwnd = window(raw);
    if hwnd.is_invalid() {
        return Err("widget frame requires a live product window".into());
    }

    let already_installed = INSTALLED.swap(true, std::sync::atomic::Ordering::AcqRel);
    if !already_installed {
        let subclass: SUBCLASSPROC = Some(widget_subclass_proc);
        // SAFETY: `hwnd` is a live window and the procedure lives in this module's
        // static code, so it outlives the window.
        let installed = unsafe { SetWindowSubclass(hwnd, subclass, WIDGET_FRAME_SUBCLASS_ID, 0) };
        if !installed.as_bool() {
            INSTALLED.store(false, std::sync::atomic::Ordering::Release);
            return Err("SetWindowSubclass failed for the product window".into());
        }
    }

    // SAFETY: enforced on the owning thread, see the function contract.
    let (before, after) = unsafe { apply_widget_ex_style(hwnd) };
    // SAFETY: read-only query of a live window.
    let taskbar_eligible = unsafe { taskbar_eligible(hwnd) };
    let report = WidgetFrameReport {
        installed: !already_installed,
        ex_style_before: format!("0x{before:X}"),
        ex_style_after: format!("0x{after:X}"),
        enforces_tool_window: is_widget_frame(after),
        clearing_succeeded: after & app_window_bit() == 0,
        taskbar_eligible,
    };
    if !report.enforces_tool_window {
        return Err(format!(
            "widget frame rejected: ex-style {} has no WS_EX_TOOLWINDOW after enforcement",
            report.ex_style_after
        ));
    }
    if !report.clearing_succeeded {
        return Err(format!(
            "widget frame incomplete: ex-style {} still carries WS_EX_APPWINDOW",
            report.ex_style_after
        ));
    }
    Ok(report)
}

/// Removes the widget frame. Test-only.
///
/// Not part of the product path: the product window is destroyed exactly once, with
/// the process, and `SetWindowSubclass` unregisters itself on `WM_NCDESTROY`. Kept
/// so a test can prove the enforcement is *removable*, which is what separates this
/// mechanism from a one-way style mutation an upper layer could never undo.
///
/// # Safety
/// `hwnd` must be a live window owned by the calling thread.
#[cfg(test)]
pub(crate) unsafe fn release_from(hwnd: HWND) {
    use windows::Win32::UI::Shell::RemoveWindowSubclass;

    if hwnd.is_invalid() {
        return;
    }
    let subclass: SUBCLASSPROC = Some(widget_subclass_proc);
    // SAFETY: `hwnd` is live and the procedure is the one that was installed.
    let _ = unsafe { RemoveWindowSubclass(hwnd, subclass, WIDGET_FRAME_SUBCLASS_ID) };
    INSTALLED.store(false, std::sync::atomic::Ordering::Release);
}

fn tool_window_bit() -> isize {
    WS_EX_TOOLWINDOW.0 as isize
}

fn app_window_bit() -> isize {
    WS_EX_APPWINDOW.0 as isize
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: isize = WS_EX_APPWINDOW.0 as isize;
    const TOOL: isize = WS_EX_TOOLWINDOW.0 as isize;
    /// A representative tao extended style for a frameless, resizable, non-topmost
    /// product window (`WS_EX_WINDOWEDGE | WS_EX_ACCEPTFILES`).
    const TAO_BASE: isize = 0x0000_0100 | 0x0000_0010;

    #[test]
    fn widget_style_sets_tool_window_and_clears_app_window() {
        assert_eq!(widget_ex_style(TAO_BASE), TAO_BASE | TOOL);
        assert_eq!(widget_ex_style(TAO_BASE | APP), TAO_BASE | TOOL);
        assert_eq!(widget_ex_style(TAO_BASE | APP | TOOL), TAO_BASE | TOOL);
        assert_eq!(widget_ex_style(TOOL), TOOL);
    }

    #[test]
    fn widget_style_is_idempotent_and_preserves_other_bits() {
        let once = widget_ex_style(TAO_BASE | APP);
        assert_eq!(widget_ex_style(once), once, "re-asserting must be a no-op");
        // Topmost, no-activate, window edge and transparent bits are none of this
        // module's business and must survive untouched.
        let others = 0x0000_0008 | 0x0800_0000 | 0x0000_0100 | 0x0000_0020;
        assert_eq!(widget_ex_style(TAO_BASE | APP | others) & others, others);
    }

    #[test]
    fn app_window_never_remains_after_enforcement() {
        // The regression this module exists for: tao re-applies WS_EX_APPWINDOW
        // from its own flags, and the widget bit has to win.
        let tao_reapplied = widget_ex_style(widget_ex_style(TAO_BASE) | APP);
        assert_eq!(tao_reapplied & APP, 0);
        assert!(is_widget_frame(tao_reapplied));
    }

    #[test]
    fn tool_window_is_not_taskbar_eligible() {
        assert!(!taskbar_eligible_from(TOOL, false, false));
        assert!(!taskbar_eligible_from(TAO_BASE | TOOL, true, false));
    }

    #[test]
    fn taskbar_rule_matches_documented_placement() {
        // Unowned top-level, no tool bit: on the taskbar.
        assert!(taskbar_eligible_from(TAO_BASE, false, false));
        // Owned top-level without the tool bit: not on the taskbar.
        assert!(!taskbar_eligible_from(TAO_BASE, true, false));
        // WS_EX_APPWINDOW forces a button, owned or not.
        assert!(taskbar_eligible_from(TAO_BASE | APP, false, false));
        assert!(taskbar_eligible_from(TAO_BASE | APP, true, false));
        // Child windows (the Desktop widget) are never on the taskbar.
        assert!(!taskbar_eligible_from(TAO_BASE, false, true));
        assert!(!taskbar_eligible_from(TAO_BASE | TOOL, false, true));
        // Both bits set: Windows gives WS_EX_APPWINDOW priority.
        assert!(taskbar_eligible_from(TOOL | APP, false, false));
    }

    #[test]
    fn every_widget_presentation_is_taskbar_free() {
        // The product presentations in the extended styles they actually carry:
        // Sidebar / Floating expanded / Orb are the same undecorated top-level
        // window (Sidebar may carry WS_EX_TOPMOST), Desktop is a `WS_CHILD` of
        // `SHELLDLL_DefView`.
        let top_level = TAO_BASE | APP;
        for ex_style in [
            widget_ex_style(top_level),
            widget_ex_style(top_level | 0x0000_0008),
        ] {
            assert!(!taskbar_eligible_from(ex_style, false, false));
            assert!(!alt_tab_eligible_from(ex_style, false, false));
        }
        let desktop = widget_ex_style(TAO_BASE) | (WS_CHILD.0 as isize);
        assert!(!taskbar_eligible_from(desktop, false, true));
        assert!(!alt_tab_eligible_from(desktop, false, true));
    }
}
