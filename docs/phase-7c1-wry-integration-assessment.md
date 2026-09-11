# Phase 7C.1 — Tauri/Wry CompositionController integration assessment

Date: 2026-09-11
Scope: read-only assessment of the production hosting path. No migration was implemented and no production dependency was changed.

## Decision

**B: small maintainable Windows-only patch/fork**

The locked Wry implementation has no public or internal controller-creation hook that an application can use to select `CreateCoreWebView2CompositionController`. A wrapper cannot replace the controller after Wry has already created a windowed controller. A Windows-only fork/patch is therefore required.

The patch does not need to replace Wry's navigation, custom protocol, IPC, WebView settings, event handlers, cookies, devtools, or `ICoreWebView2` API implementation. A composition controller also exposes `ICoreWebView2Controller`, so most of the existing controller-facing code remains reusable. The changed surface can stay localized to controller construction, visual-host handoff, spatial input, cursor/capture, and Windows lifecycle plumbing.

This is not conclusion A because no pre-creation hook exists. It is not conclusion C because the relevant Windows code is concentrated in one Wry backend and the non-Windows implementations are selected with compile-time platform gates.

## Evaluated versions

The assessment is for the versions locked in `src-tauri/Cargo.lock`:

- `tauri 2.11.5`
- `tauri-runtime 2.11.3`
- `tauri-runtime-wry 2.11.4`
- `wry 0.55.1`
- `webview2-com 0.38.2`

The exact Wry sources used below are the [`wry-v0.55.1` Windows backend](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/webview2/mod.rs) and its [Windows extension API](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs). Tauri's relevant adapter is [`tauri-runtime-wry`](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-runtime-wry/src/lib.rs).

## Phase 7C.0 reference result

Phase 7C.0 is now a real interactive-desktop **visual PASS**:

- Desktop Acrylic was visible and transitioned between active/fallback states correctly.
- `ICoreWebView2CompositionController` content was visible concurrently with Acrylic.
- `RootVisualTarget` round-tripped successfully.
- Pointer input reached the page.
- At 150% DPI, the HWND client rect, root `ContainerVisual`, WebView host visual, and raw-pixel controller bounds agreed; rasterization scale was 1.5.
- Attach, detach, and shutdown lifecycles worked.

The implementation under `tools/native-composition-webview-poc/` is retained unchanged as the reference implementation for the required Windows.UI.Composition tree, geometry units, input forwarding, and teardown order. In particular, the supported visual namespace is `Windows.UI.Composition`, not `Microsoft.UI.Composition`; Microsoft documents that `CreateCoreWebView2CompositionController` does not accept WinAppSDK `Microsoft.UI.Composition` visuals.

## 1. Current Tauri → Wry → WebView2 call path

For the configured `main` WebviewWindow, the complete path is:

1. `tauri::Builder::build(generate_context!())` loads `tauri.conf.json`.
2. Tauri creates each configured window through `WebviewWindowBuilder::from_config(...).build()`.
3. `WebviewWindowBuilder::build` combines a `WindowBuilder` and `WebviewBuilder`, producing a runtime `PendingWindow` containing a `PendingWebview`.
4. `tauri-runtime-wry::Context::create_window` dispatches the create operation to the Tao event-loop thread.
5. `tauri-runtime-wry::create_window` creates the outer Tao Win32 HWND. Its internal `after_window_creation` callback runs after the HWND exists and before the WebView is created.
6. `tauri-runtime-wry::create_webview` maps Tauri `WebviewAttributes` onto `wry::WebViewBuilder::new_with_web_context(...)`. URL, transparency, initialization scripts, custom protocols, IPC, drag/drop, theme, browser arguments, and optionally a supplied environment are applied here.
7. For the normal configured window (`WebviewKind::WindowContent`), the adapter calls `webview_builder.build(&window)`.
8. Wry calls `webview2::InnerWebView::new`, then `new_in_hwnd(parent, ..., is_child = false)`.
9. Wry creates a child `WRY_WEBVIEW` container HWND under the Tao HWND. It sizes that child to the parent client rect.
10. Wry either reuses `PlatformSpecificWebViewAttributes.environment` or calls `create_environment`, which uses `CreateCoreWebView2EnvironmentWithOptions` and waits for the callback with `webview2_com::wait_with_pump`.
11. Wry calls its private `create_controller` with the `WRY_WEBVIEW` HWND.
12. `init_webview` gets `controller.CoreWebView2()`, configures settings and handlers, installs Tauri/Wry scripts and protocols, navigates, attaches a parent-HWND subclass for resize/focus/move, sets visibility, and optionally calls `MoveFocus`.
13. `resize_to_parent` applies the initial client size to both controller bounds and the `WRY_WEBVIEW` child HWND.

