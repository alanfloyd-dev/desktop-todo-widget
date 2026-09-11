# RC-0 — Avatar Orb reachability and Sidebar resize

Release-candidate preflight for two release-facing UX regressions reported after the
Phase 7C.3b dual-backend work landed.

| | |
| --- | --- |
| Branch | `arch/composition-controller-production-spike` |
| HEAD at start | `eb1bdf906be4a7ef895c10246c4d1f894be9b6fe` |
| HEAD at end | `eb1bdf906be4a7ef895c10246c4d1f894be9b6fe` (no commits made — QA result pending review) |
| Working tree at start | clean |
| Platform under test | Windows, DISPLAY1 2560×1600 @ **150 % DPI**, work area 2560×1528 physical / 1707×1019 DIP |
| Rendering backend under test | `standard` (v1 default), `enhanced` covered by bounded code inspection |
| Recommendation | **PROCEED** |

---

## 1. Answers to the Task 1 questions

| # | Question | Answer |
| --- | --- | --- |
| 1 | Is the Orb still implemented? | **Yes.** `src/components/FloatingOrb.vue` is intact, including the 5 DIP drag-vs-click threshold and pointer capture. |
| 2 | Is `FloatingOrb` still rendered by the component tree? | **Yes.** `App.vue:454` renders it under `v-if="floatingCollapsed"`, and `floatingCollapsed` (`App.vue:47-51`) is true only when `mode === "floating" && floatingPresentation === "collapsed"`. |
| 3 | Is there still a user-reachable action that sets Floating → Collapsed? | **Yes, three.** `ProductContent.vue:62-68` ("Collapse to Avatar Orb" footer button), the DOM context menu (`ContextMenu.vue:57`), and the native tray/Orb context menu (`product_commands.rs:199-213` build it, `:203` labels it). All three dispatch the same `floating.collapse` action. |
| 4 | Does persisted state prevent it from appearing? | **Yes — this was the entire cause.** Persisted state was `mode: "sidebar"` + `floatingPresentation: "expanded"`. Both conjuncts of `floatingCollapsed` were false. |
| 5 | Did recent backend/settings work remove or shadow the presentation state? | **No.** `floatingPresentation` (and `floatingOrbX/Y`, `floatingOrbMonitorIdentity`) are present and round-trip correctly. The field is loaded, normalized, persisted, and honoured; nothing removed or shadowed it. |
| 6 | Does Standard vs Enhanced affect it? | **No.** `RenderingBackend` selects WebView *hosting* only (`settings.rs:89-91`, `lib.rs:79-89`). `floatingPresentation` is read independently of the backend in `product_window.rs`; the backend cannot change whether the Orb path exists. |

### Conclusion for the Orb: PASS — no code defect, no code change

The Orb was never broken and no Orb code was touched. The reported symptom is fully
explained by the operator's own persisted choice: the app was last closed **in Sidebar
mode with Floating left expanded**, which is exactly the state that hides the Orb.

### Exact route to reach the Orb (verified)

In the running app:

1. Switch to Floating — right-click the content surface (or the tray icon) and choose
   **Window mode → Floating**. The sidebar collapses into the Floating widget.
2. The Floating widget is already **Expanded** (`floatingPresentation: "expanded"`).
3. Click **"Collapse to Avatar Orb"** — the avatar/name button in the widget footer
   (bottom-left of the content), or the same item in the context menu.
4. The avatar Orb appears (56 DIP circle, initials of `displayName`).
5. Click the Orb to restore the Floating widget. Drag it (>5 DIP) to move it instead.
6. Right-click the Orb for the native product menu (Floating/Sidebar/Desktop, Lock,
   Always on top, Expand Floating, Settings, Quit).

---

## 2. Orb QA results

