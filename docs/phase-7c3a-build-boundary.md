# Phase 7C.3-A — build artifact self-consistency and the platform boundary

Status: **complete**. The temporary payload shortcut is gone, the payload now has a
single documented repo-owned source, Windows composition hosting lives behind an
explicit platform boundary, and the full regression sweep passes. Two blockers were
found for the follow-on work; one is fixed here, one is a pre-existing release-build
defect recorded for 7C.3-B.

No commit or push was made.

## Repository state

- Branch: `arch/composition-controller-production-spike`
- HEAD: `400fddb481bd8fb35271579cbf3a246bc5b0f2a1` (unchanged)
- Commit/push: none

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
?? docs/windows-app-sdk-runtime.md
?? src-tauri/app.manifest
?? src-tauri/src/platform/
?? src-tauri/src/qa_diagnostics.rs
?? tools/native-composition-webview-poc/
?? tools/windows-app-sdk/
?? vendor/
```

## 1. Payload: from PoC shortcut to a repo-owned pipeline

**Before:** `build.rs` copied `.dll`/`.winmd`/`.pri` out of
`tools/native-composition-webview-poc/target/self-contained/dist` — another
project's ignored build output.

**After:** the payload is produced from pinned upstream packages by a repo-owned
stager, and nothing is committed.

| Concern | Resolution |
|---|---|
| Payload source | `tools/windows-app-sdk/prepare-runtime-payload.ps1` downloads the two official component packages from `api.nuget.org`, verifies each by SHA256, extracts `runtimes-framework/win-x64/native/*` plus `metadata/*.winmd`, and asserts every `.dll` carries a valid Authenticode signature. |
| Payload location | `tools/windows-app-sdk/runtime/x64/` — 48 files, git-ignored. No dependency on the 7C.0 PoC anymore. |
| WinRT activation manifest | Generated from the packages' own `package.appxfragment` files, emitted as an XML **fragment** (`composition-host.manifest`). Never hand-maintained, so the activatable-class list cannot drift from the runtime. |
| Base app manifest | `src-tauri/app.manifest` now holds only the product's own requirement (Common Controls v6). |
| Merge | `build.rs` splices the fragment into the base manifest before `</assembly>`, validates tag balance, and hands the result to `tauri-build` for embedding. |
| Debug + release | `build.rs` copies the payload into whichever profile output directory the build is using, so `target/debug/` and `target/release/` are both self-consistent. |
| Missing payload | **Build fails** with the exact command to run. Shipping an executable that cannot start its composition path is not possible. |
| Superseded file | `src-tauri/windows-app-sdk.manifest` deleted (hand-written activation list, now generated). |

Deployment model decision, versions, EOL policy, and the upgrade procedure are in
**`docs/windows-app-sdk-runtime.md`**.

## 2. Platform boundary: `platform/windows/composition_host/`

`src-tauri/src/native_composition.rs` (monolithic, 984 lines) was split into one
module per concern:

```text
src-tauri/src/platform/
  mod.rs                     platform boundary; non-Windows compiles none of it
  windows/
    mod.rs
    composition_host/
      mod.rs                 public surface + Wry hook registration
      runtime.rs             Windows App Runtime preload and lifetime
      host.rs                window context + process-side ContextStore facade
      visual.rs              DesktopWindowTarget and the shared visual tree
      geometry.rs            raw physical client pixels, DPI, rasterization scale
      input.rs               Win32 mouse/wheel -> SendMouseInput translation
      material.rs            pure requested/resolved material policy (+ tests)
      material_backend.rs    DesktopAcrylicController + DWM host backdrop
      lifecycle.rs           focus/exit glue and release ordering
```

Boundary rules now enforced by structure:

- The only types the platform module exports upward are `NativeHost` (a
  product-level host description), `ContextStore`, and the `Arc`-shared alias
  `NativeWindowContextStore`. No `HWND`, COM interface, or composition object
  crosses the boundary.
- `product_window.rs` no longer reaches into store internals: it calls
  `composition_host::{attach_controller, apply_material, set_input_active,
  shutdown, diagnostic_summary}`.
- `lib.rs` no longer registers Wry callbacks itself; it calls
  `composition_host::register_wry_hooks(&store)`.
- Non-Windows targets compile none of this: `platform/mod.rs` is empty for them,
  and the product code degrades to no-ops.

### Vendored Wry patch stayed small — and got smaller

The mouse bridge moved out of `vendor/wry/src/webview2/mod.rs` and into
`composition_host/input.rs`. Wry now only detects the mouse messages and delegates
through a registered forwarder (`set_windows_composition_mouse_forwarder`), mirroring
the existing factory/resize hook pattern. The coordinate decoding, wheel delta
extraction, capture, and focus handling — which are application policy — now live
inside the platform boundary with unit tests. Wry also dropped the now-unused
`ReleaseCapture`/`SetCapture` imports.

## 3. Bug found and fixed during verification

**Invalid manifest from a text-based merge.** The first implementation appended
only lines starting with `<asmv3:file` / `<winrtv1:activatableClass`, which
**dropped every `</asmv3:file>` closing tag**. The embedded manifest was therefore
malformed and Windows refused to load the executable:

```text
The application has failed to start because its side-by-side configuration is incorrect.
```

`mt.exe` confirmed it: `Failed to read the manifest from the resource ... Windows
无法分析请求的 XML 数据`. Fixed by generating an XML fragment upstream and splicing
its inner markup verbatim, then asserting tag balance before use. This defect would
have shipped a completely unbootable release build, so it is worth a regression
test if the merge is ever touched again.

## 4. Build and test evidence

All run clean on the final tree:

| Check | Result |
|---|---|
| `cargo check` | PASS |
| `cargo test` | **69 passed, 0 failed** (was 64; +5 from the extracted `input` and `material` module tests) |
| `pnpm build` | PASS — `vue-tsc --noEmit` + `vite build`, 33 modules |
| `cargo build` (debug) | PASS |
| `cargo build --release` (from an empty `target/release/`) | PASS in ~6 min |
| `tauri build --debug --no-bundle` | PASS — `Built application at src-tauri/target/debug/alan-desktop.exe` |
| Release output self-consistency | `alan-desktop.exe` + 46 runtime files, **no** `DeploymentAgent.exe`/`RestartAgent.exe` |
| Embedded manifest | contains both `Microsoft.Windows.Common-Controls` and the Windows App SDK `DesktopAcrylicController` / `MicaController` / `SystemBackdropConfiguration` activation entries |
| Release launch from a directory **outside** the repository | PASS for the WinAppSDK runtime contract — `windows_app_runtime_preloaded=true`, `[wry] webview_controller_type=ICoreWebView2CompositionController`. Frontend rendering was the 7C.3-B1 defect, now fixed. Note: release must be built with `--features custom-protocol`. |
| Debug composition launch | PASS — controller type composition, real Vue frontend over the dev server, `frontend_ready=true` |
| Windowed launch (same build, no flag) | PASS — `[wry] webview_controller_type=ICoreWebView2Controller` |
| Graceful close / no orphan processes | PASS by operator observation in 7C.2 (the teardown code only moved between files; no logic change). **Not reproducible headlessly in this session:** the persisted product mode is `desktop`, which reparents the window as a child of `SHELLDLL_DefView`, so no enumerable top-level `Tauri Window` exists for a scripted `WM_CLOSE`. Every process started during this work was terminated and none remained, but that was an operator kill, not a verified graceful exit. Re-run this gate interactively. |

## 5. Blocker found, NOT fixed here: release build does not render the UI

The release executable **loads its runtime and creates the CompositionController**
but the product UI never appears:

```text
frontend_asset_mode=embedded
webview2_navigation_completed=true success=false web_error_status=0
```

`frontend_ready=true` is never emitted, so the embedded frontend never finishes
loading.

Evidence that this is **pre-existing and unrelated to composition hosting**: the
identical failure occurs with the same release binary in **windowed mode**
(`webview_controller_type=ICoreWebView2Controller`, no composition host involved).
Composition hosting is therefore not the cause.

Ruled out during diagnosis:

- Missing `dist/`: rebuilt `pnpm build` and forced a full relink; no change.
- Payload/DLL presence: the payload is complete and the runtime preload succeeds.
- Manifest: the merged manifest is valid and the executable now starts.

**RESOLVED in Phase 7C.3-B1** — see `docs/phase-7c3b1-release-embedded-frontend.md`.
The cause was not the asset protocol at all: the release binary was navigating to
`build.devUrl` (`http://localhost:1420/`) because Tauri derives its dev/production
split from the `custom-protocol` cargo feature rather than from the cargo profile,
and the release build was invoked as a bare `cargo build --release`. Fixed by
declaring the feature explicitly and building releases with
`--features custom-protocol`. Both release gates now pass on the genuine embedded
asset path.

## 6. Version / EOL finding requiring a decision

Windows App SDK **1.8 reached end of servicing on 2026-09-09**, two days before this
work. The latest stable line is **2.x** (2.4.0, EOS 04/29/2027). Microsoft's own
page still labels 1.8 "Maintenance", but the published date has passed.

This work deliberately did **not** bump the pin:

- self-contained deployment is documented as *not serviceable*, so a version change
  is an application release, not a patch;
- Microsoft publishes no step-by-step upgrade guide for unpackaged self-contained
  apps;
- the current payload is the only one validated end to end on this machine, and
  changing it during a boundary refactor would conflate two variables.

Recorded as: **1.8 is out of support and must be upgraded to 2.x**, with a concrete
procedure in `docs/windows-app-sdk-runtime.md`. The upgrade should be its own
verified task with a Glass + Floating-expanded Acrylic check, because that is the
only host combination that proves the Acrylic activation entries resolve.

## 7. Explicitly NOT done (scope discipline)

- No installer or packaging UI. `tauri build` bundling untouched.
- No business-logic, Vue, database, product-mode, or domain-contract changes.
- macOS/Linux paths untouched; `platform/mod.rs` is empty for them.
- No dependency upgrades.
- No Desktop-mode todo add/drag fix and no Sidebar hover work — those are 7C.3-B.
- `window_mode.rs` still holds HWNDs for the Desktop/WorkerW reparenting path. That
  is a pre-existing 7B-era boundary violation, documented here rather than
  refactored, because migrating it is a separate change with its own risk.

## 8. Next: Phase 7C.3-B — stability + release blockers

In priority order:

1. ~~**Release build cannot render the UI** (above).~~ **DONE in 7C.3-B1.**
2. **Windows App SDK 1.8 → 2.x upgrade** with the verified procedure.
3. Desktop-mode todo add/drag defects and Sidebar hover (operator-reported,
   pre-existing).
4. Glass + Floating-expanded Acrylic path — **provably untested**. 7C.2 QA resolved the
   window to Sidebar/Desktop, and the one QA flag that forces Floating-expanded
   (`--qa-window-to-visual`) also sets the native-material bypass, so it cannot reach
   Acrylic. This is the only host that creates a `DesktopAcrylicController`, and
   therefore the only host that proves the generated manifest's Acrylic activation
   entries actually resolve. Reaching it needs a deliberately separated flag, not a
   change to product behaviour.
5. IME/accessibility hardening and repeated lifecycle stress cycles.
6. Distribution: include the payload next to the executable in any archive;
   installer/bundling work.

## Exact resume commands

```powershell
Set-Location 'D:\Documents\Desktop_todo_list'
git branch --show-current          # arch/composition-controller-production-spike
git rev-parse HEAD                 # 400fddb481bd8fb35271579cbf3a246bc5b0f2a1
git status --short --branch

# Stage the runtime payload (required before any build; ~17 MB, git-ignored)
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1

cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
pnpm build
cargo build            --manifest-path src-tauri/Cargo.toml   # stages payload into target/debug
cargo build --release  --manifest-path src-tauri/Cargo.toml   # stages payload into target/release

pnpm dev                                                       # terminal 1: Vite on :1420
& 'src-tauri\target\debug\alan-desktop.exe' --qa-composition-controller   # terminal 2
& 'src-tauri\target\debug\alan-desktop.exe'                                # Gate H windowed
```

Note: `cargo build` **without** the staged payload is a hard error by design. Run
the stager first (or re-run it after changing the pinned versions).

## Do not redo or roll back

- Do not reintroduce the 7C.0 PoC `dist` directory as a payload source.
- Do not hand-write the Windows App SDK activation classes back into
  `src-tauri/app.manifest`; they are generated from the pinned packages.
- Do not replace the manifest merge with line-based text filtering; that is exactly
  the bug that produced an unbootable executable.
- Do not drop the tag-balance validation in `build.rs`.
- Do not remove `preload_windows_app_runtime()` or its ordering guarantee.
- Do not remove the `state.composition.is_none()` guard in the Wry `WM_SIZE` handler.
- Do not re-vendor Wry or upgrade Tauri/Wry/WebView2/WinAppSDK dependencies as part
  of unrelated work.
- Do not change the default windowed path, the Vue UI, product modes, database
  schema, domain logic, or macOS/Linux paths.
- Do not commit, push, reset, clean, checkout-discard, or stash-drop.
