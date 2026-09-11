# Phase 7C.3-B1 — release embedded-frontend navigation blocker

Status: **fixed and verified**. The release build now serves the real Vue frontend
through the embedded asset protocol, in both windowed and composition hosting. The
dev-server path is unchanged. No commit or push was made.

## Symptom

| Run | Before |
|---|---|
| debug composition | PASS — real Vue, `frontend_ready=true` |
| debug windowed | PASS |
| release composition | FAIL — `success=false`, no UI, `frontend_ready` never fired |
| release windowed | FAIL — identical |

`frontend_asset_mode=embedded` was reported, WebView2 controller creation succeeded,
the WinAppSDK payload loaded, and the manifest was valid — so the failure looked like
an asset-protocol or resolver problem.

## What the added diagnostics showed

Instrumentation (see "Evidence" below) recorded the initial source URL, the
`NavigationStarting` target, the completed URL, `web_error_status`, and every
custom-protocol request with its resolved path, MIME type and body size.

Decisive output from the release build:

```text
webview2_source_url_at_setup=about:blank
webview2_navigation_starting=true starting_url=http://localhost:1420/
webview2_navigation_completed=true success=false web_error_status=0 completed_url=http://localhost:1420/
```

**The release binary was navigating to `http://localhost:1420/` — the Vite dev
server — not to the embedded asset URL.** With Vite not running, the load simply
failed. There was no asset-protocol, resolver, MIME, or CSP defect: the embedded
asset path was never used at all.

That also explains why `frontend_asset_mode=embedded` was misleading: it was derived
from `cfg!(debug_assertions)`, which is about the *profile*, while Tauri's URL choice
is driven by something else entirely.

## Root cause

Tauri's own build script decides dev-vs-production from a **cargo feature**, not from
the cargo profile (`tauri-2.11.5/build.rs`):

```rust
fn main() {
  let custom_protocol = has_feature("custom-protocol");
  let dev = !custom_protocol;
  alias("custom_protocol", custom_protocol);
  alias("dev", dev);
  ...
}
```

and Tauri then selects the initial URL from that flag
(`tauri-2.11.5/src/manager/mod.rs`):

```rust
pub(crate) fn get_app_url(&self, https: bool) -> Cow<'_, Url> {
    #[cfg(dev)]
    let url = self.config.build.dev_url.as_ref();          // http://localhost:1420
    #[cfg(not(dev))]
    let url = match self.config.build.frontend_dist.as_ref() { ... };  // tauri://localhost
    ...
}
```

So **any build without `--features custom-protocol` is a dev build, even
`cargo build --release`.** `tauri build` supplies the feature; this repository was
building releases with a bare `cargo build --release`, which silently produced a
release-profile binary that still pointed at the dev server.

This is the accurate explanation for the previously recorded 7C.2 observation that
"a bare debug exe failed because debug points at the dev server" — the distinction is
the feature, not the profile.

## Fix

`src-tauri/Cargo.toml` now declares the feature explicitly and — importantly — **not**
as a default:

```toml
[features]
# Tauri's build script derives its whole dev/production split from this feature:
#     let custom_protocol = has_feature("custom-protocol");
#     let dev = !custom_protocol;
# so a build without it is a *dev* build no matter which cargo profile was used.
#
# This must stay NOT default. Cargo features are profile-agnostic, so making it
# default would also flip `tauri dev` onto the embedded assets and break the dev
# server / HMR path.
custom-protocol = ["tauri/custom-protocol"]
```

Release builds must therefore be invoked as:

```powershell
cargo build --release --features custom-protocol
# or, equivalently and preferred:
pnpm tauri build --no-bundle
```

### Why it is not a default feature (a wrong turn worth recording)

Making `custom-protocol` a **default** feature was tried first. It fixed release, but
verification caught that it also flipped the **debug** build onto the embedded
assets:

```text
# debug build with default = ["custom-protocol"]
webview2_navigation_starting=true starting_url=http://tauri.localhost/
```

That silently disables the Vite dev server and HMR, so it was reverted. Cargo features
are profile-agnostic and cannot express "release only". Enabling the dependency feature
from `build.rs` is not possible either: a build script may only emit
`cargo:rustc-cfg` for its own package and cannot enable `tauri/custom-protocol`.
Explicit invocation, matching what `tauri build` does, is the correct mechanism.

### New guard against silent recurrence

Because a missing feature produces a binary that *builds fine and renders nothing*,
this failure mode is now loud at startup (`qa_diagnostics::warn_if_release_without_embedded_frontend`):

```text
[frontend] WARNING release_profile_without_custom_protocol feature=true \
  frontend_asset_mode=dev-server \
  this binary will navigate to build.devUrl and render nothing without a dev server; \
  build with: cargo build --release --features custom-protocol
```

