# Future Composition Hosting Extraction

> **Future design note / architecture decision record — not a roadmap commitment.**
> This document records what a 2026-09-14 read-only extraction feasibility audit and a
> same-day policy/mechanism decoupling experiment concluded about the Windows Enhanced
> rendering stack. The experimental refactor was **reverted after the experiment** and is
> **not adopted** in the product code; it exists here as evidence that the boundary is
> real. This document does not commit the project to publishing a standalone library.

## Background

The Enhanced rendering backend of desktop-todo-widget spans a full Windows composition
stack:

- **Tauri / Wry** — the vendored, locally patched Wry 0.55.1 (`vendor/wry`) creates an
  `ICoreWebView2CompositionController` instead of the ordinary windowed controller when a
  process-wide hook supplies a root visual target;
- **WebView2 CompositionController** — the WebView is composited as a visual instead of a
  child HWND window;
- **Windows Composition visual tree** — the app owns a `Compositor`, a
  `DesktopWindowTarget`, and the container visuals the WebView visual is attached to
  (`src-tauri/src/platform/windows/composition_host/`);
- **Windows App SDK Acrylic** — a self-contained Windows App Runtime loads
  `DesktopAcrylicController` onto the same shared composition target;
- **input / DPI / lifecycle / shutdown handling** — a mouse-to-`SendMouseInput` bridge,
  a raw-pixel DPI strategy, dispatcher/apartment ownership, and a proven two-stage
  shutdown order.

Because all of this is substantial, QA-verified Windows machinery that knows almost
nothing about the todo product itself, the project evaluated whether part of it could
become a reusable Rust library for other Rust/Tauri developers.

## What appears genuinely reusable

The audit classified the following as **mechanism**, not desktop-todo-widget product
logic. None of these need to know what a task, an Orb, or a Sidebar is:

- HWND owner-thread / STA / DispatcherQueue lifecycle ownership;
- the composition visual tree (`Compositor` → `DesktopWindowTarget` → root → WebView
  container visual);
- physical-pixel resize (client rect is re-read as the single geometry source of truth);
- the raw-pixel DPI strategy (`UseRawPixels` + `ShouldDetectMonitorScaleChanges(false)` +
  `RasterizationScale = dpi / 96` — a set solution, not three independent knobs);
- the mouse / focus input bridge (button-down `MoveFocus` + `SetCapture`, up
  `ReleaseCapture`, wheel `ScreenToClient`; keyboard/IME stay on the base controller
  path — verified with a real Chinese IME);
- Windows App Runtime preload (must run before any window exists: the composition
  factory runs inside Wry's WebView-creation call stack, where `LoadLibrary` deadlocks
  on the loader lock);
- Acrylic controller lifecycle (capability probe, attach to the shared target,
  active/fallback state, explicit release order);
- the two-stage shutdown ordering (material first, visual tree deliberately released
  after Wry detaches `RootVisualTarget`, then dispatcher/apartment/runtime).

## Product-specific responsibilities

The following belong to the product and must **not** enter a generic hosting layer:

- Floating / Sidebar / Desktop product modes and the collapsed Orb presentation;
- task/product state, settings, tray menus, product appearance profiles and their CSS
  fallbacks (transparent-orb tint, translucent graphite);
- geometry persistence (DIP storage, monitor identity, snap-to-edge);
- Desktop shell attachment and recovery (Progman / `SHELLDLL_DefView` / WorkerW
  discovery, WinEvent-driven revalidation, Win+D survival) in `window_mode.rs`;
- product window orchestration in `product_window.rs`.

## Mixed responsibilities found during audit

The audit found exactly three places where product policy was welded into the hosting
mechanism:

1. **WebView selection** — the host factory hard-coded `id != "main"`, so the mechanism
   knew the product's window label;
2. **Material policy** — "only FloatingExpanded + Glass gets a
   `DesktopAcrylicController`" (with `TransparentOrb` / `ExistingProductMaterial` /
   `GlassFallback` resolutions) lived inside `composition_host::material`;
3. **Desktop input workaround trigger** — the runtime-window relocation repair was
   keyed on a `desktop_input: bool` field named after the product's Desktop mode.

## What the experiment proved

A behavior-preserving, subsequently **reverted** experimental refactor
(`experimental refactor / proof of boundary` — not adopted) demonstrated:

- the `"main"` selection can be injected by the caller as a `host_when(webview_id, hwnd)
  -> bool` predicate with a default that hosts nothing;
- the Acrylic policy can move back to the product layer as one boolean function
  (`requests_native_acrylic`), leaving the host with mechanism-only `attach_acrylic` /
  `detach_material` entries with identical execution order, error propagation, and
  fallback behavior;
- the Desktop input trigger can become a mechanism-level request
  (`set_input_route_constrained(bool)`) with the product still deciding when its window
  sits in a constrained input route;
- `composition_host` can compile and behave with **zero knowledge of product modes** —
  no `"main"`, no `FloatingExpanded`, no Desktop-named workaround (remaining "desktop"
  identifiers are Windows API vocabulary such as `DesktopWindowTarget` and
  `DesktopAcrylicController`);
