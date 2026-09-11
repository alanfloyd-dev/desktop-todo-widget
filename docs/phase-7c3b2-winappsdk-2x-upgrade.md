# Phase 7C.3-B2 — WinAppSDK 2.x upgrade spike

Status: **PASS**. The self-contained Windows App SDK runtime moved from the
out-of-support 1.8 baseline to the current stable 2.x line, and **Acrylic was
visually confirmed** on the new runtime. Recommendation: **PROCEED**.

Nothing committed or pushed. HEAD is unchanged at `2c5fd57`.

## Result summary

| Item | Outcome |
|---|---|
| Build gates (check / test / frontend / debug / release) | PASS |
| Self-contained output (no missing DLL, no SxS error, no loader-lock regression) | PASS |
| Runtime load (`WindowsAppRuntime_EnsureIsLoaded`) | PASS |
| Debug windowed / debug composition | PASS |
| Release windowed (embedded path) / release composition | PASS |
| **Acrylic activation + visible blur (Glass + Floating-expanded)** | **PASS — human-confirmed** |
| Acrylic active↔fallback lifecycle | PASS |
| Clean shutdown, no orphans | PASS |
| Rollback to 1.8 | PASS — verified by rebuilding and launching |
| Recommendation | **PROCEED** |

## 1. 1.8 baseline (A/B reference)

Recorded before any change, and re-verified after refactoring the stager.

| Property | Value |
|---|---|
| Foundation package | `Microsoft.WindowsAppSDK.Foundation` `1.8.260803002` |
| Foundation SHA256 | `B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E` |
| InteractiveExperiences package | `Microsoft.WindowsAppSDK.InteractiveExperiences` `1.8.260708001` |
| InteractiveExperiences SHA256 | `496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76` |
| Resolved runtime version | `1.8.260804001` |
| Payload | 48 files / 17.23 MB — 20 dll, 23 winmd, 2 pri, 2 exe, 1 generated manifest |
| `Microsoft.WindowsAppRuntime.dll` file version | `1.8` |
| Manifest source | generated from both packages' `runtimes-framework/package.appxfragment` |
| Manifest SHA256 | `0A2102A7391E7A21CA16AC2E5CA96D8D1CE60B23F46A98AA7BB6C940391A11E2` |
| Activation entries | 11 `<asmv3:file>` blocks, 175 `activatableClass` entries |
| Runtime EOL | **09/09/2026 — already elapsed** |

Prior validated behaviour (from the 7C.2 / 7C.3-A / 7C.3-B1 handoffs, not re-run
broadly): composition Gate A–H PASS, release embedded-frontend fix PASS.

## 2. 2.x package set (Step 2 findings)

Verified from official package metadata rather than assumed.

The umbrella package `Microsoft.WindowsAppSDK` `2.4.0` (stable, released
08/13/2026) declares its component dependencies in its own nuspec:

| Component | Version | Role |
|---|---|---|
| `Microsoft.WindowsAppSDK.Foundation` | **2.3.9** | native runtime payload + activation fragment |
| `Microsoft.WindowsAppSDK.InteractiveExperiences` | **2.1.6** | Acrylic/composition activation fragment + payload |
| `Microsoft.WindowsAppSDK.Base` | `2.0.4` | declared dependency, **no native payload** |
| `Microsoft.WindowsAppSDK.Runtime` | `[2.4.0]` | declared dependency |
| (umbrella) `Microsoft.WindowsAppSDK` | `2.4.0` | dependency aggregator only |

**All exact versions and SHA256 values are pinned in the stager; nothing floats.**

| Package | Version | SHA256 |
|---|---|---|
| `Microsoft.WindowsAppSDK` (umbrella) | `2.4.0` | `6EC2EBB6ADD33ECEBAC1F5773AD4CABE934B82FB18D7BEA98E011BB0FC0A37B9` |
| `Microsoft.WindowsAppSDK.Foundation` | `2.3.9` | `230BC605A3FC9ED689B2117056C5274923BF58B453FA44EDDE18A168BBF628BE` |
| `Microsoft.WindowsAppSDK.InteractiveExperiences` | `2.1.6` | `DE7B5907C63C8A79606CCC8F0D98943B154A2E62312308187E8CDC3304FF3D0B` |
| `Microsoft.WindowsAppSDK.Base` | `2.0.4` | `E3E13478C4C80C59ED5F8F89542FE49A2985DAA484753E93A5858E90C2D46A4D` |

