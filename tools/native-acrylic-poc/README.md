# Native Acrylic proof of concept

This is an isolated Win32/Windows Composition diagnostic. It does not load
Tauri or WebView2 and is intentionally kept outside the product runtime.

The process creates two ordinary top-level Win32 windows on one UI thread:

- a large painted test fixture containing color blocks, thin lines, text, and
  a checker pattern;
- a smaller transparent composition target positioned over that fixture.

The foreground window asks `DesktopAcrylicController` to target its
`DesktopWindowTarget`. It also enables `DWMWA_USE_HOSTBACKDROPBRUSH`, which the
Windows App SDK documentation requires for Desktop Acrylic on a Win32 HWND.

The PoC is an unpackaged process. It requires a matching Windows App Runtime
1.8 Framework package in the current user's package graph and the Microsoft
signed `Microsoft.WindowsAppRuntime.Bootstrap.dll` beside the executable. The
bootstrapper is intentionally not committed to this repository.

Before bootstrap, the executable prints its requested major/minor, stable
version tag, minimum runtime version, process architecture, and the loaded
bootstrap DLL's file version. After a successful bootstrap it enumerates the
dynamic package graph and prints the resolved Framework package full name and
architecture. These diagnostics contain no username, SID, or personal path.

Run from this directory with `cargo run`. The console reports bootstrap,
DispatcherQueue, target, DWM attribute, and controller results. Press Escape or
close either window to stop the process. For a single QA capture that exits
automatically after three seconds, run:

```powershell
cargo run -- --qa-capture native-acrylic-gate-a.bmp
```

Visual acceptance requires the fixture colors and details to participate in a
clearly blurred Acrylic backdrop. A successful HRESULT or `true` return alone
is not accepted as proof.

On the Phase 7 development machine, bootstrap currently stops with HRESULT
`0x80670016` because the matching Framework dependency is not registered in the
desktop user's package graph. The product integration must not begin until this
PoC reaches `READY` and its capture passes visual review.

## Self-contained diagnostic

If the current user cannot resolve the framework package, prepare the isolated
PoC with the official component-package self-contained layout:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/prepare-self-contained.ps1
target/self-contained/dist/native-acrylic-poc.exe --self-contained --qa-capture qa-output/self-contained.bmp
```

The preparation script pins and verifies the Foundation and
InteractiveExperiences NuGet packages, copies their x64 `runtimes-framework`
payload and metadata, generates WinRT registration from the official package
fragments, and embeds that manifest during the MSVC-compatible Rust link. The
binary then calls the official Undocked RegFree WinRT
`WindowsAppRuntime_EnsureIsLoaded` entry point instead of the package-graph
bootstrapper. All downloaded/generated assets remain ignored under `target`;
no runtime binary is committed.

## Activation matrix

Run one isolated layer for four seconds with:

```powershell
target/self-contained/dist/native-acrylic-poc.exe --self-contained --qa-variant runtime-full --qa-seconds 4
```

Supported variants are `runtime-only`, `runtime-dispatcher`,
`runtime-compositor`, `runtime-target`, `runtime-controller`,
`runtime-controller-config`, `runtime-controller-target`, `runtime-full`, and
`runtime-target-then-config`. `runtime-full-no-root` is a deliberate negative
control: it reproduces the deferred `STATUS_STOWED_EXCEPTION` after `SetTarget`
by omitting the root visual. It must never be treated as a valid setup.

The positive target variants retain the `DesktopWindowTarget`, its projected
`CompositionTarget`, a root `ContainerVisual`, `WindowId`, configuration,
controller, compositor, and DispatcherQueue for the complete message loop.
Shutdown explicitly removes targets and closes the controller before releasing
configuration, composition, DispatcherQueue, and finally the runtime lifetime.

The one-shot capture first attempts the existing screen BMP path. If the host
does not expose a usable screen DC, it tries the official Windows Graphics
Capture HWND API. Some automation desktops provide neither a screen DC nor the
per-user capture service; that is a capture-environment failure and never counts
as visual proof.

## Regenerating Windows App SDK bindings

After preparing the self-contained payload, regenerate the narrow projection
from the official `Microsoft.UI.winmd` and `Microsoft.Foundation.winmd` files:

```powershell
cargo run --manifest-path bindgen/Cargo.toml
```

The generator pins `windows-bindgen` 0.61.0 and emits `src/winappsdk.rs`. No
handwritten Acrylic vtable is used.

## Human visual QA

From a normal, non-administrator Windows PowerShell on the interactive desktop:

```powershell
.\target\self-contained\dist\native-acrylic-poc.exe --self-contained --qa-manual
```

The manual mode never captures the screen and does not time out. Drag the
foreground window across the red, blue, green, mixed-boundary, grid, and text
areas. All probes retain the same HWND, root visual, and fixture position:

- `T`: detach Acrylic for the sharp transparent negative control;
- `A`: restore the default Acrylic properties;
- `M`: set the QA-only fallback color to unmistakable `#FF00FF`;
- `1`: set `TintOpacity` to zero;
- `2`: set `LuminosityOpacity` to zero;
- `3`: set both opacity properties to zero;
- `S`: print the current controller state, backdrop configuration, transparency
  setting, high-contrast state, remote-session state, and battery-saver state;
- Escape: exit.

In transparent mode, grid/text edges should remain sharp. In live Acrylic mode,
the sampled background color should change with position and the same edges
must show clear spatial blur rather than a uniform tint, black frame, or plain
alpha transparency. If the entire client changes to magenta in the `M` probe,
the controller is displaying `FallbackColor`, not a live Acrylic backdrop.
`SetTarget` succeeding is lifecycle/API evidence only; the printed controller
state distinguishes `active`, `fallback`, and `high-contrast` rendering.

The window activation handlers keep `SystemBackdropConfiguration.IsInputActive`
in sync through `WM_ACTIVATE` and `WM_ACTIVATEAPP`. The diagnostics read system
conditions without changing Windows settings. The magenta fallback color and
opacity probes are isolated QA settings and are not production defaults.

The test-window class has a null background brush, handles `WM_ERASEBKGND`
without filling, and performs an empty `BeginPaint`/`EndPaint` validation for
`WM_PAINT`. Its empty `ContainerVisual` has no SpriteVisual or color brush. The
foreground HWND uses `WS_EX_NOREDIRECTIONBITMAP` so the transparent comparison
does not expose an opaque default Win32 redirection surface when the Acrylic
controller target is removed.

If an older manual-QA process is still running and locking the default output,
prepare a separate ignored output slot with
`scripts/prepare-self-contained.ps1 -OutputSlot qa-transparent`. The slot name
is restricted to a short lowercase alphanumeric/hyphen value and always remains
under the PoC `target` directory.