Verified: an optimized build with the feature omitted emits exactly this warning and
then navigates to `http://localhost:1420/`, confirming both the detection and the
diagnosis.

`frontend_asset_mode()` was also corrected to report `cfg!(dev)` instead of
`cfg!(debug_assertions)`. It previously described the profile, which is why it claimed
"embedded" for a binary that was actually using the dev URL. It now describes the real
frontend path.

## Evidence

Requested diagnostic set, captured from the release build after the fix:

```text
webview2_source_url_at_setup=about:blank
webview2_navigation_starting=true starting_url=http://tauri.localhost/ already_cancelled_by_other_handler=false
custom_protocol_hit=true requested_uri=http://tauri.localhost/ protocol=tauri
resolver_request_path=tauri://localhost/ method=GET
resolver_response status=200 OK mime=Some("text/html") body_bytes=430
custom_protocol_hit=true requested_uri=http://tauri.localhost/assets/index-BO1pZLcL.js protocol=tauri
resolver_response status=200 OK mime=Some("text/javascript") body_bytes=123978
custom_protocol_hit=true requested_uri=http://tauri.localhost/assets/index-Bbw2Ecz7.css protocol=tauri
resolver_response status=200 OK mime=Some("text/css") body_bytes=22007
webview2_navigation_completed=true success=true web_error_status=0 completed_url=http://tauri.localhost/
custom_protocol_hit=true requested_uri=http://ipc.localhost/today_tasks protocol=ipc
custom_protocol_hit=true requested_uri=http://ipc.localhost/product_state protocol=ipc
frontend_ready=true
```

`index.html` and both JS/CSS chunks resolve with correct MIME types, and IPC follows.
The `dbg-asset` request/response tracing was removed after diagnosis; the
`starting_url` / `completed_url` / `source_url` records were kept, because URL
mix-ups are otherwise invisible (navigation just fails and the window stays blank).

## Gates

| Gate | Result |
|---|---|
| release windowed loads the real Vue frontend | **PASS** — `http://tauri.localhost/`, `success=true`, `frontend_ready=true` |
| release composition loads the real Vue frontend | **PASS** — `ICoreWebView2CompositionController`, `http://tauri.localhost/`, `success=true`, `frontend_ready=true` |
| both use the same production embedded asset path | **PASS** — both `tauri://localhost` with `frontend_asset_mode=embedded` |
| `pnpm build` not regressed | PASS — 33 modules |
| `cargo test` not regressed | PASS — 69 passed, 0 failed |
| debug path not regressed | PASS — debug composition still `http://localhost:1420/` (dev server), `frontend_ready=true` |
| release artifact starts from a clean directory outside the repo | PASS — exe + 46 runtime files, no orphans after exit |

Release builds were additionally verified after removing all temporary instrumentation.

## Files changed in this phase

| File | Change |
|---|---|
| `src-tauri/Cargo.toml` | `[features] custom-protocol = ["tauri/custom-protocol"]`, deliberately not default, with the reasoning recorded inline. |
| `src-tauri/src/qa_diagnostics.rs` | `NavigationStarting` + completion/source URL diagnostics (kept), `warn_if_release_without_embedded_frontend` guard (new), `frontend_asset_mode()` now derived from `cfg!(dev)`. |
| `src-tauri/src/lib.rs` | Calls the new guard at startup. |
| `vendor/wry/src/webview2/mod.rs` | Explicit response type annotation on the custom-protocol responder (semantics unchanged); temporary request/response tracing added and removed. |

## Scope respected

- WinAppSDK version untouched (still 1.8; the 2.x upgrade remains a separate task).
- No composition-architecture, Vue UI, Desktop-mode, DB, or domain changes.
- No Tauri/Wry version change.
- No external HTTP server used to bypass embedded assets — the release build now uses
  the genuine production path.
- The blocker was **not** in Tauri/Wry asset-protocol internals, so no patch there was
  needed; the defect was build configuration.

## Exact commands

```powershell
# debug / development (dev server on :1420 must be running)
pnpm dev
cargo build --manifest-path src-tauri/Cargo.toml

# release (embedded frontend)
pnpm build
cargo build --release --manifest-path src-tauri/Cargo.toml --features custom-protocol

# regression
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
pnpm build
```

## Do not redo or roll back

- Do not make `custom-protocol` a default feature; it breaks the dev-server path.
- Do not "fix" a blank release window by pointing it at the dev server.
- Do not remove the startup warning or the navigation-URL diagnostics; they are the
  only signal for this class of failure.
- Do not revert `frontend_asset_mode()` to `cfg!(debug_assertions)`.
- Do not start the WinAppSDK 2.x upgrade as part of this line of work.
- Do not commit, push, reset, clean, checkout-discard, or stash-drop.
