# Phase 7C.3-B3 — Product interaction regression cleanup

Status: **PROCEED WITH BLOCKERS**. The two deferred Desktop-mode product defects do
**not** reproduce — Desktop add and Desktop keyboard interaction work. However the
phase uncovered a real, reproducible **accessibility blocker specific to
CompositionController hosting**, which fails the Task 6 BLOCKER condition.

Nothing committed or pushed. HEAD remains `7c0e168`.

## Git state

- Branch: `arch/composition-controller-production-spike`
- HEAD before and after this work: `7c0e168ed1755671b5a7302372d53669400dce6a` (unchanged)
- Working tree: modified, uncommitted (see changed files below)

## Task 1 — Desktop add Todo: **does not reproduce; works**

### Reproduction

The reported symptom was "Desktop mode cannot add todo". Reproduced by setting the
product to `mode=desktop` and driving the real UI.

### What the trace proved

A bounded QA beacon (`qaTrace`, fixed vocabulary, no user content) was added to the
add path and the input field, then the operator clicked the input, typed, and pressed
Enter. Result:

```text
frontend_input_received=pointerdown
frontend_input_received=add_submit_click
frontend_key_received=x / u / e / x / i
frontend_key_received=Backspace   (x12)
frontend_key_received=Shift
frontend_key_received=Enter
frontend_input_received=add_invoke_start
frontend_input_received=add_submit_enter
frontend_input_received=add_invoke_ok
frontend_input_received=add_reload_ok
```

And persistence, verified directly in SQLite:

```text
4c19830e|学习|pending|2026-09-11|10
```

**Conclusion:** the "Desktop cannot add" report does not reproduce against the
current binary. Mouse, keyboard, IPC, command, persistence, and UI refresh all work
in Desktop mode.

### Contributing finding (fixed)

The add form had **no submit control at all**: the `＋` glyph is an
`aria-hidden` span, and the only way to create a task was pressing Enter inside the
field. When that keystroke is missed — WebView2 does occasionally drop the first
keystroke after focus moves into a control — the user has **no alternative and no
visible affordance**, which is consistent with the original report.

Minimal fix: one real, labelled submit button wired to the form's existing
`@submit` handler. No layout redesign, no styling invention (it reuses the existing
`.category-add` class with a small addition). This also gives the form a proper
accessible submit affordance, which it previously lacked.

## Task 2 — Desktop drag/reorder: **not re-verified**

Desktop drag was not re-tested. It cannot be reached until a task exists (it can now
be created), and the phase's context budget was spent on the accessibility blocker
below, which is a release-relevant finding.

Carried over from before the migration: the operator previously reported Desktop
drag failing while Floating drag worked. Drag in this product is **not** HTML5 DnD —
it is a custom pointer-capture handle with an explicit comment that WebView2 does not
reliably promote `draggable` rows. The relevant handlers and their new trace points
are in place (`drag_pointerdown` / `drag_pointermove` / `drag_pointerup` /
`drag_reorder_start|ok|err`), so a future run can classify it in one pass.

**This is the main outstanding functional gap.**

## Task 3 — Sidebar hover: **not verified**

Not tested this phase. The previous blocker (no task could be created, so nothing
existed to hover) is now removed. Row hover, action visibility, action hit targets,
stuck-hover, flicker, and context menu still need one interactive pass in Sidebar
mode.

## Task 4 — Keyboard / IME regression: **PASS for keyboard**

Verified in real Desktop mode via the key trace:

| Check | Result |
|---|---|
| ASCII typing | PASS (`x u e x i` received) |
| Backspace | PASS (12 events) |
| Enter / submit | PASS (created a task) |
| Arrow keys | PASS (`ArrowLeft`, `ArrowRight`) |
| Tab / focus movement | PASS (`Tab` reached, subsequent click returned focus) |
| Shift | PASS |
| Click focus | PASS (`add_submit_click` followed each click into the field) |
| Right-click / context menu | PASS (`contextmenu` event received) |