The important pre-controller seam is the internal `after_window_creation` callback: it proves the runtime already has a point where the Tao HWND exists but Wry has not yet created its controller. It is not currently exposed as an application-level composition-host hook.

## 2. Exact ordinary-controller creation point

The concrete call site is private function:

```text
wry 0.55.1
src/webview2/mod.rs
InnerWebView::create_controller
```

Its current behavior is:

```rust
if let Ok(env10) = env.cast::<ICoreWebView2Environment10>() {
    let controller_opts = env10.CreateCoreWebView2ControllerOptions()?;
    // default background and in-private options
    env10.CreateCoreWebView2ControllerWithOptions(hwnd, &controller_opts, &handler)?;
} else {
    env.CreateCoreWebView2Controller(hwnd, &handler)?;
}
```

Thus the normal path on a current runtime is usually `CreateCoreWebView2ControllerWithOptions`, not the fallback call. A correct patch must replace both branches with their composition equivalents:

- `ICoreWebView2Environment10::CreateCoreWebView2CompositionControllerWithOptions`; and
- fallback `ICoreWebView2Environment3::CreateCoreWebView2CompositionController`.

This preserves private-mode and controller-option behavior instead of silently regressing it.

## 3. Available hooks

### Public hooks that exist

- Tauri/Wry can inject an `ICoreWebView2Environment` through `with_environment`.
- Tauri `with_webview` and Wry `WebViewExtWindows::{controller, environment, webview}` expose native objects after creation.
- Normal runtime APIs can resize, focus, show/hide, navigate, evaluate scripts, and retrieve the base controller.

### Why none is sufficient

- Environment injection does not choose which environment factory method Wry invokes.
- `with_webview` runs after the ordinary controller and its HWND-hosted WebView have been created. A windowed controller cannot be converted into a composition controller.
- The controller factory and `PlatformSpecificWebViewAttributes` are private implementation details.
- Searching Wry 0.55.1 finds no call to `CreateCoreWebView2CompositionController`, no `RootVisualTarget` option, and no composition hosting mode.
- Tauri's `after_window_creation` callback is an internal runtime seam, not an application callback for supplying a visual tree before WebView creation.

Therefore there is no upstream/public hook sufficient for conclusion A.

## 4. Current ownership of hosting responsibilities

| Responsibility | Current owner in windowed hosting |
| --- | --- |
| Top-level HWND and event loop | Tao through `tauri-runtime-wry` |
| WebView container HWND | Wry (`WRY_WEBVIEW`, child of the Tao HWND) |
| Browser/render HWNDs | WebView2 windowed controller/runtime |
| Environment and controller construction | Wry Windows backend |
| Initial and subsequent bounds | Wry; `resize_to_parent`, `set_bounds_inner`, and the parent subclass's `WM_SIZE` handler |
| Position notification | Wry calls `NotifyParentWindowPositionChanged` for `WM_MOVE`/`WM_MOVING` |
| DPI awareness and top-level DPI events | Tao/Windows; Wry queries the HWND DPI and converts logical builder bounds to physical pixels |
| WebView rasterization/window scaling | WebView2/OS in windowed mode |
| Focus entry | Tao/Windows plus Wry `WM_SETFOCUS` handling and `ICoreWebView2Controller::MoveFocus` |
| Mouse, wheel, touch, pen | OS routes input to WebView2's HWND |
| Keyboard and normal tab traversal | OS/WebView2, with Wry initiating focus |
| IME | OS/WebView2 windowed HWND path |
| Cursor | WebView2/OS windowed HWND path |
| Accessibility | WebView2 HWND appears in the native accessibility tree |
| File drag/drop | WebView2 by default, or Wry's Windows drag/drop controller when a custom handler is enabled |
| Navigation, custom protocols, IPC, scripts, WebView events | Wry/Tauri; independent of output hosting mode |

## 5. Responsibilities that change under CompositionController

Microsoft describes visual hosting as retaining the base-controller APIs for bounds, visibility, focus, and lifecycle, while the host connects a visual tree and supplies spatial input. The required ownership becomes:

### Must be implemented by the Windows host patch

