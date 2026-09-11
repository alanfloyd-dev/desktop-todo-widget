//! Geometry and DPI policy for the composition host.
//!
//! The composition host always works in **raw physical client pixels**: the
//! WebView2 controller runs in
//! `COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS`, the root visual and the WebView
//! visual are sized to the Win32 client rect, and `RasterizationScale` is the
//! window DPI divided by 96. Nothing here converts between logical and physical
//! units — that conversion belongs to the product window layer.

use windows::{
    core::Result,
    Win32::{
        Foundation::{HWND, RECT},
        UI::HiDpi::GetDpiForWindow,
        UI::WindowsAndMessaging::GetClientRect,
    },
};
use windows_numerics::{Vector2, Vector3};

/// Physical size of a window's client area.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClientSize {
    pub(crate) width: i32,
    pub(crate) height: i32,
}

impl ClientSize {
    /// Reads the client rect of `hwnd` in physical pixels, clamped at zero.
    pub(crate) fn of(hwnd: HWND) -> Result<Self> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect)? };
        Ok(Self {
            width: (rect.right - rect.left).max(0),
            height: (rect.bottom - rect.top).max(0),
        })
    }

    pub(crate) fn as_vector2(self) -> Vector2 {
        Vector2 {
            X: self.width as f32,
            Y: self.height as f32,
        }
    }
}

/// Window DPI and the matching WebView2 rasterization scale (`dpi / 96`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DpiScale {
    pub(crate) dpi: u32,
    pub(crate) scale: f64,
}

impl DpiScale {
    pub(crate) fn of(hwnd: HWND) -> Self {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        Self {
            dpi,
            scale: dpi as f64 / 96.0,
        }
    }
}

/// Places a visual at the origin and sizes it to the client area.
pub(crate) fn apply_client_size(
    visual: &windows::UI::Composition::ContainerVisual,
    size: ClientSize,
) -> Result<()> {
    let zero = Vector3::default();
    visual.SetOffset(zero)?;
    visual.SetSize(size.as_vector2())?;
    Ok(())
}
