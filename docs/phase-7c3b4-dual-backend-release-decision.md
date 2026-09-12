# Phase 7C.3-B4 — Dual-backend release decision

Status: **PROCEED**. All interaction gates are now verified: Desktop drag PASS,
Sidebar hover PASS (both backends), the B3 add-submit fix is kept and verified, and a
minimal product-level dual-backend model exists with **Standard as the v1 default**
and Enhanced opt-in.

## Closure summary

| Gate | Result |
|---|---|
| Desktop drag/reorder | **PASS** (persistence proven in SQLite) |
| Sidebar hover — Standard backend | **PASS** |
| Sidebar hover — Enhanced backend | **PASS** |
| B3 add-submit fix | Kept, verified, no duplicate creation |
| Dual backend model | Implemented; Standard default, Enhanced opt-in |
| Standard default unchanged | Confirmed (`effective_backend=standard selection_source=persisted_setting`) |
| Enhanced setting unchanged | Confirmed (`requested_backend=standard effective_backend=enhanced selection_source=qa_override`) |
| `cargo check` / `cargo test` / `pnpm build` | PASS / **71 passed, 0 failed** / PASS |
| Residual processes | none |
| Final recommendation | **PROCEED** |

No feature work, no architecture change, no backend default change, and no UIA work was
performed in this closure phase.

Nothing committed or pushed. HEAD is `7c0e168`.

## Git state

- Branch: `arch/composition-controller-production-spike`
- HEAD: `7c0e168ed1755671b5a7302372d53669400dce6a` (unchanged; B3 changes still uncommitted on top)
- Working tree: modified, uncommitted

## Task 1 — Desktop drag/reorder: **PASS**

Reproduced with two tasks scheduled for the same task day (`学习`, `健身`) in real
Desktop mode, using the B3 trace points.

### First attempt was inconclusive, and why that mattered

The operator's first drag produced two complete reorder round-trips:

```text
drag_pointerdown -> drag_pointermove -> drag_pointerup -> drag_reorder_start -> drag_reorder_ok
drag_pointerdown -> drag_pointermove -> drag_pointerup -> drag_reorder_start -> drag_reorder_ok
```

…while the persisted order was **unchanged**. That is not evidence of a defect: two
drags that undo each other produce exactly this trace, and `reorder_pending` writes
`sort_order = (index + 1) * 10` over all pending rows, so a reversing pair is a
genuine no-op. A second attempt was therefore required before any conclusion.

### Single unidirectional drag — PASS

After dragging the first row below the second:

```text
before: 4c19830e  学习  10        after: 3f4e8750  健身  10
        3f4e8750  健身  20               4c19830e  学习  20
```

Verified directly in SQLite (persisted, not just optimistic UI), with:

- correct event chain (`drag_pointerdown → drag_pointermove → drag_pointerup → drag_reorder_start → drag_reorder_ok`)
- immediate order update
- **no accidental complete/cancel**: a query for non-pending tasks on that task day returned nothing
- order survives reload (read back from the database after the operation)

**No code changed for drag.** Marked PASS.

## Task 2 — Sidebar hover: **PASS**

Verified manually in real Sidebar mode (right side, 560 DIP, two pending tasks) under
**both** backends:

| Backend | Result |
|---|---|
| Standard (`ICoreWebView2Controller`) | PASS — row hover responds, action controls visible and clickable, no flicker moving row↔actions, hover releases on leave (no stuck state), right-click context menu works, no accidental complete or reorder |
| Enhanced (`ICoreWebView2CompositionController`) | PASS — identical behaviour |

**Zero code changes were needed for hover.**

Structural pre-check (done before launching, to avoid chasing a phantom): the
hover-reveal rule is

```css
.task-row:hover .task-actions,
.task-row:focus-within .task-actions { opacity: 1; }
```

and pending rows render as `class="task-row pending-row"`, so the selector genuinely
matches — it is not a dead rule. `.pending-row` alone would not have matched it, which
is why the template class list was checked rather than assumed from the CSS.