IME: the operator reported the Chinese IME test complete, but keydown-based tracing
is structurally unable to observe IME composition (committed text arrives through
composition events, not keydowns), so I cannot independently assert IME PASS from
this evidence. IME was human-verified as PASS in 7C.2 on the same input
architecture, and nothing in B3 changed the input path. Recorded as **carried-over
human evidence, not re-measured here**.

No deep input-architecture problem was exposed, so no stop condition triggered.

## Task 5 — Lifecycle sanity: **PASS**

One bounded cycle in Floating mode (a top-level window, so scriptable):

```text
minimize / restore            -> no black rectangle, window returns correctly
focus stolen then returned    -> lifecycle_event=Focused input_active=false
                                 lifecycle_event=Focused input_active=true
WM_CLOSE                      -> shutdown=targets-removed
                                 shutdown=subscription-removed
                                 shutdown=controller-closed
                                 shutdown=composition-released
                                 lifecycle=host-release-deferred-until-wry-drop
exit                          -> ~2 s
orphan processes              -> 0
```

Acrylic was `controller_state=active` on 2.x at startup in this run. Mode switching
(Floating ↔ Desktop) exercised earlier: Desktop attach succeeded onto
`SHELLDLL_DefView` with correct bounds, and detach was not broken.

**Note:** this is the first time graceful close was verified headlessly end to end.
Previous phases could not script it because Desktop mode hides the window from
top-level enumeration. It behaves correctly.

## Task 6 — Accessibility baseline: **BLOCKER (composition-hosting specific)**

Measured with the real UI Automation tree (`AutomationElement.FromHandle`, descendants),
comparing the two hosting modes on the same build and same frontend.

### Windowed hosting (`ICoreWebView2Controller`) — healthy

```text
descendants: 22
  [Pane]   name='TAURI_DRAG_RESIZE_WINDOW'
  [Pane]   name='Alan Desktop'
  [Pane]   name='Alan Desktop - Web 内容'      <- WebView2 content host exposed
  [Text]   name='无标题'
  [Button] name='最小化'  focusable=True
  [Button] name='最大化'  focusable=True
  [Button] name='关闭'    focusable=True
  ...
```

The WebView2 accessibility tree **is** exposed, controls carry names, and keyboard
focusable elements are reported.

### Composition hosting (`ICoreWebView2CompositionController`) — empty

```text
descendants: 1
  [Pane] name='TAURI_DRAG_RESIZE_WINDOW' focusable=False
```

Reproduced twice, including after the frontend had finished loading. MSAA
`AccessibleObjectFromWindow(OBJID_CLIENT)` also returns `childCount=3` with an empty
accessible name, versus a populated tree in windowed mode.

### Assessment

- WebView2 accessibility tree exposed: **NO in composition mode**, YES in windowed.
- Keyboard navigation reaches interactive controls: functionally YES (Tab and arrows
  work — verified in Task 4), but this is DOM-level focus, **not** UIA-visible.
- Obvious controls have accessible names: YES in the markup (`aria-label="New task"`,
  `aria-label="Add task"`, `aria-label="Complete task"`, etc.), but **none of it is
  reachable by assistive technology in composition mode**.
- CompositionController hosting introduces a known accessibility gap: **YES — and it
  is total for the content area.**

This matches Microsoft's documented caution that visual hosting
(`ICoreWebView2CompositionController`) does not provide the same
accessibility/HWND-based integration as windowed hosting. It is not a regression this
phase introduced; it is an inherent property of the hosting mode that was adopted in
7C.2, and it was never gated because composition accessibility was out of scope then.

### Why this is a BLOCKER and not LIMITED

For a screen-reader or automation user, a composition-hosted build presents a window
that contains **no accessible content at all**. That is not "incomplete names" or
"missing landmarks" — it is the absence of the entire content tree. Shipping
composition hosting as the default user-facing path would be an accessibility
regression relative to windowed hosting, which works today.

### Not fixed here (correctly)

Fixing this requires exposing the WebView2 content through UIA in visual-hosting
mode, which is a platform/architecture task (documented approaches involve hosting
the content in a way that participates in UIA, or bridging the content's automation
providers). That is explicitly out of scope for B3 and trips the "accessibility
requires a large subsystem" stop condition. **No hack was attempted.**

