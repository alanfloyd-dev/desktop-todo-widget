//! The shared Windows.UI.Composition visual tree.
//!
//! One `DesktopWindowTarget` carries both the Acrylic target and the WebView2
//! composition visual, so the WebView no longer occludes the transparent
//! background with an opaque child surface.
//!
//! ```text
//! Win32 HWND
//! └─ DesktopWindowTarget
//!    └─ ContainerVisual (root)
//!       └─ ContainerVisual (WebView2 RootVisualTarget)
//! ```
//!
//! Acrylic attaches to the same `CompositionTarget` (see [`super::material_backend`]).

use windows::{
    core::{IUnknown, Interface, Result},
    UI::Composition::{CompositionTarget, Compositor, ContainerVisual, Desktop::DesktopWindowTarget},
    Win32::{
        Foundation::HWND,
        System::WinRT::Composition::ICompositorDesktopInterop,
    },
};

use std::ffi::c_void;

use super::geometry::{apply_client_size, ClientSize};

/// Owns the composition objects for one window: compositor, target, and both
/// visuals. The WebView2 visual is handed out as the controller's
/// `RootVisualTarget`.
pub(crate) struct ComVisual {
    /// Stored as a raw value, not `HWND`, so this type stays `Send`: the Tauri
    /// managed-state store requires it.
    hwnd: usize,
    compositor: Option<Compositor>,
    target: Option<DesktopWindowTarget>,
    composition_target: CompositionTarget,
    root: ContainerVisual,
    webview: ContainerVisual,
    released: bool,
}

impl ComVisual {
    /// Creates the target and visual tree for `hwnd`, sized to its client area.
    ///
    /// # Safety
    /// `hwnd` must be a live top-level window owned by the current thread, and
    /// the thread must already have a DispatcherQueue and WinRT apartment.
    pub(crate) unsafe fn new(hwnd: HWND) -> Result<Self> {
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        // `top_level = true`: the target belongs to the top-level product window.
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        let root = compositor.CreateContainerVisual()?;
        let webview = compositor.CreateContainerVisual()?;
        root.Children()?.InsertAtTop(&webview)?;
        target.SetRoot(&root)?;
        let composition_target: CompositionTarget = target.cast()?;

        let mut visual = Self {
            hwnd: hwnd.0 as usize,
            compositor: Some(compositor),
            target: Some(target),
            composition_target,
            root,
            webview,
            released: false,
        };
        visual.resize(ClientSize::of(hwnd)?)?;
        eprintln!(
            "[phase7c2] desktop_window_target_created=true root_visual=ContainerVisual webview_visual=ContainerVisual"
        );
        Ok(visual)
    }

    /// The object handed to `ICoreWebView2CompositionController::SetRootVisualTarget`.
    pub(crate) fn webview_root_target(&self) -> Result<IUnknown> {
        self.webview.cast()
    }

    /// The composition target Acrylic attaches to.
    pub(crate) fn composition_target(&self) -> CompositionTarget {
        self.composition_target.clone()
    }

    /// Handle value of the window this tree belongs to.
    pub(crate) fn hwnd(&self) -> usize {
        self.hwnd
    }

    /// Native handle for Win32 calls.
    pub(crate) fn native_hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }

    /// Resizes both visuals to `size` and reports the geometry decision.
    pub(crate) fn resize(&mut self, size: ClientSize) -> Result<()> {
        apply_client_size(&self.root, size)?;
        apply_client_size(&self.webview, size)?;
        let scale = super::geometry::DpiScale::of(self.native_hwnd());
        eprintln!(
            "[phase7c2] geometry client_physical_px={}x{} dpi={} scale={:.4} root_size={}x{} webview_visual_size={}x{} bounds={}x{}",
            size.width,
            size.height,
            scale.dpi,
            scale.scale,
            size.width,
            size.height,
            size.width,
            size.height,
            size.width,
            size.height
        );
        Ok(())
    }

    /// Releases the target and compositor exactly once.
    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        self.target.take();
        self.compositor.take();
        self.released = true;
        eprintln!("[phase7c2] lifecycle=composition-host-released");
    }
}

impl Drop for ComVisual {
    fn drop(&mut self) {
        self.release();
    }
}
