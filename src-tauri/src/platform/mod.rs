//! Platform-specific implementation boundary.
//!
//! Everything that touches a native window handle, a COM interface, or an
//! OS-specific composition API lives under this module. Code outside `platform`
//! must not import `windows`, `windows-core`, `webview2-com`, `windows-numerics`,
//! or any raw handle type.
//!
//! Non-Windows targets compile none of this: the corresponding product code
//! degrades to no-ops instead of pulling in a Windows dependency.

#[cfg(target_os = "windows")]
pub(crate) mod windows;