### Package layout differences from 1.8

| Aspect | 1.8 | 2.x | Impact |
|---|---|---|---|
| Component split | Foundation + InteractiveExperiences | same, **plus** Base/WinUI/DWrite/Widgets/AI/ML/Search (declared by umbrella) | **none** — only the two payload components are consumed; Base/umbrella are pinned for provenance but carry no self-contained native payload in this model |
| `runtimes-framework/<arch>/native` | present in both components | identical | none |
| `metadata/*.winmd` | present | identical | none |
| `package.appxfragment` | present in both components | present in both components | none |
| Fragment XML shape | `Fragment > Extensions > Extension > InProcessServer > Path/ActivatableClass` | **identical** | the same generator serves both |
| `runtimes/` (non-framework) | Foundation has it | Foundation has it | unused, as in 1.8 |
| Payload file set | 48 files | **48 files, identical names** | none |

### Activation manifest differences

| Property | 1.8 | 2.x |
|---|---|---|
| `<asmv3:file>` blocks | 11 | 11 (**same DLL names**) |
| `activatableClass` entries | 175 | **177** (+2) |
| `DesktopAcrylicController` / `MicaController` / `SystemBackdropConfiguration` in `wuceffectsi.dll` | present | **present** |
| Manifest SHA256 | `0A2102A7…A11E2` | `98B23F5F…A5388` |

The only semantic delta is two additional activation classes; the class names this
product depends on are unchanged. **No hand-written activation entries were
introduced**, and the XML fragment + tag-balance validation from 7C.3-A is intact.

### Runtime contract checks

| Check | Result |
|---|---|
| `WindowsAppRuntime_EnsureIsLoaded` export present | ✅ (both 1.8 and 2.x) |
| `Microsoft.WindowsAppRuntime.dll` file version | `2.0` (1.8 reports `1.8`) |
| Authenticode on all payload DLLs | ✅ all 20 valid, signed by Microsoft Corporation |
| Resolved runtime version | `2.4.0` (declared), DLL file version `2.0` |

## 3. Staging layout (Step 3)

Payloads are now **versioned so two runtimes coexist**, enabling A/B and rollback:

```text
tools/windows-app-sdk/runtime/
  active-runtime.txt          <- generated selector consumed by build.rs (currently "2.x")
  1.8/x64/                    <- Windows App SDK 1.8 payload (preserved)
  2.x/x64/                    <- Windows App SDK 2.x payload
```

`git check-ignore` confirms the whole `runtime/` tree (including the selector) stays
git-ignored, so no payload or generated manifest is committable.

Staging a runtime **never deletes** another runtime's payload. The stager writes the
selector by default; `-NoActivate` stages without switching, and `-ListRuntimes`
prints the available payload sets.

The new pipeline was proven to reproduce the old one exactly before it was trusted:
the regenerated 1.8 payload is **byte-identical** to the previously validated
baseline (same file set, same manifest SHA256 `0A2102A7…A11E2`).

## 4. Code changes (Step 4) — minimal and version-only

| File | Change |
|---|---|
| `tools/windows-app-sdk/prepare-runtime-payload.ps1` | Reworked into a pinned **runtime-set table** (`1.8`, `2.x`) with `-Runtime`, `-ListRuntimes`, `-NoActivate`; emits into versioned directories; writes the selector. |
| `src-tauri/build.rs` | `payload_dir()` resolves the payload through the selector; added `runtime_root()` and `selected_runtime()`; watches `active-runtime.txt` for re-runs. |

Deliberately unchanged: `NativeHost` API, visual-tree structure, input forwarding,
geometry, lifecycle, DPI handling, Wry CompositionController selection, WebView2
controller type, product UI, IPC/protocol architecture, composition host design.

`payload_dir()` falls back to the legacy unversioned path when the selector is
absent or malformed, so the "payload missing" error still names an actionable path.

## 5. Build gates (Step 5)

| Gate | Result |
|---|---|
| `cargo check` | PASS |
| `cargo test` | PASS — **69 passed, 0 failed** |
| `pnpm build` | PASS — 33 modules |
| `tauri`/debug build | PASS |
| `cargo build --release --features custom-protocol` (from an emptied `target/release/`) | PASS |
| Release output self-consistency | 46 runtime files / 19.95 MB, `Microsoft.WindowsAppRuntime.dll` **v2.0**, **no** `DeploymentAgent.exe` / `RestartAgent.exe` |
| Launch from a clean directory outside the repo (exe + payload only) | PASS |

