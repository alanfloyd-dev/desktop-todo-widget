# Phase 1 — Windows Desktop Layer PoC (frozen)

Status: **PASS — Phase 2 entry approved on 2026-08-29**

This report freezes the accepted Phase 1 result. Product work must not expand the PoC diagnostics or rewrite the verified attach/detach path without a separately scoped regression need.

## Accepted gates

| Gate | Result |
| --- | --- |
| Desktop attach | Pass |
| Desktop widget hover / click / scroll / keyboard | Pass |
| Desktop icon interaction outside the widget | Pass |
| Desktop → Normal detach | Pass |
| Desktop ↔ Normal round trip on the same HWND | Pass |
| 10-second observer non-invasive control | Pass |
| Win+D foreground transition | Pass |
| Win+D attachment stability | Pass |
| Win+D recovery mutation unnecessary | Pass |
| Multi-monitor hot-plug | Deferred; does not block Phase 2 |

## Verified native route

Desktop mode discovers the Windows Shell through Progman/WorkerW and attaches the existing Tauri HWND as a focusable child of the interactive `SHELLDLL_DefView`. It stays above the icon ListView only inside the widget rectangle. The non-interactive WorkerW-bottom fallback is rejected. The existing WebView2 controller is retained and receives parent-position notification after native changes.

Desktop → Normal calls `SetParent(hwnd, NULL)`. A successful call returns the previous `SHELLDLL_DefView`; while `WS_CHILD` is still present, `GetParent` may transiently report the desktop root `#32769`. That transitional value is diagnostic rather than failure. The adapter restores the captured top-level style/ex-style, applies `SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_NOACTIVATE`, then requires final `Parent = 0x0`.

Manual transitions and Win+D lifecycle recovery use the same transition mutex, preventing a queued recovery from reattaching after detach. The accepted Desktop → Normal → Desktop → Normal sequence retained one HWND and correct parent, styles, visibility, bounds, and interaction without drift or WebView recreation.

## Win+D conclusion

The `SHELLDLL_DefView` widget is a real child, not an independently activatable top-level window. Win+D changes the foreground/top-level application state but does not give the child its own ordinary-window desktop/previous-app toggle semantics. Remaining attached and visible as an ambient desktop widget is the accepted product behavior.

The diagnostic observer was proven non-invasive: after arming and doing nothing for 10 seconds, HWND, parent, style/ex-style, bounds, z-order, visibility, enabled/hit route, and recovery mutation count remained unchanged. The subsequent Win+D trace saw the foreground transition while the widget attachment stayed valid, so no recovery write was needed. No global Win+D hotkey is registered.

## Preserved safeguards

- Event-driven WinEvent/Shell validation with one-shot debounce; no repeated `SetParent` polling.
- Recovery mutates only after a confirmed invalid parent, visibility, bounds, style, or desktop z-order state.
- `SWP_NOACTIVATE` recovery and no Desktop always-on-top disguise.
- No input-blocking click-through styles.
- Diagnostics remain available under **Settings → Developer → Desktop diagnostics**, hidden by default.

## Known risks

WorkerW, Progman, and `SHELLDLL_DefView` structure is undocumented and may change after Windows or Explorer updates. Explorer restart, monitor hot-plug, lock/sleep/resume, fullscreen, and additional Windows-build coverage remain useful regression gates. Multi-monitor hot-plug is explicitly deferred and did not block Phase 2.

## Frozen Phase 1 evidence snapshot

- Windows 11 24H2-generation kernel (`10.0.26100 x86_64`)
- Desktop → Normal → Desktop → Normal passed on one unchanged HWND
- Detach diagnostics showed successful `SetParent` return/last-error handling, transient and final parent, restored styles, visibility, and stable bounds
- Phase 1 Rust tests, Clippy with denied warnings, and frontend build passed before Phase 2 approval

Phase 2 may call the native adapter through a thin product-mode boundary. It must not replace the accepted mechanism with a global shortcut, timer-based reparenting, always-on-top, or WebView destruction/recreation.
