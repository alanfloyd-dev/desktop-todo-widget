# Window modes and Windows desktop integration

## Product modes

**Floating** is an ordinary top-level Tauri window and the first-run default. Its default presentation is a 56 DIP avatar Orb. Clicking expands that same WebView window to its separately saved full Widget size; collapsing restores the independently saved Orb anchor. Lock disables drag/resize but never click, keyboard focus, or context-menu access.

**Sidebar** is also a top-level window. It occupies the monitor work-area height, snaps left or right, and remembers side and width. It does not use WorkerW or `SHELLDLL_DefView`.

**Desktop** is Windows Shell integration and is a supported v1 mode. The same Tauri HWND and WebView2 controller are retained while the bounded, frameless Widget is converted into a child of the interactive desktop host. Its geometry is independent from Floating mode and clamped within the host client area, leaving the real wallpaper, desktop icons, and native desktop surface available outside the Widget. It is draggable and resizable while unlocked, and it never uses always-on-top as a substitute.

Appearance does not alter this parenting route. Desktop clears top-level Acrylic before `WS_CHILD` reparenting and uses a transparent WebView with the documented translucent Graphite fallback; it does not claim unreliable child-window backdrop blur, and native Acrylic is unavailable in this mode by design because a Shell child has no top-level HWND semantics. Switching Orb → Desktop or Desktop → Floating changes presentation through the existing mode state machine without copying the 56 DIP size into Desktop or expanded-Floating geometry.

## Relevant Shell windows

- **Progman** is the legacy Program Manager desktop window and a starting point for Shell discovery.
- **WorkerW** windows are Shell-managed worker surfaces. Their arrangement varies across Windows builds and Explorer restarts.
- **SHELLDLL_DefView** hosts the desktop view and contains the interactive icon view.
- **SysListView32** is the desktop icon ListView. Placing a widget behind it may render correctly while losing pointer and keyboard hit testing.

The adapter asks the Shell to materialize its WorkerW arrangement, finds the current `SHELLDLL_DefView`, and rejects a non-interactive WorkerW-bottom fallback.

## Attach and detach lifecycle

Attach captures the original top-level style and extended style before applying the child-window style, calls `SetParent` with `SHELLDLL_DefView`, positions the child in parent coordinates, applies a frame change without activation, and notifies WebView2 that its parent position changed.

Product Desktop geometry is stored as parent-client `x`, `y`, `width`, and `height` in logical pixels (DIP). First use defaults to approximately 420×700 DIP near the right edge. Entering Desktop multiplies that geometry by the current Tauri window scale factor, clamps the resulting physical rectangle inside the physical Shell host client rect, and only then calls `SetWindowPos`. Leaving Desktop converts the actual physical child rect back to DIP before saving it. Floating geometry is saved and restored separately using the same logical-unit contract.

Floating Orb coordinates are also stored in DIP. At 100/125/150/200% scaling the 56 DIP Orb becomes 56/70/84/112 physical pixels. Expansion chooses left/right and up/down from the Orb center relative to its current monitor work area, clamps the full rectangle, and never rewrites the anchor; repeated expand/collapse therefore cannot drift it.

The product Desktop child removes top-level caption, border, thick-frame, system-menu, and minimize-box styles while retaining `WS_CHILD` and `WS_TABSTOP`. This keeps the Widget frameless without changing the Phase 1 parent/input lifecycle.

Detach has a strict order:

1. Call `SetParent(hwnd, NULL)` and judge success from its return value plus the cleared/captured last error.
2. Restore the captured top-level style and extended style.
3. Apply `SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_NOACTIVATE`.
4. Only then require `GetParent(hwnd) == NULL`.

Immediately after successful `SetParent(NULL)`, Windows may temporarily report desktop root class `#32769` while `WS_CHILD` is still set. Treating that transitional parent as final failure caused the original detach blocker.

Manual transitions and lifecycle recovery share one transition mutex. Recovery rechecks the requested mode and attachment after acquiring it, preventing an older queued recovery from reattaching the window after a successful detach. The WebView is not destroyed or recreated.

## Win+D behavior

A `SHELLDLL_DefView` child is not an independently activatable top-level application window. Win+D changes the foreground and ordinary top-level windows; the widget remains an ambient child of the desktop host. The accepted Phase 1 trace observed the foreground transition while parent, style, bounds, visibility, z-order, and attachment stayed stable. No recovery mutation was necessary.

desktop-todo-widget does not register or intercept the global Win+D chord. Lifecycle observation uses WinEvents and a debounced, conditional validation. It does not repeatedly call `SetParent`.

## Why the desktop host is handled carefully

Progman, WorkerW, and `SHELLDLL_DefView` topology is not a documented public embedding API. Explorer restarts and Windows updates can change handles or hierarchy, and the fallback that places the Widget behind the icon ListView is rejected rather than accepted silently.

Desktop is therefore a supported mode with a guarded implementation rather than experimental. The Phase 1 interaction, detach/reattach, observer, and Win+D gates passed, and the dual-backend work verified Desktop drag/reorder with SQLite persistence proof on this route.

Known limitations and deferred regression work include multi-monitor hot-plug, Explorer restart, lock/sleep/resume, fullscreen applications, DPI changes across displays, and broader Windows-build coverage.

## References and provenance

The implementation uses Microsoft Win32/WebView2 documentation and an in-project adapter. The following projects were reviewed as architecture references; no source was copied wholesale:

- [Microsoft SetParent](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setparent)
- [Microsoft SetWinEventHook](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setwineventhook)
- [Microsoft WebView2 API overview](https://learn.microsoft.com/microsoft-edge/webview2/concepts/overview-features-apis)
- [tauri-plugin-wallpaper](https://github.com/meslzy/tauri-plugin-wallpaper) — MIT
- [tauri-plugin-widgets](https://github.com/s00d/tauri-plugin-widgets) — MIT; reviewed and rejected for this native route
- [RIGStats](https://github.com/dvalfrid/rigstats) — MIT architecture reference
- [dev-hud](https://github.com/soldforaloss/dev-hud) — MIT; WorkerW behavior was roadmap context
- `floating-todo` and `Animated Desktop Wallpapers Helper` were read-only context because a suitable license was not established; no code was copied
