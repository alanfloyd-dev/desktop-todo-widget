//! Windows composition hosting for the product window.
//!
//! This module is the entire platform boundary for the optional
//! CompositionController hosting path. It replaces Wry's windowed WebView2
//! surface with a WebView2 composition visual hosted in the application's own
//! `Windows.UI.Composition` tree:
//!
//! ```text
//! Win32 HWND
//! └─ Windows.UI.Composition DesktopWindowTarget
//!    └─ ContainerVisual (root)          ← shared by Acrylic and the WebView
//!       └─ ContainerVisual (WebView2 RootVisualTarget)
//! ```
//!
//! Submodules own one concern each:
//!
//! | Module | Concern |
//! |---|---|
//! | [`runtime`] | Windows App Runtime preload and lifetime |
//! | [`host`] | Window context ownership and the process-side store facade |
//! | [`visual`] | DesktopWindowTarget and the shared visual tree |
//! | [`geometry`] | Raw physical client pixels and DPI/rasterization scale |
//! | [`input`] | Mouse/wheel translation into `SendMouseInput` |
//! | [`input_target`] | Keeps the product window reachable for the pointer |
//! | [`material`] | Pure requested/resolved material policy |
//! | [`material_backend`] | DesktopAcrylicController and DWM host backdrop |
//! | [`lifecycle`] | Focus/exit glue and release ordering |
//!
//! ## Boundary rules
//!
//! * Upper layers (product window, settings, commands) must not hold an `HWND`,
//!   a COM interface, or a CompositionController. The only types this module
//!   exports are [`NativeHost`] (a product-level host description),
//!   [`ContextStore`] (an opaque handle), and [`NativeWindowContextStore`]
//!   (the `Arc`-shared alias used by the product runtime).
//! * The default windowed Wry backend is untouched: the host factory returns
//!   `None` unless the opt-in QA flag selected composition hosting.
//! * The vendored Wry patch stays as small as possible: Wry only *requests* a
//!   root visual target, reports resize, and delegates mouse messages here.

pub(crate) mod geometry;
mod host;
pub(crate) mod input;
mod input_target;
mod lifecycle;
mod material;
mod material_backend;
mod runtime;
mod visual;

use std::sync::Arc;

pub(crate) use host::ContextStore;
pub(crate) use lifecycle::{set_input_active, shutdown};

/// Product-level description of which native host the window is in.
///
/// This must stay a product/host concept and must never expose a window handle:
/// the resolver only needs to know which presentation the user is looking at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHost {
    FloatingExpanded,
    FloatingCollapsed,
    Sidebar,
    Desktop,
}

/// Shared handle to the composition host state.
pub(crate) type NativeWindowContextStore = Arc<ContextStore>;

/// Loads the self-contained Windows App Runtime before any window exists.
///
/// Must be called from `run()` when composition hosting is requested; see
/// [`runtime`] for why the timing is load-bearing.
pub(crate) fn preload_windows_app_runtime() -> Result<(), String> {
    runtime::preload()
}

/// Registers the Wry hooks that opt a WebView into composition hosting.
///
/// Wry calls these only for the WebView ids this store answers for. The mouse
/// forwarder is registered here so all input translation stays inside this
/// boundary instead of living in the vendored Wry patch.
pub(crate) fn register_wry_hooks(store: &NativeWindowContextStore) {
    let factory_store = Arc::clone(store);
    wry::set_windows_composition_host_factory(move |id, hwnd| {
        factory_store.prepare_webview(id, hwnd)
    })
    .expect("register Phase 7C.2 Wry composition factory");

    let resize_store = Arc::clone(store);
    wry::set_windows_composition_host_resize_handler(move |id, hwnd, width, height| {
        resize_store.resize_webview(id, hwnd, width, height)
    })
    .expect("register Phase 7C.2 Wry composition resize handler");

    wry::set_windows_composition_mouse_forwarder(
        |controller, composition, hwnd, message, wparam, lparam| {
            if input::is_forwarded_message(message) {
                // SAFETY: Wry passes the live controller of the composition
                // hosted WebView and the parent window the message arrived on.
                unsafe {
                    input::forward_mouse_input(
                        controller,
                        composition,
                        hwnd,
                        message,
                        wparam,
                        lparam,
                    );
                }
            }
        },
    )
    .expect("register Phase 7C.2 Wry composition mouse forwarder");
}

/// Re-export so the product window layer can attach controller settings without
/// naming the store type's internals.
pub(crate) fn attach_controller(
    store: &ContextStore,
    window: &tauri::WebviewWindow,
) -> Result<(), String> {
    store.attach_controller(window)
}

/// Re-synchronises who owns mouse input for the current host.
///
/// In Desktop mode the product window is reparented under `SHELLDLL_DefView`, so
/// it drops below every top-level window — including the Win32 window WebView2
/// keeps for a composition-hosted WebView. Without this call that window wins the
/// hit test and the widget stops receiving pointer input entirely; see
/// [`input_target`].
pub(crate) fn sync_input_target(store: &ContextStore, desktop: bool) -> Result<(), String> {
    store.sync_input_target(desktop)
}

pub(crate) fn apply_material(
    store: &ContextStore,
    window: &tauri::WebviewWindow,
    requested_glass: bool,
    host: NativeHost,
) -> Result<(), String> {
    store.apply_material(window, requested_glass, host)
}

pub(crate) fn diagnostic_summary(store: &ContextStore) -> String {
    store.diagnostic_summary()
}