- vendored Wry needed **no changes** for any of this; `window_mode.rs` needed none; and
  `product_window.rs` needed only minimal call-site adjustments (the removed couplings
  were themselves part of the host's public API surface).

## Hosting invariants that must remain together

Future extraction work — regardless of target shape — must not split the following
owners apart for the sake of fine-grained modularity:

1. **thread / apartment / dispatcher** — the HWND owner-thread check, STA
   initialization, and `DispatcherQueueController` are mutually dependent and must be
   created and released together;
2. **visual tree + resize** — one `DesktopWindowTarget` carries both the Acrylic target
   and the WebView visual; resize re-reads the client rect and resizes both visuals;
3. **DPI strategy** — `UseRawPixels`, monitor-scale detection off, and
   `RasterizationScale` are one contract; changing one breaks the others;
4. **input / focus bridge** — capture pairing, focus on button-down, wheel coordinate
   conversion, and the keyboard path through the base controller were verified together;
5. **shutdown ordering** — material → (Wry detaches `RootVisualTarget`) → composition
   target → dispatcher → apartment → runtime. Ordering mistakes surface only at exit.

## Wry boundary

- CompositionController creation, `RootVisualTarget` handling, composition-mode resize
  behavior, composition-mode DPI handling, and the drop ordering (detach the root
  visual target before `controller.Close()`) belong in the **Wry WebView2 backend
  layer**; the application cannot substitute for them.
- The current vendored Wry patch adds three small, generic, opt-in process-wide hooks
  (host factory, resize handler, mouse forwarder). That patch is the **potential
  upstream candidate**; it leaves the default windowed path untouched and contains no
  product vocabulary.
- A standalone host library and an upstream Wry contribution are **not mutually
  exclusive**: the natural layering is upstream hooks (transport) + separate crate
  (visual tree, backdrop, input translation, runtime preload).
- If Wry later ships native CompositionController support, the host/backdrop/input layer
  keeps independent value as a consumer of that API.
- No upstreaming is planned or in progress; the vendored patch remains the local
  mechanism of record.

## Maintenance risks

Recorded in decreasing practical severity for any future extraction:

- **Wry/Tauri internal coupling** — the hooks are a local patch; every Wry upgrade can
  require re-basing it against subclass and bounds internals;
- **WebView2 Runtime compatibility** — controller availability and
  `ICoreWebView2Environment10` paths depend on the user's runtime version;
- **Windows App SDK runtime distribution** — the self-contained
  `Microsoft.WindowsAppRuntime.dll` payload must ship with (or be installed alongside)
  the app; a library cannot hide this from its consumers;
- **COM lifetime / shutdown ordering** — release-order regressions only appear at exit;
- **undocumented Chromium / runtime-window behavior** — the input-target repair depends
  on Chromium window class names and on the counter-intuitive fact that *hiding* the
  WebView2 runtime window stops forwarded input delivery (it must be parked visible,
  off-screen);
- **DPI / multi-monitor QA** — per-monitor DPI transitions need repeated manual
  verification;
- **Windows version differences** — DWM host-backdrop and `DesktopAcrylicController`
  availability gates (probed at runtime, with CSS fallback);
- **manual native QA burden** — none of the above is fully coverable by unit tests.

## Possible future shape

Conceptual layering only — not a committed API:

```text
product layer
    ↓
optional Tauri/Wry integration
    ↓
Composition host
    ├─ visual tree
    ├─ DPI / resize
    ├─ input bridge
    └─ backdrop lifecycle
    ↓
raw Win32 / COM escape hatches
```

The escape-hatch principle matters more than the names: a future library should expose
real `windows` / `webview2-com` objects rather than seal them, so advanced consumers are
never gated on the library author's feature list.

## Why extraction is deferred

**Extraction is deferred.**

desktop-todo-widget itself has higher-priority internal refactoring ahead —
particularly `src-tauri/src/window_mode.rs` and `src-tauri/src/product_window.rs`,
whose product-side orchestration boundaries are not yet stable. Establishing an
independent crate before that settles would force two unstable structures to be
maintained in parallel — a product refactor and a library refactor — multiplying
rework and synchronization cost on QA-verified Windows code. The current priority is
stabilizing the product's own architecture first. The decoupling experiment showed the
boundary is achievable; it did not show it is *now* the best use of maintenance effort.

## Re-evaluation criteria

Reassess extraction only once most of the following hold:

- the major product-side window orchestration refactor is complete;
- the `window_mode` / `product_window` responsibility boundary is stable;
- `composition_host` has run unchanged in the real product for a meaningful period;
- the Windows lifecycle / input / DPI / shutdown invariants are still verified after
  that period;
- a concrete second consumer, or a concrete need for a minimal standalone PoC, exists;
- the Wry upstream route has a clearer verdict.

**This document does not commit the project to publishing a standalone library.**

## Experimental validation

Summary of the one-day experiment (2026-09-14, against `main` @ `f8233d9`; reverted
afterwards — the code below is *not* in the tree):

- 6 tracked files affected (5 modified, `material.rs` deleted), net **−44 lines**
  (239 insertions / 283 deletions);
- `cargo test`: **127 passed, 0 failed**, including a new product-side test guarding the
  moved material policy;
- `git diff --check`: clean; vendored Wry untouched; `window_mode.rs` untouched;
  no dependency changes.
- Manual GUI QA was intentionally **not** completed, because the experiment was not
  retained.
- Unrelated to the experiment: `cargo fmt --check` currently reports pre-existing
  repository formatting debt, and `cargo clippy -D warnings` reports pre-existing lints
  on the current toolchain (verified identical at HEAD via a stash baseline). Both are
  baseline conditions of the repository, not consequences of the experiment, and were
  not fixed in this round.
