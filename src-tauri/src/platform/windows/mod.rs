//! Windows platform boundary.
//!
//! [`composition_host`] owns the optional CompositionController hosting path,
//! including the Windows App SDK runtime payload contract, the shared
//! `Windows.UI.Composition` visual tree, geometry/DPI, the mouse input bridge,
//! and the Desktop Acrylic material backend.

pub(crate) mod composition_host;
