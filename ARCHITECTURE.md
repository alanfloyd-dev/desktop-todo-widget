# desktop-todo-widget architecture

The v1 module map. Product behaviour is described in [README.md](README.md); feature documentation lives in [docs/](docs/).

```text
Vue / TypeScript
  App + mode-specific layout presets + layered appearance material
  Todo presentation / read-only Review / product content / settings / developer diagnostics
                 │ Tauri commands + events
                 ▼
Rust product boundary
  product_commands.rs  shared widget + tray actions
  product_window.rs    mode orchestration, snap, visibility recovery
  settings.rs          typed settings and persistence
  database.rs          SQLite migration/repository boundary
  tasks.rs             task/category repository + transactional commands
  reviews.rs           dynamic task-history aggregation for Daily/Weekly/Monthly Review
  task_day.rs          centralized local rollover calculation
  weather.rs           Open-Meteo adapter, normalization, cache policy, refresh gate
  appearance.rs        validation, local managed assets, wallpaper read, contrast utility
  diagnostics.rs       privacy-allowlisted support report
                 │ thin adapter call
                 ▼
Frozen Phase 1 native boundary
  window_mode.rs       HWND, SetParent, styles, z-order, Shell hooks
  platform/windows/widget_frame.rs         taskbar/Alt+Tab exclusion (widget semantics)

Maintenance subsystem (v1.2 Phase 1 — implemented)
  maintenance_admission.rs  launch admission (Managed/Unmanaged/Development/
                            PostUpdateProbation), the frozen Q14 recovery
                            route for an Updating receipt, probation child
                            HealthAck emission, canonical-root helper
                            handoff, uninstall entry
  maintenance/              offline core + native helper crate (no Tauri/network/SQLite):
                            receipt codec, path policy, locking, install/uninstall
                            transactions, Windows integration reconciliation;
                            shared package-ZIP validation primitive — the single
                            implementation both binaries call (moved here in
                            27dbd55; main still owns acquisition/staging, the
                            helper only revalidates at handoff); frozen
                            UpdateSession closed parsing/publication; helper
                            --update handoff verification producing the
                            ValidatedHandoff boundary; runtime mutation
                            transaction (c8e7c03): live caller identity,
                            Gate/exclusive lease, Updating transition,
                            intent-before-side-effect backup/replace journal,
                            runner-based helper self-replacement, crash
                            reconciliation, recovery classification; the
                            transaction completed in b825d33: suspended
                            probation launch (CREATE_SUSPENDED → identity
                            capture → durable probationProcess → resume),
                            HealthAck verification and the typed probation
                            decision, commit and rollback under the frozen
                            write ordering, executed recovery, terminal
                            cleanup — ending in a durable committed or
                            rolled-back session; QA feature is compile-time
                            gated and absent from production binaries
  update-core/              pure signed-update protocol core shared by product and
                            helper (no Tauri/network/SQLite/Win32): envelope/
                            trust-store/canonical signature verification, manifest
                            parsing and validation, candidate policy (Phase 2B
                            verification core, implemented in df75c83); the
                            compiled production trust roots (production_keys.rs,
                            46d911b) — raw 32-byte Ed25519 public keys only, the
                            single shared trust source for both binaries,
                            validated fail-closed at construction, and currently
                            EMPTY until the real key ceremony
  release-signer/           operator-side offline release-signing tool (46d911b):
                            NOT a runtime updater component and never linked into
                            a product binary; manifest validation before signing,
                            exact raw manifest-byte Ed25519 signing, exact
                            compiled trust-store membership gate, self-verification
                            through the same compiled store, atomic output with
                            an explicit overwrite policy; key-id/provision-entry/
                            generate-keypair/package-facts helpers; no network, no
                            Tauri, no provider API (operator runbook:
                            docs/release-signing.md); the release pipeline
                            delegates all cryptographic signing to it — the
                            pipeline itself is never a trust authority
  release-pipeline/         operator-side release packaging + publishing tool
                            (712a7bb): NOT a runtime updater component and never
                            linked into a product binary; canonical nine-entry
                            package assembly from explicit per-entry sources with
                            immediate self-validation through the shared
                            maintenance package-ZIP validator (no second copy of
                            the ZIP security rules); release facts computed from
                            the final verified package bytes; a deterministic
                            compact-JSON schema-1 manifest generator whose exact
                            bytes pass validate_untrusted_manifest and are signed
                            and served unchanged; signing delegated to
                            release-signer — production mode through the unchanged
                            compiled production trust gate (fail-closed while the
                            store is empty), plus an explicit rehearsal mode
                            with non-production trust material that may touch
                            a production endpoint only through a release
                            stable discovery will not select (GitHub: draft,
                            invisible to discovery; Gitee: prerelease-marked,
                            the flag discovery's publication-eligibility step
                            filters on; finalize refused in rehearsal mode);
                            GitHub/Gitee publish orchestration with an explicit
                            collision/idempotence policy, byte-exact remote
                            read-back verification, and mirror consistency by
                            artifact digests; read-only dry-run and local
                            facts/report files (operator state, not protocol
                            artifacts); no private signing key ever enters this
                            tool — provider tokens are provider authentication
                            only; both provider rehearsal paths have been
                            verified live — GitHub draft (875ee32: draft
                            creation, exact-artifact upload, live same-byte
                            idempotence, authenticated remote read-back,
                            stable-discovery isolation, finalize refusal) and
                            Gitee prerelease (64cb4b0: real multipart
                            attach_files upload, live same-byte idempotence,
                            real remote read-back) — plus one real
                            cross-provider byte-equivalence rehearsal; the
                            Gitee rehearsal release is visible in the release
                            index (no draft state) and is held out of stable
                            discovery by its prerelease flag, its
                            below-production rehearsal version, and the empty
                            production trust store
  src/updater/ (main crate) provider/network/discovery transport (GitHub/Gitee/
                            Auto bounded enumeration and metadata fetch,
                            eace586) plus the durable trusted-target record,
                            bounded package acquisition and session-bound
                            staging of the two managed EXEs through the shared
                            ZIP primitive (c8a37ab), frozen UpdateSession
                            publication + frozen handoff command construction
                            for the canonical helper (27dbd55), and the
                            production install surface — the canonical helper
                            spawn boundary with the durable-handoff wait
                            (c8e7c03) and the install_ready_update(sessionId)
                            command triggering an already-trusted staged
                            update followed by the canonical app shutdown
                            (b825d33: the frontend supplies only the
                            session id; every authority is re-derived from
                            canonical state); disk-recovered trusted state is
                            re-authenticated through the shared core on every
                            recovery, never trusted because it was once
                            persisted; discovery semantics are unchanged by
                            the release pipeline — the updater only ever
                            consumes published artifacts and never becomes a
                            publisher; the maintenance helper keeps no
                            network, provider, download, or staging
                            responsibility
```