## Changed files

| File | Change |
|---|---|
| `src/qa-trace.ts` | **New.** Bounded frontend QA beacon; fixed vocabulary, fire-and-forget, no-op outside Tauri, never sends user text. |
| `src/components/TodoPanel.vue` | Added trace calls on the add and drag paths; added the missing labelled submit button; extracted `addSubmitFromKeyboard` so the key path is separately traceable. |
| `src-tauri/src/qa_diagnostics.rs` | Extended `qa_frontend_input` with the fixed B3 vocabulary plus a `key:<name>` form carrying only `KeyboardEvent.key`. |

`qa_frontend_input`'s doc comment states explicitly that it must never carry task
titles, ids, or input contents; unknown values collapse to `other`.

## Build / test results

```text
cargo check --manifest-path src-tauri/Cargo.toml   PASS
cargo test  --manifest-path src-tauri/Cargo.toml   PASS — 69 passed, 0 failed
pnpm build                                          PASS — 34 modules (was 33; +qa-trace)
debug composition launcher                          PASS — ICoreWebView2CompositionController
debug windowed launcher                             PASS — ICoreWebView2Controller
release composition / release windowed              PASS in 7C.3-B2; not re-run this phase
```

Runtime paths exercised this phase: Desktop mode (add + input), Floating mode
(lifecycle + Acrylic active), composition hosting, windowed hosting.

## Remaining blockers

1. **Accessibility: composition hosting exposes no UIA content tree** (above).
   Release-facing decision required: ship windowed hosting for accessibility, gate
   composition hosting behind opt-in, or fund the UIA work.
2. **Desktop drag/reorder not re-verified.** Trace points are in place.
3. **Sidebar hover not verified.** Requires one interactive pass.
4. **IME not independently re-measured** (carried-over 7C.2 human evidence).
5. The original "Desktop cannot add" report does not reproduce; the missing submit
   affordance was the only concrete defect found and is fixed. If the reporter can
   still reproduce it, the next step is to capture `frontend_key_received=*` at the
   moment of failure with the tracing already in place.

## Recommendation: PROCEED WITH BLOCKERS

The Desktop add regression is closed (works, plus a real affordance fix), keyboard
input in Desktop mode is verified, and lifecycle is clean. The blockers are the
composition-hosting accessibility gap and two unverified interaction items. None
requires changing the frozen CompositionController architecture to *diagnose*; the
accessibility gap does require an architecture decision to *fix*.

## Exact resume commands

```powershell
Set-Location 'D:\Documents\Desktop_todo_list'
git branch --show-current      # arch/composition-controller-production-spike
git rev-parse HEAD             # 7c0e168

powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 2.x
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
pnpm build
cargo build --manifest-path src-tauri/Cargo.toml

pnpm dev                                                    # keep Vite on :1420
& 'src-tauri\target\debug\alan-desktop.exe' --qa-composition-controller
& 'src-tauri\target\debug\alan-desktop.exe'                 # windowed

# QA traces land in the run's stderr / phase7b-qa-material-on.log:
#   frontend_input_received=... / frontend_key_received=...
```

Product mode for testing is persisted in
`%APPDATA%\net.alanfloyd.desktop\alan-desktop.sqlite3` (`app_settings.product_settings`,
`"mode"` = `floating` | `sidebar` | `desktop`). A pre-change backup exists at
`alan-desktop.sqlite3.b2-backup`.

## What must not be redone or reverted

- Do not "re-fix" Desktop add by changing the task model or DB schema; it works, and
  the only change was a missing submit affordance in the form markup.
- Do not treat the accessibility gap as a CompositionController bug to patch with a
  hack; it needs a scoped design decision.
- Do not remove `src/qa-trace.ts` or the QA vocabulary before the drag/hover passes
  are done; they are the instrumentation for the remaining items.
- Do not resume Windows App SDK packaging, manifest generation, runtime selection, or
  the vendored Wry design from this phase.
- Do not commit or push without explicit approval.