- Create and retain the `Windows.UI.Composition` target/root/WebView child visual and set `RootVisualTarget`.
- Keep root visual size, WebView visual size, base-controller bounds, DPI/rasterization scale, and visual transforms synchronized.
- Forward the full mouse family through `SendMouseInput`: move, leave, all buttons, double-click, vertical/horizontal wheel, X buttons, modifier/button virtual-key flags, and client-coordinate conversion.
- Implement `TrackMouseEvent`, `SetCapture`, `ReleaseCapture`, and outside-bounds drag behavior.
- Subscribe to `CursorChanged` and apply `Cursor`/`SystemCursorId` in the parent HWND cursor path.
- Forward touch and pen through `SendPointerInput`, including construction/update of `ICoreWebView2PointerInfo`.
- If Tauri custom file-drop behavior remains enabled, bridge drag enter/over/leave/drop through `ICoreWebView2CompositionController3` or explicitly define the unsupported boundary.
- Disconnect `RootVisualTarget`, remove input/cursor subscriptions and HWND subclasses, close the controller, and then release the visual tree in deterministic order.

### Existing code that remains reusable but needs composition-aware routing

- `ICoreWebView2Controller::SetBounds`, `SetIsVisible`, `MoveFocus`, `NotifyParentWindowPositionChanged`, zoom, and `Close` remain the base lifecycle interface.
- Navigation, IPC, custom schemes, permissions, settings, downloads, cookies, devtools, initialization scripts, and page event handlers operate on `ICoreWebView2` and can remain unchanged.
- Keyboard has no equivalent `SendKeyboardInput` API on `ICoreWebView2CompositionController`. Focus must remain tied to the parent HWND and `MoveFocus`; accelerator and Tab traversal behavior needs regression tests around the existing Tao/Tauri message loop.
- IME remains coupled to WebView2's focused parent-window integration rather than a public composition input-forwarding API. It is a high-risk validation item: composition-host IME regressions have existed upstream, so CJK candidate placement, composition, reconversion, and focus transitions must be acceptance gates.
- WebView2 is documented to appear under the parent HWND in the accessibility tree by default. For correct custom visual ordering/positioning, the patch should validate this behavior and be prepared to expose `ICoreWebView2CompositionController2::AutomationProvider` (`IRawElementProviderSimple`) through the parent accessibility provider. This is a validation/adapter responsibility, not a reason to replace Tauri accessibility globally.

The official hosting comparison explicitly states that visual hosting requires the application to receive and forward spatial input, whereas windowed hosting gets input and accessibility behavior from the OS. The [`ICoreWebView2CompositionController` contract](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller?view=webview2-1.0.3800.47) also requires mouse-leave tracking, coordinate conversion, capture behavior, and cursor handling. Accessibility support is exposed by [`ICoreWebView2CompositionController2::AutomationProvider`](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller2?view=webview2-1.0.3967.48).

## 6. Wrapper/extension feasibility without a Wry fork

**No, not for replacing the controller while retaining Tauri's WebView.**

An application-side wrapper can inspect or configure the already-created base controller, but controller type is fixed at creation. Creating a second composition controller beside Tauri's existing WebView would produce a second WebView instance and would not inherit Tauri's private protocol/IPC/script setup. Closing the original controller afterward would leave Tauri/Wry holding stale COM objects and HWND assumptions.

Implementing a complete custom Tauri `Runtime` or independently recreating all Tauri WebView initialization would technically avoid a Wry fork, but it is substantially more invasive and less maintainable than the localized patch. It is not recommended.

## 7. Minimum fork/patch boundary

### Wry patch

Minimum files:

1. `wry/src/lib.rs`
   - Add a Windows-only composition hosting option to `PlatformSpecificWebViewAttributes` and `WebViewBuilderExtWindows`.
   - Keep the default as windowed hosting so all existing callers retain current behavior.
2. `wry/src/webview2/mod.rs`
   - Split controller creation into windowed/composition branches.
   - Return the base `ICoreWebView2Controller` plus an optional retained `ICoreWebView2CompositionController`.
   - Distinguish the Tao host HWND, any Wry helper/message HWND, and the controller parent HWND instead of assuming one `WRY_WEBVIEW` output container.
   - Set/clear `RootVisualTarget`, reuse existing base-controller setup, and add composition spatial-input/cursor/lifecycle handling to the existing parent subclass.
   - Make `bounds`, visibility, focus, move notification, reparent, and drop correct for both hosting modes.
3. Prefer a new Windows-only `wry/src/webview2/composition.rs` for input/cursor/capture/visual-host state. This is organizational, not an additional public API requirement.
4. Add a Windows test/example covering windowed default behavior and composition behavior. The Phase 7C.0 PoC remains the geometry/lifecycle oracle.

### Tauri plumbing patch

Because Tauri constructs the configured main WebView before application `setup`, the application must be able to register a composition-host factory before configured windows are created. The internal `after_window_creation` position is the correct execution point: the Tao HWND exists, execution is on its owner thread, and Wry has not created a controller yet.

Minimum likely files/API surface:

1. `tauri-runtime/src/lib.rs` and/or `tauri-runtime/src/webview.rs`
   - Add an opaque, Windows-only pre-WebView host descriptor/factory to the pending window/webview contract.