## Product and native separation

Vue owns presentation, layout presets, settings presentation, and interaction state. Rust owns OS paths, the window/tray lifecycle, persistence, and the Win32 boundary. The UI never manipulates HWND or SQLite directly.

`product_window.rs` is the formal three-mode state machine. It saves and restores product geometry, then delegates Desktop parenting and parent-client coordinate validation to the narrow `window_mode` adapter; the proven attach/detach implementation and existing WebView remain unchanged. The larger Phase 1 diagnostic command surface stays inside the native module and is only presented by the default-collapsed Developer panel.

All widget and tray actions use the same string command IDs and `dispatch_product_action` implementation, so check state and mutations cannot diverge between two menu implementations.

## Window modes

- **Floating:** top-level non-WorkerW window with `collapsed` and `expanded` presentation states. Collapsed is a 56 DIP Orb; expanded restores its independent width/height. Both use the same Tauri window and preserve topmost/lock state.
- **Sidebar:** top-level non-WorkerW window; uses the selected monitor work area, left/right edge, stored width, and full work-area height. Moving Floating within 24 physical pixels of an edge enters Sidebar.
- **Desktop:** a bounded, frameless Widget using the Phase 1 `SHELLDLL_DefView` child route; draggable and resizable while unlocked, with always-on-top forced off. Its parent-client geometry is persisted separately and clamped inside the current desktop host. Returning to Floating saves Desktop geometry, detaches through the validated Phase 1 path, and restores the independent Floating rectangle.