No missing DLL, no side-by-side configuration error, no loader-lock regression, no
runtime load failure, and no dependency on a globally installed Windows App SDK
(the payload is loaded from beside the exe).

## 6. Runtime QA (Step 6)

| Run | Result | Evidence |
|---|---|---|
| Debug windowed | PASS | `[wry] webview_controller_type=ICoreWebView2Controller`, dev-server `http://localhost:1420/` |
| Debug composition | PASS | `ICoreWebView2CompositionController`, `runtime_mode=self-contained runtime_loaded=true`, `frontend_ready=true` |
| Release windowed | PASS | embedded `http://tauri.localhost/`, `success=true`, `frontend_ready=true` |
| Release composition | PASS | `ICoreWebView2CompositionController`, `desktop_window_target_created=true`, `host_selected=true`, embedded path, `frontend_ready=true` |

Frontend routes are unchanged from 7C.3-B1: debug uses the dev server, release uses
the embedded asset protocol.

## 7. Mandatory Acrylic proof (Step 7) — PASS

Ran the **Glass + Floating-expanded** host, which is the only product combination
that creates a `DesktopAcrylicController` and therefore the only one that proves the
2.x RegFree activation manifest is correct.

```
[native-material] webview_default_background_before=ARGB(0,0,0,0) webview_default_background_after=transparent
[native-material] composition_target=DesktopWindowTarget top_level=true root_visual=ContainerVisual controller_generation=1 set_target=true
[native-material] lifecycle_event=StateChanged controller_state=active controller_generation=1
[native-material] requested_material=Glass resolved_material=DesktopAcrylic native_backend=DesktopAcrylicController desktop_acrylic_supported=true controller_state=active set_target=true runtime_mode=self-contained window_mode=Floating
```

Interpretation: the `Microsoft.UI.Composition.SystemBackdrops.DesktopAcrylicController`
COM class **resolved through the 2.x activation manifest** (`desktop_acrylic_supported=true`
comes from `DesktopAcrylicController.IsSupported()`), the shared `DesktopWindowTarget`
was retained, and `SetTarget` returned true.

State machine observed in one run — repeated cycles then back to active:

```text
StateChanged active → fallback → active → fallback → active → fallback → active
```

**Human visual confirmation (required, and obtained):** the operator reports real
frosted blur over the desktop background, no opaque black rectangle, and the real
WebView2 UI still visible.

Lifecycle deactivate/reactivate, driven by stealing and returning window focus:

```text
[native-material] lifecycle_event=Focused input_active=false controller_generation=2
[native-material] lifecycle_event=Focused input_active=true  controller_generation=2
```

Note on method: the headless `SW_MINIMIZE`/`SW_RESTORE` path emitted no state
transitions, so the lifecycle check was driven by a real foreground-focus change
instead. This is why the gate is reported from focus-driven evidence.

## 8. DPI / geometry sanity (Step 8) — PASS

At the machine's 150% display:

```text
geometry client_physical_px=657x1103 dpi=144 scale=1.5000 root_size=657x1103 webview_visual_size=657x1103 bounds=657x1103
```

Floating window measured `438x735` DIP (= `657x1103` physical) with `dpi=144`,
i.e. `RasterizationScale` stayed `144/96 = 1.5`. The composition visual matches the
client area exactly — no postage-stamp rendering, no double scaling, and no
`WM_SIZE` cascade. Resize continued to work during the run.

## 9. Regression boundary (Step 9)

| Check | Result |
|---|---|
| debug composition / debug windowed | PASS |
| release composition / release windowed | PASS |
| embedded frontend path | PASS |
| dev-server path | PASS |
| IPC (`http://ipc.localhost/*` product commands reaching the app) | PASS — `frontend_ready=true` requires a successful `qa_frontend_ready` invoke, and `product_state`/`today_tasks` calls are served |
| Clean shutdown | PASS (below) |
| Pointer / click / wheel / keyboard / focus | Not re-exercised in this spike. The 2.x change touches only the runtime payload, not the input bridge, and the composition input forwarder plus keyboard/IME were confirmed in 7C.2. Recorded as carried-over evidence, not re-verified here. |

