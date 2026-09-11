# Phase 7C.0 result

## Status

`INCONCLUSIVE` for the visual PASS gate. Controller creation and visual
attachment succeed, but the available automated execution environment denies
the WebView2 GPU process before pixels can be validated.

This is not evidence that Acrylic and CompositionController are incompatible.
It is also not sufficient evidence to approve a production architecture change.

## Implemented architecture

```text
Win32 HWND
└─ Windows.UI.Composition DesktopWindowTarget
   └─ ContainerVisual root
      └─ ContainerVisual passed to WebView2 RootVisualTarget

DesktopAcrylicController
└─ targets the projected CompositionTarget for the same DesktopWindowTarget
```

`Microsoft.UI.Composition` is not used. The only Microsoft UI API involved is
Windows App SDK's `DesktopAcrylicController` and its configuration/target API.

WebView2 is created exclusively through
`ICoreWebView2Environment3::CreateCoreWebView2CompositionController`. The PoC
does not call `CreateCoreWebView2Controller`.

## Build result

- `cargo check --offline`: PASS
- `cargo test --offline`: PASS (compilation test; zero unit tests)
- self-contained preparation: PASS
- Windows App SDK self-contained initialization: `S_OK`
- WebView2 Runtime observed: `148.0.3967.54`

## Runtime result

The default co-existence run reached:

```text
[poc] acrylic_attach_result=true
[poc] acrylic_state=fallback
[poc] composition_controller_creation_call_hresult=0x00000000
[poc] composition_controller_creation_callback_hresult=0x00000000
[poc] webview_composition_controller_created=true
[poc] root_visual_target_set=true
```

The `RootVisualTarget` getter returned the same COM identity as the supplied
`Windows.UI.Composition.ContainerVisual`.

The WebView2 GPU child then failed with:

```text
kind=6 reason=3 exit_code=0xc0000022
```

This is GPU-process crash + `STATUS_ACCESS_DENIED`. Renderer processes then
reported launch failure and navigation completed with connection-aborted. The
same sequence occurs with all of these negative controls:

- Acrylic omitted;
- Windows App SDK initialization omitted;
- WebView2 `--disable-gpu` diagnostic enabled;
- normal HWND DWM redirection surface used.

Therefore the observed child-process failure is independent of Acrylic and the
self-contained Windows App SDK runtime. The sandbox-external retry did not
return diagnostics and was terminated after a bounded wait. Native window
screen capture/control was unavailable because the Windows control service was
not configured.

## Input result

The PoC translates `WM_MOUSEMOVE`, `WM_LBUTTONDOWN`, and `WM_LBUTTONUP` into
`ICoreWebView2CompositionController::SendMouseInput`. A QA click traverses the
real HWND message procedure. Because the browser process has already exited,
the calls correctly fail with `0x8007139f` (`ERROR_INVALID_STATE`); the HTML
`pointerdown` acknowledgment cannot run in this environment.

This also confirms an architectural cost: visual hosting makes the application
responsible for forwarding input. Production quality would additionally need
leave/wheel/cursor, keyboard, touch/pen, capture, IME, accessibility, DPI, and
focus handling. The isolated PoC intentionally does not implement that stack.

## Decision

Phase 7C.0 is **not PASS** because the required simultaneous visible Acrylic and
WebView2 pixels were not observed.

CompositionController remains the technically correct WebView2 API for a
`Windows.UI.Composition` visual tree, but it is **not yet justified as this
Tauri/Wry product's final architecture**. Adopting it would replace Wry's
windowed host and transfer substantial input, accessibility, focus, DPI, and
lifecycle ownership to product code (or require upstream Wry support/forking).

## Next gate

Run the generated executable from a normal, unlocked interactive desktop:

```powershell
.\target\self-contained\dist\native-composition-webview-poc.exe
```

The gate passes only when all are simultaneously true:

1. diagnostics reach `[poc] acrylic_state=active`;
2. `Hello WebView2 Composition` is visibly rendered;
3. live Acrylic remains visible in the transparent area;
4. clicking the button emits `[poc] pointer_event_received=true`.

Even if that gate passes, the recommended next action is an upstream Wry
feasibility assessment for composition hosting and input ownership, not a
production integration. If upstream support is unavailable and forking remains
forbidden, retain a windowed WebView architecture and use a backdrop approach
compatible with HWND hosting.