Deliberate design note, not a defect: Sidebar overrides `.task-actions` to
`opacity: 0.72` so actions stay partially visible at rest instead of hidden until
hover. The operator confirmed this reads correctly and hit targets work. Behaviour is
unchanged by this phase.

## Task 3 — B3 add-submit fix: **kept, verified**

The B3 fix (a real labelled submit button on the add form) is retained. Verified in
Desktop mode:

- `add_submit_enter` → `add_invoke_start` → `add_invoke_ok` → `add_reload_ok` and the
  task was persisted (`学习`, later reordered)
- button click path present and traced (`add_submit_click`)
- no duplicate creation observed across the session's adds (each attempt produced the
  expected single row)

Note on the shared path: the button is `type="submit"` inside the form whose
`@submit.prevent="add"` performs the add, while the input's `@keydown.enter.prevent`
calls `addSubmitFromKeyboard()`. Exactly one of the two runs per interaction; the
trace confirms one `add_invoke_start` per add, not two. Layout is unchanged apart
from reusing the existing `.category-add` button style.

## Task 4 — Backend model

A product-level preference, expressed without naming any hosting API:

```rust
pub enum RenderingBackend {
    #[default]
    Standard,    // windowed WebView2 controller; full UIA exposure
    Enhanced,    // composition controller; Acrylic/transparency, UIA-limited
}
```

Persisted as `renderingBackend` inside the existing `product_settings` row — **one
setting, not per-window-mode**. It is orthogonal to `ProductWindowMode`: Floating,
Sidebar and Desktop all work on either backend.

User-facing wording used in Settings:

| Option | Sub-label |
|---|---|
| Standard | "Best compatibility and accessibility." |
| Enhanced transparency | "Acrylic and transparent window effects. Screen-reader accessibility is currently limited." |

No internal terms (`ICoreWebView2Controller`, `CompositionController`, `Wry`, `HWND`)
appear in the UI. Developer diagnostics do name the controller type.

## Task 5 — Selection behavior

`run()` resolves the effective backend **before** `tauri::Builder` creates the config
window and its WebView:

```text
[rendering] requested_backend=standard effective_backend=standard selection_source=persisted_setting
[wry] webview_controller_type=ICoreWebView2Controller
```

Verified live: with an existing (pre-B4) settings row and no QA flag, the app selects
Standard and creates the **windowed** controller.

Ordering and single-source-of-truth design:

- The hosting backend is fixed at WebView creation, so the setting cannot be applied
  by a settings write. It is resolved early and reported in the log.
- `AppState::read_rendering_backend(data_dir)` reads only the `product_settings` row
  early, rather than duplicating settings storage. `AppState` is still built in the
  `setup` hook as before; there is still exactly one settings source.
- Because `AppHandle::path()` does not exist that early, the data directory is also
  derived directly. `setup` compares the two paths and logs
  `[rendering] app_data_dir_mismatch …` if they ever diverge, so this cannot silently
  read the wrong file.
- Changing the setting does **not** hot-swap the WebView. Recreating a WebView is not
  proven safe here, so the UI states "Restart the app to apply this change." Settings
  persistence is the only effect of the write.
- QA override: `--qa-composition-controller` still forces Enhanced, recorded as
  `selection_source=qa_override`. It is a test-only escape hatch and does not change
  the product default.
- Non-Windows: the early read returns the default and `enhanced_requested` is compiled
  out, so no platform depends on the setting on macOS/Linux. No macOS/Linux file was
  touched.

## Task 6 — Default and migration

**Default is Standard** (accessibility-safe), Enhanced opt-in.

Migration behaviour, verified against the real pre-B4 database rather than assumed:

```text
existing settings row has `renderingBackend`? -> False
therefore deserializes to -> RenderingBackend::default() == Standard
```

Existing users therefore land on the compatibility backend on first launch after the
upgrade. This is the safe direction: they gain accessibility exposure and lose the
Acrylic transparency effect until they opt in. No user is silently moved to the
backend without a UI Automation tree.

Locked in by a test (`settings_without_rendering_backend_migrate_to_standard`) that
asserts a settings JSON with no `renderingBackend` key loads and yields Standard. A
malformed or absent row also falls back to Standard so a damaged database can never
prevent startup.