2. `tauri-runtime-wry/src/lib.rs`
   - Execute that factory in the existing post-HWND/pre-WebView seam and pass the resulting RootVisualTarget/lifetime object into Wry's Windows builder option.
3. `tauri/src/app.rs` plus the narrow builder module that owns WebView attributes (`tauri/src/webview/mod.rs` and, if needed for dynamic windows, `webview_window.rs`)
   - Expose one Windows-only opt-in registration API. No cross-platform enum variant should alter default construction.

Suggested conceptual API, not an implementation commitment:

```rust
#[cfg(windows)]
Builder::on_windows_webview_host_created(
    |label, hwnd| -> Result<WindowsCompositionWebViewHost>
)
```

The returned opaque host should contain or retain:

- the `Windows.UI.Composition::ContainerVisual` used as `RootVisualTarget`;
- the root/target/compositor lifetime needed by Acrylic and WebView;
- geometry synchronization access on the HWND owner thread; and
- teardown ownership.

Avoid a global registry, raw borrowed pointer, environment-variable switch, or callback that runs after `with_webview`; those designs obscure COM/thread ownership and make teardown unsafe.

### Estimated maintenance cost

These are engineering estimates, not measured implementation results:

- Initial patch with mouse, wheel, capture, cursor, focus, geometry, teardown, and Tauri plumbing: roughly 5–10 engineering days after the PoC, including integration tests.
- Touch/pen, drag/drop parity, IME matrix, accessibility verification, multi-monitor DPI, reparent/mode transitions, and multi-window hardening: another 5–10 engineering days.
- Expected code size: approximately 500–1,000 Windows-specific lines plus focused tests; most should live in a new composition module rather than complicating the windowed path.
- Upgrade maintenance: usually 0.5–2 days per relevant Wry/Tauri update, with a larger rebase when `src/webview2/mod.rs` or Tauri pending-window construction changes.
- Required CI/manual matrix: x64 plus ARM64 if supported; 100/125/150/200% DPI; mouse/wheel/touch/pen where available; CJK IME; keyboard/Tab; screen reader smoke test; active/fallback Acrylic; resize/move/minimize/restore; Floating/Desktop transitions; and shutdown/recreate.

This cost is meaningful but bounded. It remains a maintainable platform patch as long as the acceptance scope explicitly includes the input/IME/accessibility tests above. If implementation expands into a custom Tauri runtime or duplicates Tauri's IPC/protocol/navigation layer, the decision must be reclassified to C and stopped.

## 8. macOS/Linux isolation

Yes. macOS and Linux can remain completely unchanged if all new types, fields, builder methods, and branches are guarded with `#[cfg(windows)]`, and windowed hosting remains the default.

The platform call sites already diverge inside Wry and `tauri-runtime-wry`:

- Windows uses `wry::webview2` and WebView2 COM.
- macOS uses WKWebView.
- Linux/BSD uses WebKitGTK.

The patch must not add a mandatory cross-platform trait method unless it has a no-op/default implementation. A Windows-only extension trait/opaque field is preferable. Cross-platform build CI should still be required to prove that feature and dependency gating did not drift.

## Recommended next boundary

Do not begin product migration directly. The next safe artifact is a minimal fork spike against the exact locked versions that does only the following:

1. exposes the pre-WebView Windows host factory;
2. selects composition controller creation;
3. reuses all existing Wry `ICoreWebView2` initialization;
4. renders the existing Tauri application content;
5. implements mouse/wheel/cursor/focus and deterministic shutdown; and
6. runs explicit IME/accessibility/keyboard gates before any product architecture is committed.

No database, frontend, or product window-mode change is required to answer that spike. If the spike cannot preserve Wry's existing IPC/protocol/navigation behavior without duplicating it, stop and reclassify the route as C.

## Sources

- [Wry 0.55.1 Windows WebView2 backend](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/webview2/mod.rs)
- [Wry 0.55.1 public Windows extensions](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs)
- [Tauri runtime-wry adapter at the locked Tauri tag](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-runtime-wry/src/lib.rs)
- [Tauri WebView surface at the locked Tauri tag](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs)
- [Microsoft: Windowed vs. Visual hosting](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/windowed-vs-visual-hosting)
- [Microsoft: ICoreWebView2CompositionController](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller?view=webview2-1.0.3800.47)
- [Microsoft: ICoreWebView2CompositionController2](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller2?view=webview2-1.0.3967.48)
- [Microsoft: Overview of WebView2 APIs](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/overview-features-apis)
- [Microsoft WebView2Samples](https://github.com/MicrosoftEdge/WebView2Samples)
