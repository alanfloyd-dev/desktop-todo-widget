# Phase 7C.2 production-shell CompositionController spike — resumable handoff

Status: the CompositionController production-shell path **starts successfully**, the real Vue
product frontend loads through it, and **all eight QA gates (A–H) passed**. Three integration
defects were found and fixed this session (two fatal startup blockers and one window-collapsing
geometry feedback loop). No commit or push was made.

## Repository state

- Branch: `arch/composition-controller-production-spike`
- HEAD: `400fddb481bd8fb35271579cbf3a246bc5b0f2a1` (unchanged — nothing committed)
- Commit/push: none
- Required preservation rule: do not reset, clean, stash, discard, or overwrite the pre-existing dirty work.

`git status --short --branch`:

```text
## arch/composition-controller-production-spike
 M .gitignore
 M src-tauri/Cargo.lock
 M src-tauri/Cargo.toml
 M src-tauri/build.rs
 M src-tauri/src/lib.rs
 M src-tauri/src/product_window.rs
 M src/main.ts
?? docs/phase-7c1-wry-integration-assessment.md
?? docs/phase-7c2-production-composition-spike.md
?? src-tauri/src/native_composition.rs
?? src-tauri/src/qa_diagnostics.rs
?? src-tauri/windows-app-sdk.manifest
?? tools/native-composition-webview-poc/
?? vendor/
```

`.gitignore`, `src-tauri/Cargo.lock`, `src/main.ts`, the 7C.0 PoC, the WinAppSDK manifest, and much
of the native Acrylic/QA plumbing predate 7C.2 and must not be attributed to or reverted as part of
this spike.

## Nine Phase 7C.2 files and diff summary

| File | 7C.2 delta |
|---|---|
| `src-tauri/Cargo.toml` | Add exact Wry dependency/patch, `windows-numerics`, needed Windows feature. Tracked aggregate: +22 lines. |
| `src-tauri/Cargo.lock` | Resolve local path Wry and direct Windows dependency metadata. Tracked aggregate: +4/-2. Already dirty. |
| `src-tauri/build.rs` | **New this session.** Keep the WinAppSDK manifest, and copy the validated self-contained Windows App SDK payload (45 files: `.dll`/`.winmd`/`.pri`) beside the executable so the opt-in composition path can load its runtime. Tracked aggregate: +78. |
| `src-tauri/src/lib.rs` | Register opt-in factory/resize callbacks before Tauri creates the config WebView; **preload the Windows App Runtime before any window exists**; attach diagnostic/controller settings after setup. Tracked aggregate including older work: +93/-9. |
| `src-tauri/src/product_window.rs` | Share the native context store with the pre-WebView factory through `Arc`; expose attach/store accessors. Tracked aggregate including older work: +239/-12. |
| `src-tauri/src/qa_diagnostics.rs` | Add `--qa-composition-controller` selection and log field. Already untracked. |
| `src-tauri/src/native_composition.rs` | Create/retain shared Windows.UI.Composition target/root/WebView child visual, raw-pixel geometry callback, attach Acrylic to the same composition target, defer host release until Wry teardown. **New this session:** `preload_windows_app_runtime()` plus runtime reuse in `NativeWindowContext::new`. Already untracked. |
| `vendor/wry/src/lib.rs` | Add process-wide Windows-only root-target factory and resize callback hooks. |
| `vendor/wry/src/webview2/mod.rs` | Select `CreateCoreWebView2CompositionController[WithOptions]`, retain base controller compatibility, set `RootVisualTarget`, hide the helper HWND in composition mode, forward move/down/up/right/wheel, update raw-pixel bounds/DPI, detach root in `Drop`, and **skip the parent-window `SetWindowPos` in composition mode** (see blocker 2). |

`vendor/wry/` is exact Wry 0.55.1 plus the two edits above (82 files).

## Integration defects found and fixed this session

- **Blocker 1** (fatal at startup): the self-contained Windows App Runtime could not load.
- **Blocker 2** (fatal, geometry): a `WM_SIZE` feedback loop collapsed the window to zero width.

### Blocker 1 — self-contained Windows App Runtime could not load (crash)

Symptoms, in order:

```text
[phase7c2] enabled=true scope=main default_windowed_unchanged=true
... then a hang inside the composition factory ...
Failed to setup app: error encountered during setup hook: the underlying handle is not available
```

Two independent root causes:

1. `LoadLibraryW("Microsoft.WindowsAppRuntime.dll")` was executed **from inside Wry's WebView
   creation call stack** (factory → `NativeWindowContext::new` → `initialize_self_contained`), which
   deadlocks on the Windows loader lock. Diagnosed by step-tagging the factory: it printed
   `window-context: threads ok` and then never printed past `runtime: loading ...dll`.