Lock disables drag initiation and native resizing. It does not enable click-through, so content, links, settings, and the context menu remain interactive.

## Rendering

The product has a single windowed WebView2 backend. Tauri/Wry create the ordinary windowed `ICoreWebView2Controller`, and window materials are owned entirely by the CSS material layers over a transparent window — no native window effect is applied in any mode. There is no user-selectable backend and no second hosting path.

## Persistence and migration

At startup, Tauri resolves the platform app-data directory and opens `alan-desktop.sqlite3` (an internal compatibility filename; the public product name is `desktop-todo-widget`). Migration 1 creates:

- `schema_migrations`
- `categories`
- `tasks`
- `shortcuts`
- `app_settings`

`tasks.status` is constrained to `pending`, `completed`, `cancelled`, or `carried`; the schema includes scheduled/completed timestamps, ordering, nullable category, and `carried_from`. Migration 3 adds only `weather_cache`; it does not modify task, category, shortcut, or settings rows. No migration was added after schema 3. `tasks.rs` owns validation and repository operations; Vue never issues SQL or derives the authoritative task day.

Carry is history-preserving: one transaction changes the original pending row to `carried`, then inserts a pending successor for the next task day with copied title/category and `carried_from` pointing to the original. Reordering validates that every pending row for the task day appears exactly once and updates all sort positions in one transaction.

The calendar date and task day are intentionally separate. The UI header uses the current calendar date; Rust derives the query day from local time and the configured rollover (04:00 by default). Before rollover, the current task day is the previous calendar date. Previous-day pending rows are only presented for review and are never mutated automatically.

`reviews.rs` is a read-only projection over raw `tasks` rows. Each request calculates Daily, Monday–Sunday Weekly, or calendar-month bounds, queries `scheduled_date` inside those bounds, and aggregates final task statuses plus task-day and category distributions in memory. `completed_at` and `carried_from` remain available as history facts; a carried source counts as `carried` on its own scheduled day, while its successor is an independent row on its successor day. No report snapshots, percentages, scores, or other derived values are persisted.

The typed product-settings document stored in `app_settings` contains mode, independent expanded-Floating/Desktop geometry, an independent Floating Orb anchor, monitor identity, sidebar state, lock/topmost flags, day rollover, language, normalized weather settings, per-mode appearance profiles, avatar asset ID, Quick Links, and profile fields. Product geometry is persisted in logical pixels (DIP). Missing fields merge centralized defaults, so no SQLite migration is required and schema 3 remains authoritative. Documents written by very old builds may still carry a rendering-backend field; it parses, normalizes to Standard, and is rewritten on the next startup.

Appearance is split into a transparent native/WebView surface, transparent DOM roots, background/backdrop/tint material layers, and a fully opaque content layer, with one stored profile per window mode. Every mode clears native window effects; the CSS graphite tint and its per-mode variants (the Orb's transparent tint, Desktop's translucent Graphite) carry the whole look. Desktop clears the effect before Phase 1 Shell reparenting, because a `SHELLDLL_DefView` child is not a top-level HWND and cannot host a native backdrop. Rust validates/clamps the settings, reads the Windows wallpaper once on demand, copies selected PNG/JPEG/WebP files into app data with generated IDs, and returns data URLs rather than private paths. Missing or corrupt assets resolve to safe transparent graphite Glass.

Auto contrast uses a centralized representative-luminance calculation. Solid colors and gradient stops are deterministic; image/wallpaper data are sampled once with a 32×32 browser canvas when loaded or changed. A small hysteresis band prevents threshold flicker. Manual Light and Dark remain explicit overrides.

Weather is an isolated optional capability. Vue first requests cache state and renders the product immediately; stale or missing configured data triggers a background command. `weather.rs` alone understands Open-Meteo JSON and WMO codes. It validates HTTP status and payload shape, normalizes errors, and writes only a `WeatherSnapshot` to SQLite. Cache lookup requires both the stable coordinate/timezone location key and temperature unit, so a changed setting cannot relabel another location's result. One backend atomic gate and one App-level hourly timer prevent mode transitions from creating duplicate refresh loops.

Public issue diagnostics use an explicit allowlist. They include runtime/window/schema metadata plus total task count and current task day, but never task titles, categories, history content, profile values, Quick Link URLs, weather location, or the precise database path.