Clean shutdown with 2.x, on `WM_CLOSE`:

```text
[native-material] shutdown=targets-removed
[native-material] shutdown=subscription-removed
[native-material] shutdown=controller-closed
[native-material] shutdown=composition-released
[phase7c2] lifecycle=host-release-deferred-until-wry-drop
```

Process exited within ~2 s; **zero orphan processes** afterwards. This is the first
time the teardown path was exercised headlessly (it was previously only
operator-observed in 7C.2, because Desktop mode hides the window from top-level
enumeration). It behaves correctly.

## 10. Rollback path

Verified, not just documented:

1. `prepare-runtime-payload.ps1 -Runtime 1.8` → repoints the selector (`SelectorPrevious: 2.x → SelectorActive: 1.8`).
2. Rebuild → release output contains the **1.8** payload (`Microsoft.WindowsAppRuntime.dll` v1.8, 46 files).
3. The 1.8 payload directory was never deleted while 2.x was staged, so no re-download is needed (it also restages from the local NuGet cache).

The tree is currently left with the selector on **`2.x`** and the release payload at
**v2.0**, i.e. the passing configuration.

## 11. Packaging consequence worth recording

Self-contained deployment is **application-owned and not serviceable**: Microsoft
states the Windows App SDK version distributed with an app "can be updated only by
releasing a new version of your app. You're responsible for integrating servicing
updates of the Windows App SDK into your app."

Practical meaning for this repository: every future Windows App SDK security or
servicing fix, and the next end-of-servicing date (**2.x line: 2027-04-29**),
requires a new application release that restages this payload. The pinned runtime-set
table in the stager is the single place that must be updated, and the procedure is
documented in `docs/windows-app-sdk-runtime.md`.

## 12. Files changed in this phase

```
 M src-tauri/build.rs                                 (+51/-…)
 M tools/windows-app-sdk/prepare-runtime-payload.ps1  (+194/-…)
```

Plus a new document (`docs/phase-7c3b2-winappsdk-2x-upgrade.md`, this file) and an
update to `docs/windows-app-sdk-runtime.md`.

Not committed, not pushed. Generated payloads remain git-ignored.

## 13. Decision: PROCEED

2.x passes build gates, runtime activation, both frontend routes, CompositionController
hosting, DPI/geometry, clean shutdown, and — critically — **Acrylic visual
verification on the exact host that exercises the activation entries**. No blocker
remains, and rollback to 1.8 is verified.

### Residual notes (non-blocking)

- `Microsoft.WindowsAppRuntime.dll` reports file version `2.0` while the release line
  is `2.4.0`; the DLL's MajorMinor is the WinAppSDK platform version, not the product
  version. Recorded so it is not mistaken for a mis-stage — the payload is verifiably
  the 2.4.0 component set by package version and SHA256.
- The 2.x line EOS is **2027-04-29**; add it to the maintenance calendar.
- The old unversioned `tools/windows-app-sdk/runtime/x64/` directory was removed after
  the versioned layout reproduced it byte-identically.
- 1.8 remains staged and selectable for rollback.

## 14. Exact commands

```powershell
Set-Location 'D:\Documents\Desktop_todo_list'

# stage / inspect runtimes
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -ListRuntimes
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 2.x
Get-Content tools\windows-app-sdk\runtime\active-runtime.txt

# gates
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
pnpm build
cargo build --manifest-path src-tauri/Cargo.toml
cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol

# runtime QA (dev server on :1420 for the debug runs)
pnpm dev
& 'src-tauri\target\debug\alan-desktop.exe' --qa-composition-controller
& 'src-tauri\target\debug\alan-desktop.exe'

# rollback
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 1.8
cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol
```

## 15. Do not redo or roll back

- Do not re-open or redesign CompositionController hosting.
- Do not re-derive the 2.x package set: Foundation `2.3.9` / InteractiveExperiences
  `2.1.6` / Base `2.0.4`, pinned with SHA256 in the stager.
- Do not reintroduce a hand-written activation manifest; it is generated from
  `package.appxfragment` and validated for tag balance.
- Do not remove the versioned payload layout or the selector; they are the rollback
  mechanism.
- Do not copy payload files from the 7C.0 PoC.
- Do not change Tauri/Wry versions, the Vue UI, product behaviour, DB/domain, window
  modes, or IPC architecture as part of this upgrade.
- Do not commit or push without explicit approval.
