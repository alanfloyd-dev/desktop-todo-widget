# desktop-todo-widget v1.1.0

Rendering architecture simplification and stability release.

## Highlights

- **Standard-only rendering architecture.** The widget now has a single windowed WebView2 backend. The experimental **Enhanced** backend (composition-hosted WebView2 with Native Acrylic/HostBackdrop) has been retired. It served one visual effect at the cost of a custom composition host, an accessibility trade-off, and heavy Windows environment sensitivity.
- **No more Windows App SDK runtime.** The self-contained runtime payload is gone from the build, the install directory, and the installer. The app needs only the WebView2 Runtime that is already part of Windows.
- **Upstream Wry restored.** The vendored, patched Wry copy was removed; the build uses the stock crates.io release again.
- **Leaner installs.** A fresh install places the executable (and optional documentation) only. Installs upgraded from v1.0.1 may keep the old runtime files until uninstall; the uninstaller recognizes and removes them.
- **Automatic settings migration.** Profiles saved with the Enhanced backend load normally, run on Standard, and are rewritten to the Standard value on first launch. No action is required, and nothing else in your profile changes.
- **Consistent product surface.** Mode names no longer carry material annotations (Sidebar / Floating / Desktop), the Settings gear in the expanded footer now sits next to your profile identity, and Glass, Solid, Gradient, Image, and Wallpaper remain the background options. All modes render their materials through the same CSS layers over a transparent window.
- **Clearer contribution boundaries.** The Windows native layer is documented as frozen-by-default, and the bilingual contribution guide now covers backend policy, testing expectations, repository hygiene, and AI-assisted work.

The net effect is reduced maintenance surface and fewer environment dependencies — this release does not claim performance changes.

## Compatibility

- Windows 11 is currently validated. Windows 10 compatibility is not yet formally validated.
- WebView2 Runtime is required.
- Desktop uses the documented translucent Graphite material instead of Glass; window backdrop effects need top-level HWND semantics and Desktop is hosted as a Shell child.

## Data and privacy

- Everything stays local: no account, no sync, and no intentional upload. Weather is the only remote call, and only for a location you configure.
- The install scripts need no administrator privileges and do not modify system-wide settings.
- Uninstalling preserves your data unless you explicitly ask for it to be removed.