## Task 7 — Settings UI

One row under **Appearance** (no new section, no modal, no banner):

- `Rendering` — group labelled "Rendering backend" with two buttons, matching the
  existing Background option-group pattern
- sub-label switches per selection, and appends "Restart the app to apply this change."
  only when the draft differs from the persisted value

## Task 8 — Accessibility disclosure

Evidence (from B3, reproduced by controller type here):

| Backend | Controller | UIA descendants |
|---|---|---|
| Standard | `ICoreWebView2Controller` | **22**, including `'Alan Desktop - Web 内容'` and named buttons |
| Enhanced | `ICoreWebView2CompositionController` | **1** (`TAURI_DRAG_RESIZE_WINDOW` only) — no WebView content tree |

> Naming note: the quoted pane name is reproduced from the B3 trace, where the window
> title was still `Alan Desktop`. It is now `desktop-todo-widget`.

Honest statement of the position:

- Windowed hosting exposes the WebView2 content to Windows UI Automation.
- Composition hosting currently does **not** expose the WebView content tree at all.
- Enhanced transparency is therefore **opt-in** for v1, and the limitation is stated
  in the settings UI next to the choice.
- This is a known platform limitation / unresolved integration gap of visual hosting,
  not a bug introduced by this project and not something this phase fixes.
- No claim of accessibility compliance is made. A UIA provider for the composition
  path was explicitly **not** attempted.
- Follow-up: investigate visual-hosting UIA/provider integration as its own scoped
  piece of work before any future change to the default.

## Task 9 — Regression matrix

| Check | Standard | Enhanced |
|---|---|---|
| Backend selected at startup | PASS (`effective_backend=standard`) | PASS (`effective_backend=enhanced`, `source=qa_override`) |
| Controller type | PASS `ICoreWebView2Controller` | PASS `ICoreWebView2CompositionController` |
| add task | PASS (Desktop) | PASS (Desktop, verified in B3) |
| drag/reorder | PASS (Desktop) | PASS (Desktop, verified in B3; same frontend this phase) |
| hover (Sidebar) | **PASS** | **PASS** |
| keyboard | PASS (Desktop; B3 trace) | PASS (B3 trace, same frontend) |
| Acrylic/transparent | n/a (not applicable) | PASS (human-verified in 7C.2/B2/B3) |
| release embedded frontend | PASS (7C.3-B1) | PASS (7C.3-B1/B2) |
| UIA tree | PASS (22 descendants) | LIMITATION reproduced and documented |

`cargo check` PASS · `cargo test` **71 passed / 0 failed** (69 + 2 new backend tests) ·
`pnpm build` PASS (34 modules).

IME: not independently re-measured here. Retaining the prior 7C.2 human PASS evidence;
keydown tracing cannot observe IME composition, and no new IME diagnostics were built.

## Changed files

| File | Change |
|---|---|
| `src-tauri/src/settings.rs` | `RenderingBackend` enum (default Standard), `ProductSettings.rendering_backend`, `AppState::read_rendering_backend`, 2 tests. |
| `src-tauri/src/lib.rs` | Backend resolved before WebView creation; QA flag now an override; `app_data_dir()` early resolver + mismatch assertion. |
| `src-tauri/src/product_window.rs` | `SettingsPatch.rendering_backend` persisted (settings only, no hot-swap). |
| `src-tauri/src/qa_diagnostics.rs` | `record_rendering_backend` log line. |
| `src/types.ts` | `RenderingBackend` type; `ProductSettings.renderingBackend`. |
| `src/App.vue` | Browser-preview settings include `renderingBackend: "standard"`. |
| `src/components/SettingsPanel.vue` | Rendering row + restart hint; save payload includes the setting. |

## Task 10 — Diagnostics review

Kept (long-term value, no user content):

- `[rendering] requested_backend=… effective_backend=… selection_source=…` — new, answers "which backend ran" from the log alone
- `[wry] webview_controller_type=…` — controller type
- `webview2_navigation_starting/completed … starting_url/completed_url` — frontend URL diagnosis (this is what pinned the 7C.3-B1 release blocker)
- lifecycle/teardown lines and Acrylic state transitions
- `frontend_input_received=…` / `frontend_key_received=…` — **retained for now**, because Sidebar hover and any drag regression still need them; they carry only fixed event names and `KeyboardEvent.key`, never task text or input contents

