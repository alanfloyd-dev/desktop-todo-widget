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
