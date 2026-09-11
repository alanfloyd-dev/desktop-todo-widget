//! Window lifecycle glue for the composition host.
//!
//! The product window layer must not need to know which native objects exist or
//! in what order they are released. It reports product-level window events here
//! and this module performs the platform work through the context store.
//!
//! Release ordering is intentionally conservative and mirrors the Phase 7C.0
//! proof:
//!
//! 1. On focus/activation change: update the Acrylic input-active state so the
//!    controller transitions `Active` ↔ `Fallback` without being torn down.
//! 2. On exit request: release the Acrylic material, but keep the shared visual
//!    tree alive so Wry gets the first chance to detach `RootVisualTarget` in
//!    `InnerWebView::drop`. The host is released when the context is dropped.

use super::host::ContextStore;

/// Reports a window activation change for the Acrylic state machine.
pub(crate) fn set_input_active(store: &ContextStore, active: bool) {
    store.set_input_active(active);
}

/// Releases the native material during application exit.
pub(crate) fn shutdown(store: &ContextStore) {
    store.shutdown();
}