## Startup order

0. **Maintenance admission** — resolve canonical roots, classify the launch
   (Managed / Unmanaged / Development), refuse broken managed state before any
   diagnostics write or database access, and hold the shared application lease
   until exit ([docs/application-lifecycle.md](docs/application-lifecycle.md)).
1. Resolve app-data and run idempotent SQLite migration.
2. Load or create the typed settings document (Floating + collapsed Orb + Glass defaults, day rollover 04:00).
3. Install the tray menu.
4. Restore the saved product mode and validated geometry.
5. Hydrate matching weather cache without waiting for the network; refresh stale configured data in the background.
6. Load managed appearance assets locally, resolve contrast, and fall back safely when unavailable.
7. Persist later move/resize, Orb anchor, presentation, mode, lock, and settings changes.

## Scope boundary

The v1.2.0 [Application Lifecycle & Maintenance Architecture](docs/application-lifecycle.md) defines install, update, recovery, receipts, Windows integration, and uninstall. Its Phase 1 subset — manual bootstrap with v1.1 adoption, the installation receipt, launch admission, Windows integration, and native uninstall with keep/remove local-data semantics — is implemented and verified; the Phase 2 signed updater's core transaction engine is implemented end to end: the verification core (canonical signed bytes, signature envelope, trust store, version selection — implemented in `df75c83` as the shared pure `update-core` crate), the main-side provider discovery layer (bounded GitHub/Gitee/Auto enumeration and metadata fetch — implemented in `eace586`), the durable trusted-target/package-staging half (implemented in `c8a37ab` — persisted state is re-authenticated on every recovery), and the frozen `UpdateSession` envelope plus the authenticated helper handoff (implemented in `27dbd55` — the helper parses only the frozen `--update --session-id <UUID> --expected-manifest-sha256 <64 hex>` CLI, independently re-verifies the session, the persisted signed bytes, and every staged fact through the shared core and shared ZIP primitive, and produces a `ValidatedHandoff`, the typed capability every mutation API requires) exist, and the runtime mutation transaction is implemented through `ReplacedAwaitingLaunch` in `c8e7c03`: the main updater owns the authenticated spawn boundary for the canonical installed helper (exact frozen argv, plus the durable-handoff wait before any canonical shutdown), and the helper owns live caller-identity binding (PID + creation FILETIME + canonical image), the Gate/exclusive lease, the durable `Updating` transition, the intent-before-side-effect backup/replace journal over exactly the two managed executables, runner-based helper self-replacement, crash reconciliation (a landed replacement with a missing completion journal is reconciled by preserving the verified OLD backup and recording only the completion — never rebuilding the rollback asset from the target), and recovery classification. Since `b825d33` the transaction is complete: `ReplacedAwaitingLaunch` is a real recovery state and the normal entry into the completion half — the helper launches the target runtime suspended (`CREATE_SUSPENDED` → identity capture → durable `probationProcess` → resume the exact child, so the child's admission cannot race the authoritative process binding), admission gains the `PostUpdateProbation` classification binding session, nonce, journal session, PID, creation FILETIME, canonical image, and the installed target main binary's hash against the signed fact (an ordinary launch during `Updating` stays refused and routes recovery), the admitted child writes the frozen HealthAck after core initialization, and the helper decides commit or rollback under the frozen write ordering — commit only after durable `acceptedHealth`: journal `commitIntent` → verified target set → integration/registry at the target → installed evidence → receipt `Installed` at the target → session `committed`; rollback verifies the fixed OLD slots, restores both managed executables (mixed crash states are recovered by independently observing each destination), restores integration/evidence/receipt at the source, and finalizes `rolled-back` — each followed by bounded cleanup that never degrades terminal truth. `--recover` executes the frozen-safe action the classification names, and a normal launch over an `Updating` receipt routes that recovery (the frozen Q14 single entrypoint) before any database or UI work. The production `install_ready_update(sessionId)` command triggers an already-trusted staged update — the session id is the frontend's only input — and shuts the app down canonically after the durable handoff. The core authenticated update transaction is implemented, and since `46d911b` the production trust/signing infrastructure exists: main and helper share one compiled trust source (`update-core/src/production_keys.rs`) and never accept runtime trust injection from frontend, session, or provider, and the offline `release-signer` operator tool signs exact raw manifest bytes only after manifest validation, an exact compiled trust-store membership gate, and self-verification through that same compiled store — the signer's key is signing capability, never trust authority. The compiled production trust store is currently empty and fail-closed: infrastructure implemented is not production signing enabled. Since `712a7bb` the release packaging and publishing machinery around that signing is implemented as operator tooling (`release-pipeline/`: canonical nine-entry package assembly self-validated through the shared ZIP primitive, release facts from the final verified package bytes, deterministic schema-1 manifest generation whose exact bytes are signed and served unchanged, signing delegated to the release signer, GitHub/Gitee publish orchestration with an explicit collision/idempotence policy, byte-exact remote read-back verification, mirror consistency by artifact digests, and a read-only dry-run with local operator reports) — GitHub is published draft-first and only finalized after a passed read-back, while Gitee has no draft state so a Gitee publish is immediately visible; rehearsal artifacts may touch a production provider endpoint only through a release stable discovery will not select — refined in `875ee32`/`64cb4b0` after the live rehearsal smokes showed the earlier blanket production-endpoint refusal could not express the documented isolation paths — with rehearsal finalize refused; GitHub isolation is the draft state and Gitee isolation is the prerelease publication flag plus an explicitly sub-production rehearsal version. One real GitHub rehearsal draft smoke has been verified live against the production repository endpoint (commit `875ee32`: draft creation, exact-artifact upload, live same-byte idempotence, authenticated byte-exact remote read-back, package/envelope/EXE validation, stable-discovery isolation, and finalize refusal); the live smoke also hardened three GitHub adapter compatibility behaviors — draft releases are not resolvable by tag (bounded authenticated release-list fallback), draft assets download through the GitHub API asset URL (the untagged browser URL does not serve API tokens), and read-back carries the release's explicit tag — provider implementation facts, not signed-protocol changes. One real Gitee rehearsal smoke has likewise been verified live (commit `64cb4b0`): a prerelease-marked rehearsal release on the production Gitee endpoint, real multipart `attach_files` upload of the exact public artifacts, live same-byte idempotence, and authenticated byte-exact remote read-back — hardening the Gitee adapter behaviors observed live (a missing release lookup returns HTTP 200 with a literal `null` body, release creation requires `target_commitish`, the asset JSON is sparse, and Gitee auto-creates the release's git tag and source archives), again provider implementation facts, not protocol changes. The same rehearsal artifact set has been fetched from both real providers and compared byte-for-byte: names, sizes, SHA-256 digests, the manifest digest, and the envelope key id all agree — real cross-provider byte consistency verified for this rehearsal set. Gitee publishing and the updater's Gitee download plane are distinct clients with distinct policies: the publishing tool may follow the observed live asset redirect chain (gitee.com → attach-files → the `foruda.gitee.com` CDN), while the runtime updater's transport allowlist (`GITEE_DOWNLOAD_ORIGINS`) admits only `https://gitee.com` and therefore currently fails closed on that CDN redirect — runtime Gitee fetch compatibility is a dedicated transport-hardening step required before the first production RC. The Gitee CDN redirect allowlist hardening and updater-side live fetch revalidation, the real key ceremony and public-key provisioning (after the transport hardening), the first signed release, and the release/UI surface remain open ([protocol v1 contract](docs/maintenance-protocol-v1.md); operator runbook [docs/release-signing.md](docs/release-signing.md)). The maintenance architecture preserves the Standard-only rendering architecture and the frozen native boundary.

v1 stops at factual local Review and Reports. Charts, evaluative trends, AI summaries, hourly/multi-day weather products, downloadable themes, autostart, complex tray behavior, auto-hide, notifications, calendar integration, and sync remain later work. [FUTURE.md](FUTURE.md) tracks the deferred list.

## Historical note

Earlier versions included an experimental Enhanced rendering backend using composition-hosted WebView2 and Native Acrylic/HostBackdrop, with a vendored Wry patch and a self-contained Windows App SDK runtime payload.

It was retired after Standard rendering became sufficient for all supported product modes, while providing lower maintenance cost and fewer environment-dependent failures.

The previous implementation remains available in Git history and release tags.