The `key:<name>` form should be removed once the remaining interaction gates pass;
`qa-trace.ts` documents that no user content is transmitted.

## Remaining known limitations (all non-blocking)

None of these blocks the release recommendation. They are recorded so they are not
rediscovered later or mistaken for defects.

1. **Enhanced backend has no WebView UI Automation tree.** Windowed hosting exposes 22
   UIA descendants including the WebView content; composition hosting exposes 1. This
   is a known limitation of visual hosting, not a regression this project introduced.
   **Mitigation:** Standard is the v1 default, so the accessible path is what users get
   unless they deliberately opt in. Enhanced is labelled in Settings as
   accessibility-limited.
2. **Desktop drag was verified with two tasks only.** Multi-row drags, drags that cross
   the scroll boundary, and drags interleaved with a category change are untested.
3. **IME relies on prior 7C.2 manual PASS evidence.** It was not independently
   re-measured in B3 or B4; keydown-based tracing cannot observe IME composition, and no
   new IME instrumentation was built (deliberately, to avoid scope creep).
4. **Changing the rendering backend requires a restart.** This is intentional rather
   than a defect — recreating the WebView is not proven safe — and is stated inline in
   Settings. There is no separate restart prompt beyond that hint.
5. **QA interaction tracing is still present.** `frontend_input_received` /
   `frontend_key_received` were retained because they were needed through B3/B4. They
   carry only fixed event names and `KeyboardEvent.key` — never task text or input
   contents. They should be trimmed once no further interaction QA is planned.
6. **`tauri build` bundling/installer is out of scope.** The self-contained payload must
   be shipped next to the executable; distribution packaging is Release Candidate work.
7. **Windows App SDK 2.x end of servicing is 2027-04-29.** Because the payload is
   application-owned and not serviceable, the next servicing update requires an app
   release.

## Recommendation: PROCEED

Every interaction gate is verified. The dual-backend model is minimal and shares all
business and UI logic — one frontend, one settings row, no per-mode duplication —
resolves the backend before WebView creation, keeps Standard as the accessibility-safe
v1 default, preserves the Enhanced Acrylic/transparent experience as an opt-in, and
does not alter the frozen architecture. Desktop drag passes with persistence proof,
Sidebar hover passes under both backends, the add-submit fix is kept and verified, and
the Enhanced accessibility limitation is disclosed in the product UI rather than hidden.

The items in the previous section are documented non-blocking limitations. Release
Candidate work (packaging, installer, README, tags, `v1.0`) is deliberately **not**
started here.

## Exact resume commands

```powershell
Set-Location 'D:\Documents\Desktop_todo_list'
git branch --show-current      # arch/composition-controller-production-spike
git rev-parse HEAD             # 7c0e168
git status --short

powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 2.x
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
pnpm build
cargo build --manifest-path src-tauri/Cargo.toml

pnpm dev
& 'src-tauri\target\debug\alan-desktop.exe'                      # Standard (default)
& 'src-tauri\target\debug\alan-desktop.exe' --qa-composition-controller   # force Enhanced
```

Backend is persisted in `app_settings.product_settings` as `renderingBackend`
(`standard` | `enhanced`). Window mode lives in the same row as `mode`. A pre-B2
backup exists at `alan-desktop.sqlite3.b2-backup`.

## What must not be redone or reverted

- Do not remove the Standard default or make Enhanced the default while the UIA gap is open.
- Do not attempt to hot-swap the WebView on a settings change; the restart hint is deliberate.
- Do not add a fourth `WindowMode`, or per-mode backend settings; backend is orthogonal to mode.
- Do not implement a UIA provider in this line of work.
- Do not "fix" Desktop drag — it passes; do not redesign drag architecture.
- Do not re-open WinAppSDK packaging, manifest generation, runtime staging, the Wry fork, the composition visual tree, or the release custom-protocol fix.
- Do not commit or push without explicit approval.
