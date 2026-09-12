//! Windows platform boundary.
//!
//! [`composition_host`] owns the optional CompositionController hosting path,
//! including the Windows App SDK runtime payload contract, the shared
//! `Windows.UI.Composition` visual tree, geometry/DPI, the mouse input bridge,
//! and the Desktop Acrylic material backend.
//!
//! [`widget_frame`] owns the window's *identity* as a widget rather than an
//! application: the private extended style that keeps every product window mode
//! out of the taskbar and out of Alt+Tab.

pub(crate) mod composition_host;
pub(crate) mod widget_frame;
