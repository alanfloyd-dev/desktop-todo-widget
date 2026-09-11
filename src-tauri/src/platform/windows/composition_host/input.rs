//! Mouse/wheel input bridge for composition-hosted WebView2.
//!
//! Windowed WebView2 owns a real child HWND, so Windows delivers input to it
//! directly. A composition-hosted WebView has no child window: the application's
//! parent subclass receives the window messages and has to translate them into
//! `ICoreWebView2CompositionController::SendMouseInput`.
//!
//! Keeping the translation here means the vendored Wry patch only has to detect
//! the messages and delegate, and the coordinate/flag handling stays inside the
//! platform boundary where it can be reviewed and unit-tested.
//!
//! Keyboard and IME do **not** need an equivalent bridge: the base
//! `ICoreWebView2Controller` retains the normal focus path, confirmed by human QA
//! with a real Chinese IME.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2CompositionController, ICoreWebView2Controller,
    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
    COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::{
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        WindowsAndMessaging::{
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
            WM_RBUTTONUP,
        },
    },
};

/// True when `message` is a mouse message this bridge forwards.
pub(crate) fn is_forwarded_message(message: u32) -> bool {
    matches!(
        message,
        WM_MOUSEMOVE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MOUSEWHEEL
    )
}

/// Translates one Win32 mouse message and sends it to the WebView2 composition
/// controller.
///
/// `wparam`/`lparam` are passed through unchanged from the window procedure.
///
/// # Safety
/// `hwnd` must be the live parent window the controller is hosted in, and the
/// controller must belong to a live composition-hosted WebView.
pub(crate) unsafe fn forward_mouse_input(
    controller: &ICoreWebView2Controller,
    composition: &ICoreWebView2CompositionController,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) {
    let kind = match message {
        WM_LBUTTONDOWN => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
        WM_LBUTTONUP => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
        WM_RBUTTONDOWN => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
        WM_RBUTTONUP => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
        WM_MOUSEWHEEL => COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
        _ => COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
    };

    let mut point = client_point_from_lparam(lparam);
    if message == WM_MOUSEWHEEL {
        // Wheel messages carry screen coordinates.
        let _ = ScreenToClient(hwnd, &mut point);
    }

    let is_button_down = message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN;
    if is_button_down {
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        let _ = SetCapture(hwnd);
    }

    let mouse_data = if message == WM_MOUSEWHEEL {
        wheel_delta_from_wparam(wparam)
    } else {
        0
    };
    let keys = COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS((wparam.0 & 0xffff) as i32);
    let _ = composition.SendMouseInput(kind, keys, mouse_data, point);

    if message == WM_LBUTTONUP || message == WM_RBUTTONUP {
        let _ = ReleaseCapture();
    }
}

/// Decodes the signed client position packed into a mouse message `lparam`.
fn client_point_from_lparam(lparam: LPARAM) -> POINT {
    let packed = lparam.0 as u32;
    POINT {
        x: (packed & 0xffff) as u16 as i16 as i32,
        y: ((packed >> 16) & 0xffff) as u16 as i16 as i32,
    }
}

/// Extracts the signed wheel delta packed into the high word of `wparam`.
fn wheel_delta_from_wparam(wparam: WPARAM) -> u32 {
    (((wparam.0 >> 16) as u16) as i16 as i32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_positive_client_coordinates() {
        let point = client_point_from_lparam(LPARAM(((120i32) << 16 | 40i32) as isize as _));
        assert_eq!((point.x, point.y), (40, 120));
    }

    #[test]
    fn decodes_negative_client_coordinates() {
        // -5 and -9 as signed 16-bit halves.
        let packed = ((0xfff7u32) << 16) | 0xfffbu32;
        let point = client_point_from_lparam(LPARAM(packed as isize as _));
        assert_eq!((point.x, point.y), (-5, -9));
    }

    #[test]
    fn decodes_wheel_delta_for_both_directions() {
        let up = WPARAM((120u16 as usize) << 16);
        let down = WPARAM((0xff88u16 as usize) << 16);
        assert_eq!(wheel_delta_from_wparam(up), 120);
        assert_eq!(wheel_delta_from_wparam(down) as i32, -120);
    }

    #[test]
    fn only_mouse_messages_are_forwarded() {
        for message in [
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_LBUTTONUP,
            WM_RBUTTONDOWN,
            WM_RBUTTONUP,
            WM_MOUSEWHEEL,
        ] {
            assert!(is_forwarded_message(message), "0x{message:04X} should forward");
        }
        // Real non-mouse messages: WM_NULL, WM_PAINT, WM_KEYDOWN, WM_COMMAND.
        for message in [0x0000_u32, 0x000F_u32, 0x0100_u32, 0x0111_u32] {
            assert!(
                !is_forwarded_message(message),
                "0x{message:04X} must not forward"
            );
        }
    }
}
