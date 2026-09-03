# Native window and composition architecture

## Status and stop gate

Phase 7 remains stopped at **Gate A**. The framework-dependent PoC still cannot
initialize the Windows App SDK Framework in the desktop user's package graph:
`MddBootstrapInitialize` returns `0x80670016`. The official self-contained
payload now loads successfully, and the isolated PoC can create and retain its
DispatcherQueue, `Compositor`, rooted `DesktopWindowTarget`, configuration, and
`DesktopAcrylicController`; `SetTarget` returns `true` and the complete chain is
stable through a four-second Win32 message pump. This is controller-path proof,
not visual Acrylic proof. No acceptable screenshot exists yet.

Per the Phase 7 failure boundary, no Tauri Floating, Sidebar, or Desktop
integration has been attempted. The existing product and data layers remain
unchanged.

## Runtime Gate

The PoC consumes the Microsoft Windows App SDK 1.8.11 runtime package
`1.8.260804001` and the Foundation/bootstrap package `1.8.260803002`. Values
were verified against the official `WindowsAppSDK-VersionInfo.h` contained in
that runtime package rather than inferred from package family names:

| Bootstrap input | Verified value |
| --- | --- |
| Release major/minor | `0x00010008` (1.8, exact match) |
| Version tag | empty (stable) |
| Minimum runtime | `8000.946.1701.0` / `0x1F4003B206A50000` |
| Process and requested package architecture | x64 |
| Loaded bootstrap DLL file version | `1.8.0.0` |