| Check | Result | Evidence |
| --- | --- | --- |
| Orb implemented | **PASS** | `FloatingOrb.vue` present, imported and used in `App.vue` |
| Floating Expanded reachable | **PASS** | Seeded `floating/expanded` → window 679×1116 px, client 438×735 DIP |
| Collapse through real product UI | **PASS** | UIA `InvokePattern` on the real `Button 'Collapse to Avatar Orb'` → `floatingPresentation` `expanded` → `collapsed` |
| Orb appears | **PASS** | Window becomes 84×84 px = **56.0×56.0 DIP** at (2171,1112) |
| Orb size/shape correct | **PASS** | `ORB_SIZE_DIP = 56`; logo/glyph, transparent-orb-tint CSS material; visually confirmed (circular avatar with initials) |
| Orb click restores expanded | **PASS** | Real `mouse_event` left-click at Orb centre → window 679×1116 px, persisted `floatingPresentation: "expanded"` |
| Drag-vs-click behaviour | **PASS (by inspection + sibling path)** | 5 DIP threshold implemented in `FloatingOrb.vue`; drag path calls `request_window_drag`, already proven PASS and untouched |
| Right-click / context menu | **PASS** | Real right-click on the collapsed Orb → 1 native popup menu (`#32768`) via `show_product_context_menu`; Esc dismisses |
| Restart while collapsed | **PASS** | Restart → `[floating-orb] apply collapsed · logical=56x56 DIP · physical=2171,1112 84x84`, `floatingPresentation = collapsed` |
| Standard backend | **PASS** | All of the above |
| Enhanced backend | **PASS (by inspection)** | See §5 |

---

## 3. Sidebar width diagnosis (Task 2)

**No geometry regression.** The sidebar simply starts at the persisted maximum width,
amplified by the display scale factor.

| Quantity | Value |
| --- | --- |
| Persisted `sidebarWidth` | **560 DIP** |
| Clamp (`settings.rs:224`, `product_window.rs:220`, `:1054`) | min **320**, max **560** DIP |
| Default (`settings.rs:197`) | 380 DIP |
| Current DPI | **150 %** (144 dpi) |
| Requested physical width | `560 × 1.5 = 840` px |
| Effective client width | **840 px** (outer 862 px) = 560.0 DIP |
| Requested vs effective | exact match — no truncation, clamping, or double-scaling |
| Standard vs Enhanced | identical; the sidebar rect is computed before/independently of the hosting backend |

560 DIP is the maximum clamp, i.e. the widest the product permits, and on a 150 %
display that is 840 physical px ≈ 33 % of the 2560 px screen. The width is a
**persisted user preference, not a defect**.

> **Note on measurement.** An early probe reported the window as 575×1027 px, which
> looked like a DPI double-conversion bug. That was a probe artefact: the measuring
> PowerShell process was DPI-unaware, so Windows returned virtualised rectangles
> (`862 ÷ 1.5 = 574.7`). `scripts/rc0-qa/rc0-window-probe.ps1` now calls
> `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` before reading any rect. Any
> future geometry QA **must** be DPI-aware or it will misreport by the scale factor.

---

## 4. Sidebar resize root cause and fix (Task 3)

### Root cause

`handle_window_event` already contained the complete Sidebar resize contract
(`product_window.rs:1282-1291`): on `WindowEvent::Resized` in Sidebar mode it converts
the physical width to DIP, clamps it to 320–560, and persists it.

That handler was **unreachable**. `apply_product_mode` computed

```rust
let resizable = mode == ProductWindowMode::Floating
    && settings.floating_presentation == FloatingPresentation::Expanded
    && !settings.locked;
```

so Sidebar mode called `set_resizable(false)`. Tao implements that by clearing
`WS_SIZEBOX` (`tao-0.35.3/src/platform_impl/windows/window_state.rs:246-248`). The
product window is frameless (`decorations: false`, `WS_CAPTION` masked off), so
`WS_SIZEBOX` is the *only* thing that creates a grabbable border:

* Live style before the fix: `0x14CB0000` — **no `WS_SIZEBOX`**.
* There is no DOM resize handle anywhere in `src/` (no `resize` pointer handlers, no
  `sidebarWidth` writer except the unused `SettingsPatch` field).

Net effect: the user had no gesture that could change `sidebarWidth`, so it stayed
frozen at whatever had last been stored. This is a genuine release-blocking UX defect
for the v1 default mode, not a CompositionController or Wry defect.

`git log -S` shows the gate was written that way in the initial product commit
(`ef36e5d`), so this is a long-standing defect rather than a Phase 7C.3b regression —
it was simply never exercised.

### Fix (smallest root cause)

Sidebar becomes resizable; every other presentation keeps its existing rule.

* `src-tauri/src/product_window.rs` — extracted `product_window_resizable(mode, settings)`
  and used it in place of the inline expression. Sidebar → `true`; Floating → expanded
  && !locked; Desktop → `false`.