2. The runtime DLL was not present beside `src-tauri/target/debug/alan-desktop.exe`. The 7C.0 PoC
   succeeds because its own `dist/` directory contains the whole validated self-contained payload.
   Once loaded early, the load failed explicitly with `0x8007007E` ("The specified module could not
   be found").

Fix (bounded, Windows-only):

- `native_composition::preload_windows_app_runtime()` loads and `EnsureIsLoaded`-es the runtime from
  `run()` **before any window or WebView exists**, and records the module handle in a static.
  `NativeWindowContext::new` reuses the preloaded module instead of loading a DLL from inside the
  WebView creation stack.
- `build.rs` copies the Phase 7A/7C.0-validated payload (`.dll`/`.winmd`/`.pri` only) from
  `tools/native-composition-webview-poc/target/self-contained/dist` into the profile output
  directory beside the exe. The payload is copied only into ignored build output and is never
  committed. This copy is the same self-contained model the Acrylic proof already validated; it is
  a packaging concern that Phase 7C.3/7C.4 must replace with proper installer/resources handling.

With both fixes the factory is reached and the runtime initializes:

```text
[native-material] runtime_mode=self-contained runtime_loaded=true
[phase7c2] windows_app_runtime_preloaded=true module=...
[phase7c2] desktop_window_target_created=true root_visual=ContainerVisual webview_visual=ContainerVisual
[phase7c2] host_selected=true webview_id=main hwnd=... root_visual_target_set=pending
[wry] webview_controller_type=ICoreWebView2CompositionController
[phase7c2] webview_composition_controller_created=true root_visual_target_set=true input_bridge=mouse-wheel-focus
```

### Blocker 2 — `WM_SIZE` feedback loop collapsed the window to zero width

First successful composition run rendered nothing usable: the Tauri window was driven down to
**0 × 490** and the composition visual was left at a stale size. The Wry geometry log showed a
monotonic cascade (63 resizes, ~22 px of width and ~13 px of height lost per iteration):

```text
geometry ... 480x1528
geometry ... 458x1515
...
geometry ... 0x1242
```

Step-tagging the Wry parent subclass produced the decisive evidence:

```text
[dbg-resize] hit=1 msg=WM_SIZE hwnd=0x... parent_known=true parent_is_self=true computed=480x1528
```

Root cause: in composition mode `controller.ParentWindow()` is the **top-level Tauri HWND itself**
(composition hosting passes the top-level window as the controller parent). Wry's `WM_SIZE` handler
therefore called `SetWindowPos(top_level, 0, 0, width, height)`, which re-entered the same handler
with a slightly different client rect, forever. In windowed mode the same call targets Wry's private
`WRY_WEBVIEW` child HWND, which is correct and harmless.

Fix: skip that `SetWindowPos` only when composition hosting is active (`state.composition.is_none()`
guard). `SetBounds` (raw pixels) and the shared composition visual already carry the geometry.
Windowed hosting keeps byte-identical behaviour, and Gate H confirms it.

After the fix: exactly one `WM_SIZE` is handled, and the composition visual settles at the true
window client size:

```text
[dbg-resize] hit=1 msg=WM_SIZE hwnd=0x...19(removed after diagnosis) computed=840x1528
[phase7c2] geometry client_physical_px=840x1528 dpi=144 scale=1.5000 root_size=840x1528 webview_visual_size=840x1528 bounds=840x1528
```

## Composition runtime status — startup and visuals confirmed

The opt-in CompositionController path now reaches the real product shell end to end and the
operator has visually confirmed the result (see "Human QA results" below):

- `[wry] webview_controller_type=ICoreWebView2CompositionController` (explicit runtime proof).
- Root visual target set; one hidden `WRY_WEBVIEW` helper HWND plus one visible
  `Chrome_WidgetWin_0` composition surface — **no second windowed WebView2 controller**.
- The real Vue entrypoint loads through Vite (`http://localhost:1420`), not inline PoC HTML:
  `webview_navigation_completed=true`, `webview2_navigation_completed=true success=true`,
  `frontend_ready=true`.
- Geometry is identical to the default windowed run of the same build in the same mode:
  `webview2_bounds=0,0,840x1528` with `dpi=144 scale=1.5000` in **both** runs.

Note for future QA operators: during this QA run `apply_native_composition` resolved the live
window as **Sidebar**, so it produced `ExistingProductMaterial` and the native DesktopAcrylic
controller was **not** created (`requested_material=Glass resolved_material=ExistingProductMaterial
native_backend=none window_mode=Sidebar`). The transparency chain (`set_effects` + transparent
WebView2 background) is still exercised. The persisted setting in
`%APPDATA%\net.alanfloyd.desktop\alan-desktop.sqlite3` (`app_settings.product_settings`) still reads
`"mode":"desktop"` with `"sidebarWidth":560`, so the resolved host must be re-checked when the mode
or appearance changes. Desktop Acrylic only engages for the Glass Floating-expanded host, and that
combination was **not** exercised in this session — it remains a residual QA gap for 7C.3, not a
7C.2 failure.

## Build and test evidence

Passed this session (clean tree, no debug instrumentation):

```text
cargo check --manifest-path src-tauri/Cargo.toml     -> Finished `dev` profile
cargo test  --manifest-path src-tauri/Cargo.toml     -> 64 passed; 0 failed
pnpm build                                           -> vue-tsc --noEmit + vite build, 33 modules, built in 1.07s
cargo build --manifest-path src-tauri/Cargo.toml     -> Finished `dev` profile
```

All temporary diagnostics were removed before these runs: no `dbg-*` tags remain in
`src-tauri/src/*.rs`, `vendor/wry/src/**`, or the local cargo registry
(`tauri-runtime-wry-2.11.4/src/lib.rs` was restored to pristine).

## Human QA results (interactive desktop, this session)

Build under test: `cargo build` profile, launched as
`alan-desktop.exe --qa-composition-controller` with Vite serving `http://localhost:1420`.
The operator visually inspected and interacted with the real window.

| Gate | Result | Operator observation |
|---|---|---|
| A — startup | **PASS** | App launched, CompositionController active, real Vue UI on screen. |
| B — visual | **PASS (with note)** | "UI 基本正常" — the full product UI renders, no black rectangle, no postage stamp. The only remark was that the window is large; that is the persisted product setting `sidebarWidth: 560` (the maximum clamp) on a 2560×1600 display at 150% DPI, i.e. `560×1018` DIP = the measured `840×1528` physical. Windowed hosting produces the identical size, so this is a persisted preference, not a composition defect. |
| C — pointer | **PASS** | Wheel, right-click context menu, and left-click all work. Hover could not be exercised in this session and is recorded as unverified-by-observation, with the caveat below. |
| D — keyboard/focus | **PASS** | ASCII input, Backspace/Delete, arrow keys, Tab, focus switching, and alt-tab away/back all work. |
| E — Chinese IME | **PASS** | Real IME verified by the operator: candidate UI appears, composition/commit works, no duplicate or lost characters, no stuck composition, no crash on switching. |
| F — geometry/DPI | **PASS** | Window and content fill correctly at 150% DPI with no clipping and no double scaling; the operator sees the intended sidebar layout. The earlier `WM_SIZE` collapse is gone (exactly one resize handled). |
| G — lifecycle | **PASS** | Minimize/restore, focus switching, and close all behave normally. Operator reports a clean shutdown: "关闭干净无崩溃、无残留进程". Verified afterwards that no `alan-desktop` process remains. |
| H — Windowed regression | **PASS** | Separate no-flag run: `[wry] webview_controller_type=ICoreWebView2Controller`, visible `WRY_WEBVIEW` child, `webview2_bounds=0,0,840x1528`, navigation completed, app stays alive. |

### Pre-existing product issues observed during QA (NOT caused by 7C.2, NOT fixed here)

- In Desktop mode the operator cannot add a todo item, and Desktop-mode todo drag does not work;
  Floating-mode drag does work. The operator states both are problems that already existed before
  this spike. They are outside the Phase 7C.2 boundary and must not be "fixed" as part of this work.
- Hover in Sidebar mode was not exercised because the operator could not add a todo item to hover
  over. Pointer down/up/click/wheel/right-click were all confirmed, so the composition input bridge
  is demonstrably live.



## Gate evidence summary

Superseded per-gate detail is above; the current authoritative status is:

| Gate | Status |
|---|---|
| A — startup | PASS (runtime-proven + human-observed) |
| B — visual | PASS (operator-observed) |
| C — pointer | PASS except hover unexercised (see caveat) |
| D — keyboard/focus | PASS |
| E — Chinese IME | PASS |
| F — geometry/DPI | PASS |
| G — lifecycle | PASS |
| H — windowed regression | PASS |


## Known blockers and risks

1. **No blocking gate remains.** All eight gates pass, with one narrow observability gap: Sidebar
   hover was not exercised because the operator could not add a todo item to hover over (a
   pre-existing Desktop-mode product bug). Pointer down/up/click/wheel/right-click are all confirmed,
   so the input bridge is live; treat hover as low residual risk, not a failure.
2. **Runtime payload distribution is a spike-only mechanism.** `build.rs` copies 45 files from the
   ignored PoC output. A real release needs installer/resource handling, and Windows App SDK 1.8 is
   near end of support — both are Phase 7C.3+ work, deliberately not attempted here.
3. **Teardown ordering** held up in this session's clean close, but it still has only one observed
   run. Repeated open/close and mode-switch cycles are Phase 7C.3 work.
4. **IME/keyboard:** the operator confirmed real Chinese IME behaviour with no input-architecture
   work; no stop condition was triggered.
5. **Accessibility:** not implemented; CompositionController changes HWND-backed accessibility
   assumptions. Outside this spike and required before release.
6. **Debug-build launch method:** `pnpm tauri dev` cannot forward an app argument
   (`pnpm tauri dev -- -- --flag` makes Tauri emit `cargo run --flag ...` and Cargo rejects it).
   The working method used this session is a running Vite dev server plus
   `alan-desktop.exe --qa-composition-controller`. Do not conclude "hosting failed" from a bare
   debug exe without the dev server — that failure was `frontend_asset_mode=dev-server` with no
   Vite listening.
7. **Vite dev server can crash** with `EBUSY ... watch 'src-tauri\src\.product_window.rs.*.tmpdir'`
   while source files are being edited. Restart `pnpm dev` if the frontend stops loading.

## Exact resume commands

```powershell
Set-Location 'D:\Documents\Desktop_todo_list'
git branch --show-current          # arch/composition-controller-production-spike
git rev-parse HEAD                 # 400fddb481bd8fb35271579cbf3a246bc5b0f2a1
git status --short --branch
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
cargo build --manifest-path src-tauri/Cargo.toml   # also copies the WinAppSDK payload beside the exe
pnpm dev                                           # terminal 1: keep Vite on :1420
```

Terminal 2, composition run (Gate A + B–G):

```powershell
& 'src-tauri\target\debug\alan-desktop.exe' --qa-composition-controller
```

Default windowed regression run (Gate H):

```powershell
& 'src-tauri\target\debug\alan-desktop.exe'
```

Runtime log is written to `src-tauri/target/debug/phase7b-qa-material-on.log` and echoed to stderr.
The decisive lines are `[wry] webview_controller_type=...` and `[phase7c2] ...`.

## Do not redo or roll back

- Do not recreate Phase 7C.0; its real-desktop visual result is established and its directory is the
  reference implementation (and the source of the validated runtime payload).
- Do not repeat the 7C.1 call-path assessment; conclusion B already justified this Wry patch boundary.
- Do not re-vendor a different Wry version or upgrade Tauri/Wry/WebView2/WinAppSDK dependencies.
- Do not remove `preload_windows_app_runtime()` or the `build.rs` payload copy; without them the
  opt-in path deadlocks or fails to load its runtime.
- Do not remove the `state.composition.is_none()` guard in the Wry `WM_SIZE` handler; without it the
  window collapses to zero width in composition mode.
- Do not change the default windowed path, the Vue UI, product modes, database/schema, domain logic,
  macOS/Linux paths, or Desktop/WorkerW architecture.
- Do not re-mark B–G on the basis of compilation, COM API success, or log output. They are PASS
  because the operator observed them; any future re-run needs the same human observation.
- Do not retry the failed computer-use service.
- Do not "fix" the pre-existing Desktop-mode todo add/drag defects inside this spike.
- Do not commit, push, reset, clean, checkout-discard, or stash-drop.

Recommendation at this checkpoint: **PROCEED**. All eight gates pass: the production-shell
CompositionController path starts with the real Vue frontend, renders the product UI at correct
150% DPI geometry with no black rectangle or postage stamp, accepts wheel/click/right-click,
handles keyboard and focus, works with a real Chinese IME, survives lifecycle transitions, shuts
down cleanly, and leaves the default windowed backend intact. Both blockers that previously made
this path unrunnable were real integration defects and are fixed with small, explained,
Windows-only changes.

Next step: **Phase 7C.3 — productionization / boundary cleanup / accessibility / long-term
maintainability.** Do not start 7C.3 inside this spike. In particular, 7C.3 should own: replacing
the `build.rs` payload copy with real packaging/installer handling, Windows App SDK version and
end-of-support policy, the accessibility/automation-provider story, repeated lifecycle stress
cycles, moving the `platform/windows/composition_host/` boundary into place, and re-checking
`sidebarWidth` defaults and the pre-existing Desktop-mode todo add/drag defects.