The inputs match Microsoft's
[`MddBootstrapInitialize2`](https://learn.microsoft.com/windows/windows-app-sdk/api/win32/mddbootstrap/nf-mddbootstrap-mddbootstrapinitialize2)
contract. A fresh diagnostic run prints them before failing with
`0x80670016` (package dependency criteria could not be resolved). Current-user
package enumeration finds no `Microsoft.WindowsAppRuntime.1.8` Framework; the
only visible 1.8 package is the inbox/CBS package, which is not the Framework
dependency requested by the bootstrapper. AppX deployment events show that the
Framework/Main/DDLM 1.8.11 packages were registered successfully for one other
user SID, not for the interactive PoC user. Dynamic package dependency
resolution is per-user, so that registration cannot satisfy this process, as
described in the official
[Windows App SDK dynamic-dependencies specification](https://github.com/microsoft/WindowsAppSDK/blob/main/specs/dynamicdependencies/DynamicDependencies.md).

The prior non-elevated installer failure is separate: AppX deployment event
IDs 403/404/465 record `0x80070005` while opening the installer's temporary
MSIX. AppX, ClipSVC, and InstallService were running and no blocking per-user
AppX policy was found. Microsoft's
[framework-dependent deployment guide](https://learn.microsoft.com/windows/apps/windows-app-sdk/deploy-unpackaged-apps)
documents `0x80070005` from this installer as
failure to perform system-wide installation/provisioning without the required
elevation. No new installation, package removal, repair, registry change, or
ACL change was attempted during this diagnostic checkpoint.

Framework-dependent recovery therefore requires the official signed runtime
installer to execute with elevation in the actual interactive user's Windows
session, so its package registration becomes resolvable for that same user. If
that cannot be performed in the host environment, the next isolated research
route is an officially built unpackaged
[self-contained PoC](https://learn.microsoft.com/windows/apps/package-and-deploy/self-contained-deploy/deploy-self-contained-apps)
using Undocked RegFree WinRT initialization; copying arbitrary DLLs beside the
Rust executable is not an acceptable substitute.

The signed 1.8.11 x64 installer was subsequently launched through an approved
interactive UAC elevation and returned exit code `0`. It emitted no new AppX
deployment events, the interactive user's package list remained unchanged, and
the diagnostic PoC again returned `0x80670016`. This is not an installer failure:
it is a successful/no-op installer result that did not repair registration for
the user running the PoC. The framework-dependent route is therefore blocked by
the host's user/elevation-context separation, and no repeated install or manual
package-state manipulation will be attempted.

The isolated fallback follows Microsoft's
[self-contained deployment model](https://learn.microsoft.com/windows/apps/package-and-deploy/self-contained-deploy/deploy-self-contained-apps).
It pins Foundation `1.8.260803002` and InteractiveExperiences `1.8.260708001`,
copies only their official x64 `runtimes-framework` payload into ignored build
output, derives the activation manifest from the official package fragments,
and calls `WindowsAppRuntime_EnsureIsLoaded`. The resolved runtime is
`1.8.260804001`; the prepared layout contains 48 files, and every copied DLL
has a valid Authenticode signature. Runtime binaries and raw QA artifacts are
not committed.

## Architecture overview

The intended architecture separates product intent from Windows mechanisms:

```text
ProductSettings (Glass, Solid, Gradient, Image, WindowsWallpaper)
    -> window orchestrator
        -> NativeWindowContext (single native-state owner)
            -> WindowHost (Floating | Sidebar | CurrentShellChildHost)
            -> material resolver
                -> MaterialBackend (Acrylic | Transparent | Disabled)
```

This is the target boundary, not a claim that the production refactor has been
implemented. Gate A must pass before replacing the current implementation.

## Window Host

A host owns HWND placement and relationship mechanics: parent/owner, top-level
versus child styles, attach/detach, z-order, geometry, DPI, monitor, visibility,
topmost behavior, and Shell relationships. Floating and Sidebar are ordinary
top-level hosts. Desktop's current `SHELLDLL_DefView` + `WS_CHILD` + `SetParent`
route is named `CurrentShellChildHost` conceptually and remains replaceable.

The existing Shell adapter is not to be rewritten merely because Acrylic is
desired. If a later Desktop compatibility test proves the child host cannot be
targeted, that result becomes an explicit capability failure and a separate
host-replacement investigation.

## Material Backend

A material backend owns only visual native state: transparent background,
system backdrop setup, Composition object lifetime, enable/update/disable, and
fallback cleanup. It must not own product mode, persistence, geometry, Shell
attachment, Todo, Weather, Review, or database behavior.

The smallest reliable Rust representation may be an enum-backed state machine;
a general plugin framework is unnecessary. A C++/WinRT bridge remains acceptable
only if generated Windows App SDK bindings and COM lifetime management cannot be
kept narrow and auditable in Rust.

## Native Window Context

`NativeWindowContext` is intended to be the sole owner of the live HWND truth,
WebView2 controller access boundary, current host, requested/resolved material,
capabilities, DPI/monitor snapshot, DispatcherQueue, Compositor,
CompositionTarget, and controller lifetime. Host and backend operations receive
this context; they do not rediscover or cache HWND independently.

Shutdown order matters. Backdrop targets/controllers must be removed or closed
before their CompositionTarget, then the Compositor/DispatcherQueue, and finally
the Windows App SDK bootstrap reference. Mode changes must disable the old
backend before changing an incompatible host relationship.

## Capability detection

The future central capability snapshot should include the Windows build,
top-level/child host kind, system transparency policy, composition availability,
Desktop Acrylic support, host-backdrop support, and future Mica support. Build
numbers are inputs, not proof: runtime `IsSupported`, target creation, controller
return state, and visual evidence remain distinct gates.

The tested host is Windows 11 24H2 build `26100.8037`. The isolated failure is a
deployment/package-graph failure, not evidence that the OS compositor lacks
Acrylic support.

## Resolver

`resolve_material(requested, host, mode, capabilities)` should be pure and
unit-tested. Expected policy:

| Request and host | Resolved backend |
| --- | --- |
| Glass + expanded Floating top-level + supported | Acrylic |
| Glass + Sidebar top-level + supported | Acrylic |
| Glass + collapsed Floating | Transparent orb fallback |
| Glass + Desktop child with no proven support | Transparent/solid fallback with reason |
| Solid, Gradient, Image, Windows Wallpaper | Disabled native backdrop |

The result records requested product intent, selected backend, and a structured
fallback reason. UI settings continue to persist `Glass`; they never persist a
Windows API or backend name.

## WebView2 transparency chain

The required chain is:

```text
transparent HTML/CSS roots
    -> transparent WebView2 DefaultBackgroundColor
        -> transparent Tauri host client area
            -> native CompositionTarget/system backdrop
                -> windows or desktop content behind the HWND
```

Transparency is necessary but is not Acrylic. The future composition backend
must be the single owner of WebView2 `DefaultBackgroundColor`; the orchestrator,
appearance module, and hosts must not each set it independently.

## Acrylic implementation research

Microsoft's Win32 path requires:

1. initialize COM/WinRT on the UI thread;
2. initialize the Windows App SDK package graph for an unpackaged process;
3. create one current-thread DispatcherQueue and keep its controller alive;
4. create a `Windows.UI.Composition.Compositor`;
5. create a `DesktopWindowTarget` for the top-level HWND;
6. set `DWMWA_USE_HOSTBACKDROPBRUSH` on that top-level HWND;
7. create and retain `DesktopAcrylicController`;
8. call `SetTarget(WindowId, CompositionTarget)` on the DispatcherQueue thread;
9. treat its Boolean return as setup state, not visual proof;
10. close targets/controllers before tearing down composition and bootstrap.

Official references:

- [DesktopAcrylicController.SetTarget](https://learn.microsoft.com/windows/windows-app-sdk/api/winrt/microsoft.ui.composition.systembackdrops.desktopacryliccontroller.settarget)
- [Using the Visual Layer with Win32](https://learn.microsoft.com/windows/uwp/composition/using-the-visual-layer-with-win32)
- [CreateDispatcherQueueController](https://learn.microsoft.com/windows/win32/api/dispatcherqueue/nf-dispatcherqueue-createdispatcherqueuecontroller)
- [DWMWA_USE_HOSTBACKDROPBRUSH](https://learn.microsoft.com/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute)
- [Unpackaged Windows App SDK deployment](https://learn.microsoft.com/windows/apps/windows-app-sdk/deploy-unpackaged-apps)

## Isolated proof of concept

The PoC lives in `tools/native-acrylic-poc`. It uses an ordinary top-level test
fixture painted with red, blue, and green regions, a fine white grid, and
high-contrast text. A separate overlapping top-level HWND is the composition
target. Its optional one-shot BMP capture exists only for QA; there is no screen
capture loop and no captured-image blur mechanism.

Observed sequence:

| Step | Result |
| --- | --- |
| WinRT single-thread apartment | Success |
| Test fixture and Acrylic HWND creation | Success |
| Self-contained Undocked RegFree WinRT initialization | Success |
| DispatcherQueue creation | Success |
| DesktopWindowTarget creation | Success |
| Root `ContainerVisual` set and retained | Success |
| `DWMWA_USE_HOSTBACKDROPBRUSH=true` | Success |
| Controller activation without bootstrap | `0x80040154` (`REGDB_E_CLASSNOTREG`) |
| Microsoft-signed runtime/bootstrap artifacts | Signature valid |
| Bootstrap with 1.8 / min `8000.946.1701.0` | `0x80670016` |
| Current desktop-user Framework package | Absent; only CBS packages visible |
| Current-user installer attempt | `0x80070005` (access denied in sandbox) |
| Elevated package registration | Registered in isolated elevated context, not the desktop-user graph |
| Generated official-WinMD Rust controller projection | Success |
| `SystemBackdropConfiguration` (active, dark) | Set and retained |
| `DesktopAcrylicController.SetTarget` | Returned `true` |
| Complete message-pump survival | Stable for 4 seconds, clean shutdown |
| Screenshot/visual result | Not available; Gate A remains failed |

The PoC exposes an activation matrix from `runtime-only` through `runtime-full`.
All nine supported positive variants survived the same four-second message loop
and exited with code 0. Both configuration-before-target (the retained final
order) and target-before-configuration survived. The no-configuration target
variant also survived, but it is diagnostic only because the official
configuration contract still applies.

The earlier `0xC000027B` delayed failure was isolated with a deliberate
`runtime-full-no-root` negative control. It repeats the complete setup except
for `DesktopWindowTarget.Root`; `SetTarget` returns `true`, then the process
terminates in the message pump with `STATUS_STOWED_EXCEPTION`. Restoring and
retaining a `ContainerVisual` root makes the otherwise identical full variant
stable. This distinguishes a deferred invalid Composition target from a Rust
vtable or temporary projected-object lifetime failure. No native debugger is
installed in this environment, so the nested stowed HRESULT was not recovered;
the fail-fast was not swallowed.

The remaining blocker is capture, not runtime/controller stability. The host
has no valid screen DC for `BitBlt`, and the official Windows Graphics Capture
`CreateForWindow` path returns `0x80070424` because its capture service is not
available in this execution environment. Only an interactive-desktop screenshot
showing fixture colors and spatially blurred grid/text through the Acrylic HWND
can pass Gate A.

The first human A/B run found that target removal exposed an opaque white client
surface, so that negative control was invalid and Gate A remained pending. The
fixture class was not responsible: only it owns a paint brush and colored GDI
painting. The Acrylic class already had a null `hbrBackground`, and its retained
Root is an empty `ContainerVisual` with no SpriteVisual or ColorBrush. The actual
gap was the test HWND's ordinary DWM redirection surface plus default message
handling. The corrected Composition host now uses
[`WS_EX_NOREDIRECTIONBITMAP`](https://learn.microsoft.com/windows/win32/winmsg/extended-window-styles),
explicitly handles
[`WM_ERASEBKGND`](https://learn.microsoft.com/windows/win32/winmsg/wm-erasebkgnd)
without a fill, and validates `WM_PAINT` with an empty `BeginPaint`/`EndPaint`
pair. Runtime diagnostics confirmed class background `0x0`, extended style
`0x00200100` with the no-redirection flag present, and an empty rooted target;
the complete chain again survived four seconds and shut down cleanly. Human
verification that T is now sharp transparency is still required before Acrylic
can be assessed.

## Lifecycle and mode gates

Floating integration may begin only after Gate A. It must reuse the existing
Tauri HWND/WebView2, verify Glass/Solid/Image transitions and 20-cycle
enable/disable and collapse/expand tests, and preserve the 56 DIP Orb invariant.
Sidebar follows only after Floating passes. Desktop follows only after Sidebar
and begins with a compatibility test of the current child host; incompatibility
stops work before any host rewrite.

## Fallback behavior

Fallbacks are explicit resolver results, never silent claims of Acrylic. Solid,
Gradient, Image, and Windows Wallpaper do not request a native backdrop.
Collapsed Floating uses transparent orb rendering. Desktop Glass uses the
documented translucent Graphite fallback until its host compatibility is proven.

## Diagnostics and privacy

Safe diagnostics may include requested/resolved material, backend, host kind,
capability flags, Windows build, target/controller state, HRESULT/code, and a
sanitized fallback reason. They must omit usernames, absolute personal paths,
database/task content, coordinates, weather location details, managed image IDs,
and wallpaper paths.

## Windows App SDK build, runtime, and packaging impact

The PoC targets stable Windows App SDK 1.8.11 (`1.8.260804001`) with runtime
minimum `8000.946.1701.0`; 1.8 is near end of support and must be reconsidered
before production integration. The development machine has the Windows SDK but
not Visual Studio C++ Build Tools, CMake, or a .NET SDK, which favors a narrow
Rust ABI experiment but is not a release-toolchain decision.

Framework-dependent unpackaged deployment has the smallest application payload
but requires the matching Windows App Runtime and bootstrap initialization on
every target machine. Self-contained deployment copies framework files beside
the app, increases installer and binary payload substantially, and changes
packaging/build rules. Microsoft recommends deploying WinMD files where runtime
marshalling might need them. Release installer/CI work is intentionally deferred.

## Known limitations and recommendation

- Gate A is unresolved; no production Acrylic backend exists.
- No visual screenshot can be accepted from this run; capture is unavailable in
  the current execution desktop.
- The PoC now uses bindings generated from the official Windows App SDK WinMD;
  the former handwritten controller ABI has been removed.
- Existing `product_window.rs` and `window_mode.rs` responsibilities remain
  coupled because changing them after Gate A failed would violate the stop gate.
- The current Shell child host remains experimental and untested with
  `DesktopAcrylicController`.

Recommendation: run the already prepared self-contained `runtime-full` PoC on
the interactive desktop and capture red, blue, and mixed-boundary positions plus
the transparent A/B control. Proceed to production architecture work only after
those images visibly prove spatial backdrop blur.