* `src-tauri/src/product_commands.rs` — `lock.toggle` recomputes `resizable` the same
  way, so locking position in Sidebar mode no longer strips the resize border. The lock
  keeps guarding free repositioning; the sidebar is always edge-anchored and the
  Resized handler persists its width regardless of lock state.

No new features, no redesign, no architecture change. The frontend is untouched — the
resize border is the same native mechanism Floating Expanded already used, and the
`Resized` handler that consumes it already existed.

### Verification (real mouse drags, not simulated geometry)

Driving actual `LEFTDOWN` → stepped `SetCursorPos` → `LEFTUP` through the window
manager's modal sizing loop:

| Check | Result |
| --- | --- |
| `WS_SIZEBOX` present at startup | **PASS** — style `0x14CF0000` |
| Press point hit-tests as a resize edge | **PASS** — `WM_NCHITTEST` → `HTLEFT` (and `HTRIGHT` on the other edge) |
| Drag narrower from 560 DIP | **PASS** — 862 → 682 px; persisted `sidebarWidth` 560 → **440** DIP |
| Continuous change | **PASS** — incremental throughout the drag |
| Persist new width | **PASS** — SQLite `sidebarWidth` updated; `round(682 / 1.5) = 440`, an exact match |
| Min clamp | **PASS** — drag far inward → client 480 px, persisted **320** DIP |
| Max clamp | **PASS** — drag far outward → persisted **560** DIP |
| Restart restores resized width | **PASS** — after 400 DIP + restart: outer 862×1541 px, client 840×1528 px = 560 DIP |
| Standard backend | **PASS** — all of the above |
| Enhanced backend | **Not run live** — bounded by inspection, see §5 |
| Desktop geometry unaffected | **PASS** — Desktop startup reparents under `Progman`, webview 630×1050 px, persisted desktop rect 1287,32 420×700 DIP, no panic |
| No `WM_SIZE` collapse regression (7C.2) | **PASS** — not reproduced; the sidebar resized cleanly across many `WM_SIZE` events |
| Sidebar content/hover after the change | **PASS** — full UI renders (date, weather, tasks, add-submit, footer), Acrylic intact, no clipping |

### Known residual (documented, deliberately not fixed)

The OS lets the user drag the edge **past** the 320/560 DIP clamp. The window reached
1380 px (905 DIP) while persistence correctly stored 560; the request is re-clamped on
the next mode transition or restart. This is a pre-existing consequence of clamping at
persist time rather than constraining the live drag, it is self-correcting, and fixing
it would mean adding runtime `SetMaxSize`/`SetMinSize` enforcement mid-drag — which is
outside the smallest root-cause fix and risks reintroducing the 7C.2 `WM_SIZE` sizing
path. Recorded as cosmetic, not release-blocking.

---

## 5. Standard / Enhanced comparison

| Behaviour | Standard | Enhanced | Basis |
| --- | --- | --- | --- |
| Orb render / collapse / expand / click / right-click | PASS | same | `RenderingBackend` selects WebView hosting only (`lib.rs:79-89`); `floatingPresentation` handling is backend-independent |
| Sidebar width clamp + default | identical | identical | `apply_sidebar_rect` / `normalize` take no backend input |
| Sidebar resize border | PASS (live) | same expected | `set_resizable` → tao window flags, not a WebView-hosting API |
| `Resized` → persist chain | PASS (live) | same expected | `handle_window_event` is tao-level; Wry's `WM_SIZE` subclass only resizes the *WebView child*, and its composition guard does not gate the tao `Resized` event |
| Acrylic | PASS | PASS (already proven in 7C.2) | unchanged by this work |

Enhanced was **not** driven live in this pass: it requires a persisted backend change
plus an app restart, i.e. a second full QA cycle, and the resize path is provably
backend-independent at the tao layer. This is a bounded limitation, not an untested
assumption about the code under change (which is one boolean and its call site).

---

## 6. Persisted-setting behaviour (Task 4)

| Question | Answer |
| --- | --- |
| Current intended default `sidebarWidth` | **380 DIP** (`ProductSettings::default`) |
| Was 560 user/test-persisted state? | **Yes** — a deliberate prior selection, and it is the clamp maximum |
| Is a one-time migration justified? | **No.** 560 is a legal, in-range value. The schema, clamp, and DIP units are already consistent (`geometryUnitsVersion: 1`), and the value migrated correctly. |
| Action taken | **None.** No migration added, no persisted value overwritten. Per Task 4, user choice is preserved; only *reachability* of the resize gesture was restored. |

