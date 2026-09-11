# Phase 7C.0 WebView2 CompositionController PoC

This is an isolated Win32 experiment. It does not load Tauri, Wry, Vue, the
product database, or any product window code.

The visual tree is deliberately narrow:

```text
Win32 HWND
└─ Windows.UI.Composition DesktopWindowTarget
   └─ ContainerVisual (root)
      └─ ContainerVisual (WebView2 RootVisualTarget)

DesktopAcrylicController targets the same DesktopWindowTarget and renders
behind the transparent WebView2 composition content.

The HWND keeps its normal DWM redirection surface, matching Microsoft's Win32
WebView2 WinComp sample. The `WS_EX_NOREDIRECTIONBITMAP` switch used by earlier
Acrylic negative controls is intentionally not carried into this experiment.
```

The implementation uses `Windows.UI.Composition`, not
`Microsoft.UI.Composition`. Windows App SDK is used only for
`DesktopAcrylicController` and its backdrop configuration.

## Build and run

Prepare the isolated self-contained Windows App SDK layout:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/prepare-self-contained.ps1
```

Then run from the generated `dist` directory:

```powershell
target/self-contained/dist/native-composition-webview-poc.exe
```

Press Escape or close the window to exit. For automation-oriented diagnostics:

```powershell
target/self-contained/dist/native-composition-webview-poc.exe --qa-seconds 8
```

Expected diagnostics include:

```text
[poc] acrylic_state=active
[poc] webview_composition_controller_created=true
[poc] root_visual_target_set=true
[poc] lifecycle=attach-complete
```

Click the central `Hello WebView2 Composition` button. A successful minimal
input bridge changes the label to `Pointer received` and emits:

```text
[poc] pointer_event_received=true source=web-message
```

The mouse bridge is diagnostic only. It does not attempt production-grade
wheel, leave, cursor, keyboard, touch, pen, IME, accessibility, or capture
behavior.

`--without-acrylic` is an isolated negative-control switch for diagnosing the
WebView2 visual host. It is not a PASS configuration.

`--software-rendering` adds WebView2's `--disable-gpu` browser argument. It is a
diagnostic for GPU-process failures and is not the default acceptance path.

`--system-composition-only` skips Windows App SDK initialization and Acrylic,
leaving only system `Windows.UI.Composition` plus WebView2. It is a negative
control for self-contained runtime interaction, not a PASS configuration.

## Acceptance boundary

The API logs establish controller creation and attachment, but Phase 7C.0 is a
visual gate: the transparent area must visibly retain live Acrylic while the
WebView2 button is visible. Confirm this on an interactive desktop; an HRESULT
alone is not visual proof.
