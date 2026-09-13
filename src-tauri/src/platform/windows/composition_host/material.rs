//! Pure material resolution policy for the native backdrop.
//!
//! This module holds no Windows objects and no window handles: given the
//! requested product material, the resolved host, and the platform capability
//! snapshot it returns exactly one resolved backend. It is unit-tested as a pure
//! function so the policy can be reviewed without running a compositor.

use super::NativeHost;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestedMaterial {
    Glass,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedMaterial {
    DesktopAcrylic,
    TransparentOrb,
    ExistingProductMaterial,
    GlassFallback,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlatformCapabilities {
    pub(crate) desktop_acrylic_supported: bool,
}

pub(crate) fn resolve(
    requested: RequestedMaterial,
    host: NativeHost,
    capabilities: PlatformCapabilities,
) -> ResolvedMaterial {
    // DesktopAcrylic is reachable only for Floating-expanded. Desktop falls
    // through to `ExistingProductMaterial` on purpose: that mode reparents the
    // product HWND as a child of the desktop shell, while the controller builds a
    // DesktopWindowTarget that depends on top-level HWND semantics. Desktop keeps
    // its translucent CSS fallback instead.
    match (requested, host, capabilities.desktop_acrylic_supported) {
        (RequestedMaterial::Glass, NativeHost::FloatingExpanded, true) => {
            ResolvedMaterial::DesktopAcrylic
        }
        (RequestedMaterial::Glass, NativeHost::FloatingExpanded, false) => {
            ResolvedMaterial::GlassFallback
        }
        (RequestedMaterial::Glass, NativeHost::FloatingCollapsed, _) => {
            ResolvedMaterial::TransparentOrb
        }
        _ => ResolvedMaterial::ExistingProductMaterial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: PlatformCapabilities = PlatformCapabilities {
        desktop_acrylic_supported: true,
    };
    const UNSUPPORTED: PlatformCapabilities = PlatformCapabilities {
        desktop_acrylic_supported: false,
    };

    #[test]
    fn resolver_only_enables_acrylic_for_supported_floating_expanded_glass() {
        assert_eq!(
            resolve(
                RequestedMaterial::Glass,
                NativeHost::FloatingExpanded,
                SUPPORTED
            ),
            ResolvedMaterial::DesktopAcrylic
        );
        assert_eq!(
            resolve(
                RequestedMaterial::Glass,
                NativeHost::FloatingCollapsed,
                SUPPORTED
            ),
            ResolvedMaterial::TransparentOrb
        );
        assert_eq!(
            resolve(RequestedMaterial::Glass, NativeHost::Sidebar, SUPPORTED),
            ResolvedMaterial::ExistingProductMaterial
        );
        assert_eq!(
            resolve(RequestedMaterial::Glass, NativeHost::Desktop, SUPPORTED),
            ResolvedMaterial::ExistingProductMaterial
        );
        assert_eq!(
            resolve(
                RequestedMaterial::Other,
                NativeHost::FloatingExpanded,
                SUPPORTED
            ),
            ResolvedMaterial::ExistingProductMaterial
        );
    }

    #[test]
    fn resolver_reports_fallback_when_desktop_acrylic_is_unsupported() {
        assert_eq!(
            resolve(
                RequestedMaterial::Glass,
                NativeHost::FloatingExpanded,
                UNSUPPORTED
            ),
            ResolvedMaterial::GlassFallback
        );
    }

    /// The stager/runtime contract: an expanded floating Glass window is the only
    /// host that may create a DesktopAcrylicController.
    #[test]
    fn only_floating_expanded_reaches_the_acrylic_backend() {
        for host in [
            NativeHost::Sidebar,
            NativeHost::Desktop,
            NativeHost::FloatingCollapsed,
        ] {
            assert_ne!(
                resolve(RequestedMaterial::Glass, host, SUPPORTED),
                ResolvedMaterial::DesktopAcrylic
            );
        }
    }
}