The existing `geometryUnitsVersion` migration (`migrate_geometry_values_to_dip`) was left
untouched. Note for future QA: it also scales a legacy `sidebar_width` by the scale
factor, so a pre-version-1 row is converted once, not repeatedly.

---

## 7. Changed files

Product code (`git diff --stat`: **+77/−11** across 2 files — the bulk is the new
regression test and doc comments, not behaviour):

* `src-tauri/src/product_window.rs` — added `product_window_resizable()`
  (`product_window.rs:487`); Sidebar is now resizable; `apply_product_mode` uses the
  helper (`:671-673`); added the
  `sidebar_is_resizable_and_other_presentations_stay_fixed` regression test.
* `src-tauri/src/product_commands.rs` — `lock.toggle` keeps Sidebar resizable
  (`product_commands.rs:87-90`).

QA tooling (new, not product code): `scripts/rc0-qa/` — `rc0-window-probe.ps1`
(DPI-aware style/geometry probe), `rc0-settings.py` (parameterized settings read /
patch / restore), `rc0-uia-click.ps1`, `rc0-drag.ps1`, `rc0-click.ps1`,
`rc0-rightclick.ps1`, `rc0-capture.ps1`.

Not modified: `vendor/wry`, WinAppSDK runtime, `composition_host`, manifest/build
pipeline, backend default, UIA/accessibility implementation, release embedded frontend.

### Operator state

The operator's persisted settings were backed up before testing and restored
byte-identically afterwards, including `mode: "sidebar"`, `sidebarWidth: 560`,
`floatingPresentation: "expanded"`, the saved Orb anchor, weather location, appearance,
and profile fields. No task rows were touched.

---

## 8. Regression results (Task 5)

| Check | Result |
| --- | --- |
| `cargo check --manifest-path src-tauri/Cargo.toml` | **PASS** — clean, no warnings introduced |
| `cargo test --manifest-path src-tauri/Cargo.toml` | **PASS** — **72 passed; 0 failed** (71 pre-existing + 1 new) |
| `pnpm build` (`vue-tsc --noEmit && vite build`) | **PASS** — 34 modules, `index-DEZ9ydwR.js` 125.48 kB |
| Floating: Expanded → Collapse → Orb → Expanded | **PASS** |
| Sidebar: resize narrower → restart persistence | **PASS** |
| Sidebar: hover/content still works | **PASS** |
| Desktop: startup sanity | **PASS** |
| Standard: basic sanity | **PASS** |
| Enhanced: basic sanity | **Not run live** — bounded by inspection (§5) |

The full 7C suite was not re-run: no lower-level (Wry/WinAppSDK/composition) file
changed.

---

## 9. Remaining blockers

1. **Enhanced-backend live pass for sidebar resize** — low risk, backend-independent at
   the changed layer, but not observed on this pass. Recommended as a one-restart
   confirmation before shipping the release notes.
2. **Live-drag overshoot past the 320/560 clamp** — cosmetic and self-correcting on the
   next mode transition/restart; see §4.
3. Unrelated pre-existing items already tracked by 7C.3 (`build.rs` payload copy vs real
   packaging, WinAppSDK end-of-support policy, accessibility/UIA story) — untouched here.
4. The Desktop-mode todo add/drag defects noted in 7C.2 remain out of scope and were not
   re-opened.

No stop condition was triggered: no WindowMode redesign, no CompositionController/Wry
change, no schema migration, no Desktop/WorkerW involvement, no macOS/Linux change, and
no scope expansion.

---

## 10. Recommendation

**PROCEED.**

* The Orb is fully functional; the report was persisted state, and the documented route
  restores it. **Zero Orb code changes were made.**
* Sidebar width was at the intended maximum clamp, amplified by 150 % DPI. **No geometry
  bug.**
* Sidebar resize was genuinely broken and is fixed by one bounded, root-cause change that
  makes an already-written persistence path reachable, guarded by a new unit test and
  verified with real mouse drags end to end.
* `cargo check`, `cargo test` (72/72), and `pnpm build` all pass.

Recommended before release, as a separate short pass: confirm sidebar resize once on the
Enhanced backend, since that is the only claim here made by inspection rather than
observation.
