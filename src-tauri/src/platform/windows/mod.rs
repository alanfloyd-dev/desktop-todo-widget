//! Windows platform boundary.
//!
//! [`widget_frame`] owns the window's *identity* as a widget rather than an
//! application: the private extended style that keeps every product window mode
//! out of the taskbar and out of Alt+Tab.

pub(crate) mod widget_frame;
