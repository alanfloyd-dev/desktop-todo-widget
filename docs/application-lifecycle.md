# Application Lifecycle & Maintenance Architecture

Status: **Phase 1 implemented** — installation lifecycle, receipt, launch admission, manual bootstrap with v1.1 adoption, reinstall semantics, interrupted-install recovery, and native keep/remove-data uninstall implemented in commit `b381963` on top of v1.1.0 (`6c1de930ee82116198e592a38086adf3dcc62923`). **The Phase 2 signed updater (§6–§10, §16) is not implemented** and its sections remain design. Deviations between this document and the Phase 1 implementation are recorded inline as "Phase 1 notes"; the source-level audit in §2 describes the v1.1.0 baseline and is kept as the historical record.

This document defines installation, Windows registration, update, recovery, and uninstall as one application lifecycle. It follows the existing flat `docs/` structure. Wire contracts are in [Maintenance protocol v1](maintenance-protocol-v1.md). The §2 audit describes v1.1.0 shipping behavior. Sections §3–§18 mix the standing design with "Phase 1 notes" that record how a requirement was actually implemented; anything outside those notes and outside the implemented status above is still future work.

## 1. Goals and non-goals

v1.2.0 is the first self-maintaining / self-updating release. Updates must be authenticated, recoverable, and independent of application data. Uninstall must be discoverable, resumable, and preserve personal data by default. All operations are per-user, offline during installation mutation, and require no administrator privileges.

Non-goals: implementing this design in the documentation change; MSI/NSIS, delta patches, self-extracting installers, elevation, services, a second Tauri app, complex PKI/revocation infrastructure, or generic transaction frameworks. Do not restore Enhanced, composition-hosted WebView2, Native Acrylic/HostBackdrop, WinAppSDK, or patched Wry.

`src-tauri/src/window_mode.rs` and `src-tauri/src/product_window.rs` remain frozen by default. Maintenance is orthogonal to HWND styles, reparenting, Shell children, DPI, subclassing, taskbar visibility, and geometry persistence. Future startup/exit integration belongs at the application boundary; file size or architectural tidiness is not a reason to change the native core.

Preserve `alan-desktop` (Cargo package/binary), `alan_desktop_lib`, `net.alanfloyd.desktop`, `alan-desktop.sqlite3`, and `alan-desktop-tray`. Release packaging continues to expose `desktop-todo-widget.exe`; the new helper is `desktop-todo-maintenance.exe`.

## 2. Current implementation audit

| Area | Evidence and shipping behavior | Consequence for v1.2.0 |
| --- | --- | --- |
| Build layout | [Cargo.toml](../src-tauri/Cargo.toml) is one package with library and executable; no Cargo workspace or helper crate. [package.json](../package.json) defines the pnpm frontend workspace scripts. | Introduce a small shared Rust library and native helper later; no crate restructuring now. |
| Version sources | `package.json`, `src-tauri/Cargo.toml`, the local package in `src-tauri/Cargo.lock`, and [tauri.conf.json](../src-tauri/tauri.conf.json) carry 1.1.0. [build.rs](../src-tauri/build.rs) delegates Windows resources to tauri-build. Bundle generation is disabled. | CI must check agreement and inspect packaged PE ProductVersion, not infer it from a filename. |
| Install | [install.ps1](../install.ps1) accepts public or legacy EXE name, copies to `%LOCALAPPDATA%\Programs\desktop-todo-widget`, optionally copies four exact documentation names, stages `.installing`, and moves into place. Custom descendants of LOCALAPPDATA are accepted. Stops image-path-matched processes, with forced-stop fallback. | No receipt, publisher authentication, durable journal, HealthAck, rollback, or registry registration currently exists. Existing lexical path checks are not the future path safety boundary. |
| Shortcut | Installer uses WScript.Shell to create `%APPDATA%\Microsoft\Windows\Start Menu\Programs\desktop-todo-widget.lnk`, with target, working directory, description, and icon. `-NoStartMenuShortcut` opts out. | Persist the integration preference; reconcile must respect opt-out. |
| Uninstall | [uninstall.ps1](../uninstall.ps1) removes a matching shortcut, rejects subdirectories/unrecognized files, removes files then an empty root. It recognizes EXEs plus broad DLL/WINMD/PRI/LOG/MD/TXT suffixes. `-RemoveUserData` prompts; `-Force` skips the prompt. | These extension rules can classify unrelated files as owned. Do not copy them into Maintenance; use exact identities. Existing script remains unchanged in this task. |
| Data | [lib.rs](../src-tauri/src/lib.rs) resolves `%APPDATA%\net.alanfloyd.desktop` early, compares with Tauri `app_data_dir`, then opens the DB. [database.rs](../src-tauri/src/database.rs) has migrations through schema 3. [settings.rs](../src-tauri/src/settings.rs) stores typed settings in `app_settings`; Quick Links are in that document, with the legacy shortcuts table preserved. [appearance.rs](../src-tauri/src/appearance.rs) stores managed images in `assets/` beside the DB. | Data is **Roaming APPDATA**, not LOCALAPPDATA. Do not move it. Tasks, history, settings, profile, Quick Links, images, and the weather cache are outside runtime rollback. |
| Startup | `lib.rs::run` initializes QA diagnostics and reads settings before building Tauri; setup opens DB, loads state, installs tray, finds main window, applies widget frame, restores mode. | Future normal-launch guard must precede diagnostics writes and early DB reads. HealthAck follows successful core setup; no native-window reordering is required. |
| Logging | [qa_diagnostics.rs](../src-tauri/src/qa_diagnostics.rs) recreates `qa-diagnostics.log` beside the EXE at startup and appends diagnostics/panics. [diagnostics.rs](../src-tauri/src/diagnostics.rs) provides allowlisted support output. | Maintenance needs independent persistent session logs; do not reuse an overwritten application log as a journal. |
| UI | [SettingsPanel.vue](../src/components/SettingsPanel.vue) contains product settings and a collapsed DeveloperDiagnostics area; [App.vue](../src/App.vue) owns settings display. No dedicated lifecycle/About or updater UI exists. | Add a future Settings/About maintenance section, separate from developer diagnostics; both uninstall entry points use the same helper UI. |
| Release | [README](../README.md), [release notes](../RELEASE_NOTES.md), and [contribution guide](../CONTRIBUTING.md) describe production builds and manual scripts. No tracked release packaging script or `.github/workflows` pipeline exists at this baseline. Local `release/` is untracked. | The CI sequence below is a new requirement, not an existing automation. Ignored legacy SDK files under `tools/` do not constitute a current build dependency. |
| Update | No updater dependency, UpdateSession, Maintenance helper, receipt, or HealthAck implementation was found in tracked application sources. | Avoid presenting planned capabilities as shipped. |

The local archive was found at `release/v1.1.0/desktop-todo-widget-v1.1.0-windows-x64.zip` (one directory deeper than the supplied shorthand). Read-only inspection confirmed exactly six root entries: EXE, `install.ps1`, `uninstall.ps1`, `README.md`, `README_ZH.md`, `LICENSE`.

| Verified artifact | Bytes | SHA256 |
| --- | ---: | --- |
| ZIP | 2,456,690 | `43f6dae8ec70b2aa5ee4e1a666734ba02b5fb107ec3118516316397b1df0f4c1` |
| EXE inside ZIP | 5,157,888 | `32425a23b9f8ae10abfdf358553471713024569880ba742094dea670c9840cfc` |

`v1.0.0` is an annotated tag: its tag object differs from its peeled commit. `git rev-parse 'v1.0.0^{commit}'` confirmed `083e21388f434d1228dc460384a022e4e9a930e5`. Do not move or recreate it. Preserve `.rc0-evidence/`, `.rc0-taskbar-target/`, `scripts/rc0-qa/`, `inquiry/`, and existing release artifacts; never use broad staging or cleaning for this work.

Reported release QA baseline: 125 Rust tests passed; cargo check has three existing warnings and zero new warnings; production custom-protocol build and vue-tsc pass; script verification is 43/43. These are historical inputs, not tests rerun for this documentation change.

## 3. Architecture and responsibility boundaries

```text
Vue Settings/About
    -> main Rust Updater Core
        -> GitHubReleaseSource / GiteeReleaseSource / Auto policy
        -> authenticate manifest -> download -> verify -> stage
        -> user-approved UpdateSession
    -> native desktop-todo-maintenance.exe
        -> Maintenance Core
            InstallTransaction / UpdateTransaction
            UninstallTransaction / RecoveryTransaction
        -> wait for exit -> replace -> launch probation child
    <- new app core initialization -> HealthAck -> commit

Windows Installed apps -> same native helper -> same UninstallTransaction
install.ps1           -> native helper --install (future bootstrap)
```

The main Rust backend owns discovery, network access, source preferences, downloads, signature/hash verification, staging, version policy, and confirmation before handing control to Maintenance. Vue sees normalized progress and errors, not filesystem authority or provider JSON. No arbitrary-path maintenance commands are exposed to frontend callers.

The helper is a small GUI-subsystem Rust/Windows executable, with native progress/error and uninstall confirmation UI. It owns validation, locks, waiting for known processes, backup, replacement, launch, health verification, rollback, recovery, uninstall, and integration reconciliation. It never contacts GitHub/Gitee, chooses a release, opens SQLite, or understands Today/Weather/Review.

Future logical components (placement to be decided during implementation): shared `maintenance-core` library with path policy, resource catalog, receipt codec, lock, journal/atomic writes, signature verification and integration reconciliation; separate transaction modules; native helper binary; updater module in the main backend. Native UI is a helper adapter over core outcomes. Keep the four state machines independent rather than introducing a universal transaction engine.

## 4. Resource ownership and supported installation scope

Protocol 1 supports one managed stable installation per Windows user at the canonical default root. Existing custom installs and portable extracted copies still run, but do not silently become managed installations. They require an explicit manual adoption/move flow before self-update. A second copy must not seize the first copy's receipt, shortcut, or data-deletion authority.

| Class | Identities / canonical location | Update | Uninstall |
| --- | --- | --- | --- |
| Runtime | `mainExecutable`, `maintenanceHelper` at fixed names under install root | Replace only signed, policy-known files | Remove identified owned files |
| Runtime support | Exact optional `README.md`, `README_ZH.md`, `LICENSE`, `THIRD_PARTY_NOTICES.md` copied by bootstrap, recorded only after creation | Not updater-managed in protocol 1 | Remove only with ownership evidence; preserve modified/ambiguous documents |
| Integration | `mainStartMenuShortcut`, `windowsUninstallEntry`; future startup registration needs an explicit policy extension | Reconcile desired state | Remove only matching owned resources |
| PersistentData | DB including journal/WAL/SHM, settings/history in DB, generated appearance assets under Roaming app data | Never mutate or restore | Keep by default; validated, explicitly confirmed cleanup only |
| Ephemeral | Sessions, package cache, staging, helper runners, maintenance logs; exact `qa-diagnostics.log` | Bounded cleanup after terminal state | Clean known inactive resources, preserve current recovery evidence until finalization |
| Maintenance metadata | Receipt, committed manifest/signature and active journal | Durable coordination and release evidence; not business data | Keep until cleanup completion; never treat active or committed release evidence as disposable cache |
| Unknown | Any unrecognized or ambiguously owned entry | Preserve | Preserve and report; remove root only when empty |

Compiled identity mappings determine paths. An arbitrary file named `notes.md` or `something.dll` is not owned. Historical SDK leftovers are not rediscovered by suffix: a future explicit legacy catalog and evidence are needed, otherwise leave them untouched. Never recursively erase the install root.

Proposed layout:

```text
%LOCALAPPDATA%/Programs/desktop-todo-widget/
  desktop-todo-widget.exe
  desktop-todo-maintenance.exe
  installation-receipt.json
  installed-manifest.json             # exact committed signed bytes
  installed-manifest.json.sig
  .maintenance/<sessionId>/           # same-volume replacements + backups
%LOCALAPPDATA%/net.alanfloyd.desktop/maintenance/
  updates/sessions/<sessionId>/       # durable session, manifest, sig, ZIP, staging, health, logs
  uninstall/sessions/<sessionId>/     # journal + temporary runner
%APPDATA%/net.alanfloyd.desktop/
  alan-desktop.sqlite3
  assets/
```

Paths are descriptive; environment variables in serialized records are expanded absolute snapshots, never executable substitutions. Known Folder resolution and handle-based validation are authoritative. WebView2 user-data/cache location must be inventoried during implementation; no guessed WebView directory may be deleted. Shared WebView2 Runtime is an external prerequisite and is never uninstalled by this app.

## 5. InstallationReceipt

The [receipt contract](maintenance-protocol-v1.md#installationreceipt) describes what this machine has installed. The UpdateManifest describes what the publisher permits installing. Neither is a script, and the receipt is not a signature trust root.

First managed install generates an installation UUID; regular updates and rollback retain it. Successful uninstall ends it, even when business data is retained; reinstall creates a new UUID and may reopen the old data. Legacy v1.1 adoption has no old receipt UUID to preserve. Receipt loss must not fabricate continuity: explicit repair may establish a new identity after conservative discovery and confirmation.

Lifecycle values: `Installing`, `Installed`, `Updating`, `Uninstalling`, `RecoveryRequired`. During update `currentVersion` remains the last committed runtime until commit; target version and active session describe the transition. An `Installed` receipt requires verified runtime plus reconciled integration. A partially completed commit remains transitional and blocks normal launch until recovery completes.

Safety precedence is **compiled safety policy > canonical installation validation > receipt assertions**. Reject mismatched appId, malformed UUIDs, unsupported schema, duplicate identities, conflicting root/version evidence, and roots outside policy. Receipt text claiming `C:\Windows` cannot authorize access there. Unknown receipt schema is preserved for a newer helper/manual repair, not interpreted as an empty resource list.

**Phase 1 notes (implemented).** The receipt is the installation's durable state machine, enforced by one codec:

- `deny_unknown_fields` is deliberate and stricter than the generic extension-field allowance in [protocol v1](maintenance-protocol-v1.md): an unknown field, resource identity, or lifecycle value fails deserialization, so a forged or future-schema receipt can never smuggle new authority into the current binary.
- An `Installing` receipt is published **before** the first filesystem mutation and is the durable crash marker of an interrupted bootstrap. Normal launch is refused in every transitional state; rerunning the same trusted payload resumes the transaction (idempotently, keeping the existing `installationId`), and any other recovery belongs to the helper. No separate install journal exists.
- Same-version reinstall keeps the installation id and increments `receiptGeneration`. A receipt claiming a **newer** version than the running installer refuses as a downgrade; installing the same or a newer version normally is allowed.
- Support-resource ownership carries across reinstall only when the identity is compiled-known **and** the file on disk still matches the recorded SHA256 fingerprint; otherwise the record is dropped and the file becomes unowned. Unknown files are never adopted, and a receipt can never widen the compiled deletion authority (the resource enum is closed at deserialization).
- After the manual bootstrap `committedManifestSha256` stays empty and `activeSessionId`/`lastCompletedSessionId` stay unset; no signed evidence is fabricated.

## 6. Manifest, trust, protocol evolution

Protocol 1 requires signed raw manifest bytes, flat ZIP, Windows x64, fixed managed files, offline native helper, persistent journal, safe replacement, HealthAck, and runtime rollback. `schemaVersion` answers whether the fields can be read; `updaterProtocol` answers whether the operation can be executed safely. Check both independently in main app and helper. Parseability does not confer protocol compatibility.

Use Ed25519 through a maintained library; never implement cryptographic primitives. Embedded `TrustedKeyStore` maps known key IDs to keys. Verify detached signature over exact bytes before parsing or trusting manifest fields; retain those same bytes in the session. SHA256 then verifies ZIP and each managed file. A `.sha256` sidecar remains useful for manual download checking but does not authenticate a publisher. HTTPS and filename/version agreement alone are insufficient.

The running old helper, copied to a session runner, verifies the next payload using its existing trust store. Do not execute an unverified replacement helper to validate itself. Key rotation uses a bridge release signed by K1 that embeds K1+K2, followed by K2-signed releases. Old K1-only clients need the bridge or a manual upgrade; adapters cannot bypass trust failures. Protocol changes likewise need a release executable by the old protocol that installs support for the new one.

Lost private key: if no already-trusted alternative can sign a bridge, manual upgrade establishes a new root. Compromised private key: attacker signatures can look valid; adding a key signed only by the compromised key does not restore trust. Halt publication, communicate the incident through independently authenticated channels, and permit manual reinstallation. No online revocation guarantee is claimed.

## 7. ReleaseSource abstraction

`ReleaseSource` exposes `fetchLatest(channel)` returning raw manifest/signature plus untrusted discovery metadata, and `resolveAsset(exactVersion, platform, filename)` returning a download locator. GitHub/Gitee adapters normalize their own provider JSON; Updater Core consumes only the internal model. Repositories/hosts are application configuration, never supplied by manifest notes. Redirects and download hosts need adapter allowlists; impose network timeouts and response-size limits.

Offer GitHub, Gitee, Auto from day one. Explicit sources do not silently switch. Auto tries GitHub with bounded transient retry (initial attempt plus two retries; respect bounded Retry-After), then Gitee when discovery is unavailable. No IP/geolocation. Authentication/signature, wrong-app, malformed protocol, or inconsistent-content failures are surfaced and do not silently downgrade trust. A valid GitHub result with no newer release completes discovery; do not compare arbitrary latests to find a larger number.

After a trusted manifest is accepted, freeze `(appId, channel, version, platform, manifest SHA256, package filename/size/SHA256)`. A subsequent artifact transport failure can use the other adapter only for that exact tuple. Different manifest bytes for the same version are a mirror-integrity failure. Never query another latest to replace an accepted target. No auto-downgrades or reinstallation of the current version; explicit repair is a separate operation. Signed metadata can be stale: signatures do not prove latestness; reject below the installed version and record the limitation rather than inventing an online freshness guarantee.

## 8. UpdateTransaction

| State | Required action / durable boundary |
| --- | --- |
| Prepared | Main app verifies signed manifest, ZIP structure/hash and staged files; records session and obtains user consent. Runtime remains unchanged. |
| ValidatingHandoff | Old trusted helper validates installation, session UUID/root binding, protocol, signature, all staging hashes and caller identity. |
| Locking | Acquire maintenance exclusion; persist active session in receipt and journal before asking app to exit. |
| TransitionToTemporaryHelper | Copy current helper to the private session runner, verify bytes, start it and transfer coordination without a normal-launch gap. Runner assumes ownership before parent exits. |
| WaitingForMainAppExit | Request graceful shutdown and wait on known process handles. Do not force-kill by name. Timeout aborts before mutation, or offers explicit retry/force-stop for verified installation processes. |
| BackingUp | Recheck installed preimage, reserve space, capture previous receipt/integration desired state and verified old hashes; copy replacements into install-volume workspace and verify again. |
| Replacing | Journal intent and completion for each fixed resource; replace main and helper. An intermediate mixed pair is never launchable. |
| VerifyingInstalled | Rehash installed files and check version resources against the accepted target. |
| LaunchingProbation | Launch exact installed EXE with restricted child-only coordination environment; retain process handle. |
| AwaitingHealthAck | Validate one-use marker from that child within bounded timeout. No successful update yet. |
| Committing | Persist commit intent/validated health evidence; reconcile target integration, write target receipt, persist committed journal. Retry interrupted steps idempotently. |
| Committed | Release lock, report success, then garbage-collect owned backup/staging with bounded retention. |
| RollingBack | Stop/wait probation child, restore verified preimages for every touched resource, verify old runtime, reconcile old desired integration and receipt, restart previous version through authorized launch. |
| RolledBack / RecoveryRequired | Report failure clearly. Failed restoration or unverifiable state keeps launch blocked and preserves evidence for recovery/manual repair. |

Cancellation is safe before replacement: clear active state only after checking no mutation occurred. After first mutation, cancellation means finish recovery/rollback, never immediate helper termination. A healthy rollback launch must have a separate one-use authorization; failure to restart the old version is reported, not relabeled update success.

### Windows replacement and helper self-update

Use `ReplaceFileW(target, replacement, backup, ...)` for an existing managed target. All three reside on the same volume; copy from staging into `.maintenance/<sessionId>/` first. This is a per-file primitive, **not** atomic commit for a pair of executables or a complete installation. The journal supplies recovery across files. First installation of an absent target uses a separately validated same-volume move and records an absent preimage.

Microsoft documents partial-failure distinctions for errors 1175, 1176, and 1177. On any failure inspect target/replacement/backup identities and hashes rather than assuming the old layout remains. `REPLACEFILE_WRITE_THROUGH` is unsupported; durable journal writes need their own flush discipline. Validate behavior on supported Windows/filesystems before implementation is accepted. [Microsoft ReplaceFileW reference](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).

**Phase 1 notes (implemented).** Phase 1 does not use `ReplaceFileW` (that belongs to the update transaction). Its shared file primitives, which the update transaction will reuse, are:

- Deletion uses POSIX semantics (`FILE_DISPOSITION_INFO_EX`, with a legacy-disposition fallback for older systems), so the directory entry disappears atomically with the call and no delete-pending ghost name can block an immediate recreate or replace.
- Same-volume replacement is centralized in one primitive: `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` with a bounded retry ladder (a sub-100 ms cap) restricted to ACCESS_DENIED/sharing-class errors only. External/system-level sharing conflicts — real-time scanning, indexing, or shell activity around a file that was just created, renamed, or registered — can hold files for longer than any reasonable ladder; those surface as a precise per-file error, and the documented recovery is to rerun the legal entry point (bootstrap retry or uninstall resume), never an unbounded wait. The primitive holds no destination handle across the replacement call; long-lived external sharing conflicts are surfaced rather than hidden behind an unbounded wait.
- Failed atomic writes remove their own unpublished temporary file so the next lifecycle starts clean.

Executing from the session copy releases the installed helper image for replacement/deletion. The runner remains the **old protocol implementation** until the transaction ends. Keep its hash and original installation binding in the journal; recovery must never execute an arbitrary path from session JSON. The runner can leave its own locked EXE for the next verified Maintenance invocation to remove after its process exits. Do not use a shell command assembled from paths or admin-only reboot deletion as the normal cleanup route.

## 9. Durable journals and RecoveryTransaction

Sessions persist `session.json`, original manifest bytes/signature, package, staging, logs, HealthAck, resource pre/post hashes, operation intent/completion, and snapshots needed for reconciliation. Active backups and journals are not cache. Bound raw metadata sizes, preserve monotonic generation numbers, and use sibling temp writes, full flush, atomic replacement, then validate on read. Keep the last valid generation. Do not claim power-loss safety based solely on a rename; qualify filesystem/flush behavior in fault tests.

Recovery runs under the same exclusion and before a normal app opens its DB. It examines receipt plus journal plus actual files. Journal contents locate only policy-derived identities; they do not grant deletion, launch, or rollback authority. Retain the committed signed manifest/signature at the fixed installed evidence paths independently of session retention. At update start copy that evidence into the protected rollback workspace; commit publishes the new evidence together with journaled receipt/integration changes, and rollback restores the old evidence. These metadata files have compiled identities and are not arbitrary payload destinations. For initial manual adoption, validated local preimage hashes are recovery evidence, not publisher authentication.

| Last provable boundary | Recovery decision |
| --- | --- |
| Prepared, no runtime mutation | Cancel/resume preparation after validation; release only consistent installed state. |
| Replacement intent without completion | Inspect all three file locations; classify known old/new/absent hashes; restore the old set before normal launch. |
| Mixed runtime or no accepted durable HealthAck | Roll back every touched resource, including helper; never run a mixed set. |
| HealthAck exists but helper died before validating it | Do not trust stale PID/timestamp. Revalidate runtime and perform fresh controlled probation if safe; otherwise rollback. |
| Durable commit intent with accepted health evidence | Verify target set and finish receipt/integration reconciliation; do not blindly rerun replacement. |
| Committed but cleanup incomplete | Verify committed installation; retry only owned ephemeral cleanup. |
| Uninstalling | Resume idempotent uninstall; do not restore deleted integration. Reconfirm destructive data choice after process loss. |
| Corrupt/missing journal, conflicting receipt or unknown file hashes | Preserve files; mark RecoveryRequired and offer explicit repair/manual install. No guessed destructive rollback. |

No startup scheduler/service is required: next normal launch, Installed apps invocation, or manually launched verified helper enters recovery. If the main EXE is missing, use the installed helper or extracted verified release helper. Recovery cannot promise autonomous execution when every entry point is missing; document the manual route.

## 10. HealthAck and application data compatibility

Success requires initialized Rust/Tauri backend, successful DB open/migration, essential settings loaded, tray setup and main window creation/restoration successful. Weather network responses, avatar fetches, and other optional content are excluded. Future frontend readiness can be an additional local check without changing native ordering.

The child writes bounded `health.json.tmp`, flushes, and atomically renames to `health.json`. Helper verifies sessionId, target version, nonce, child PID plus creation time/process handle, exact image identity and current session phase. Timestamp is metadata, not proof. A stale marker or unrelated running app cannot acknowledge. Protocol defaults and fields are in the companion document.

Runtime rollback does not undo database migration or user changes. The release gate requires old runtime compatibility with all DB/settings writes possible during probation. Hold ordinary editing until maintenance commits to minimize exposure. A breaking migration blocks automatic delivery until a separately designed application-owned backup/migration recovery scheme exists. The helper never opens SQLite or interprets schema migrations.

## 11. Maintenance lock and launch admission

Use a user/installation-scoped named mutex with a restrictive user DACL for live helper ownership, plus an exclusive lock file under the canonical maintenance root to cover the same user's concurrent Windows sessions. A session-local mutex alone cannot protect shared install files across Fast User Switching/RDP sessions. Key exclusion by canonical install root and user SID, not solely by a mutable receipt UUID. Hash names where needed; do not log raw SID.

Normal launch obtains the short admission gate before any DB/log/window initialization, checks active durable maintenance state, registers its process identity, then releases admission. Helper takes the same gate to close admission and persist its intent before enumerating/waiting for all registered and path-verified app instances. This prevents the check-then-launch race. Reject unsupported cross-session maintenance if all writers cannot be identified/stopped safely.

The live lock ends on crash, but the active journal/receipt remains: an abandoned mutex is a recovery trigger, not permission to launch normally. Transfer from installed helper to runner uses an explicit ready/ownership handshake while durable admission stays closed.

Only a helper-authorized probation/rollback child can enter while closed. Generate at least 256 random bits, pass through that child's environment (never normal command line), bind it to the current session and process creation, consume once, and clear it before launching descendants. Environment and user-owned files are coordination, not a security boundary against malicious software already running as the same user. The design prevents unsafe remote payloads, wrong paths, stale coordination and accidental concurrent launches; it does not claim resistance to an attacker fully controlling that user's processes.

**Phase 1 notes (implemented).** Launch admission is live for ordinary starts:

- Admission is the first thing `run()` does — before any diagnostics write and before the early rendering-backend database read — and it is check-only: it never repairs a receipt.
- Classification: `Managed` (valid `Installed` receipt and the process runs from the canonical install root), `Unmanaged` (no receipt and no helper in the canonical root — legacy/custom/portable/development copies run normally and are never auto-adopted), `Development` (a valid canonical receipt exists but this process is not the installed runtime: dev loop, test harness, or a portable copy beside a managed install; allowed, holds the lease, is not the managed runtime).
- Fail-closed refusals (never downgraded to Unmanaged): malformed receipt, unsupported schema, appId/root mismatches, unsafe state, every transitional lifecycle, an active uninstall journal, and **helper present without a receipt** (broken managed state). Refusals show a native error and exit non-zero.
- The app holds the shared application lease until process exit, so a maintenance transaction cannot start while any instance runs; a running transaction's gate, conversely, refuses normal launch before the database is opened.
- The Settings uninstall entry exists only for `Managed` instances. The frontend receives a minimal `{ mode }` view and never sees the receipt, paths, or maintenance files; `start_uninstall` takes no path argument — the helper identity is compiled policy (admission's install root + fixed name), validated as a regular unlinked non-reparse file, spawned without waiting, and the app then exits through its normal exit path.
- Known Folder resolution failure in the main app degrades to the pre-maintenance behavior without a lease (logged); the helper, by contrast, fails closed when it cannot resolve canonical roots. This asymmetry is deliberate: the app preserves legacy compatibility, the maintenance tool must never guess a target.

## 12. Path safety

Every operation independently derives canonical roots from Windows Known Folders and compiled policy. Protocol 1 refuses custom/system/root/profile-wide destinations, UNC/device paths, alternate data streams, traversal, reserved DOS names, trailing-dot/space aliases, reparse points/junctions/symlinks within managed trees, and unexpected hard links on managed files. Validate volume, final resolved handle path, owner/ACL and file type. Validate existing parents when a leaf is absent. Receipt/session/CLI paths must agree with these derivations.

String prefix checks and `GetFullPath` alone are insufficient. Use opened handles and revalidate identity immediately before mutation; avoid following links during enumeration or cleanup. An entry that changes between validation and use aborts the affected operation. Do not broaden permissions to resolve access denied. Reject a receipt at `C:\Windows` even if its appId is correct.

ZIP processing never calls unrestricted extract-all: reject traversal, absolute/nested names, backslashes, case-insensitive duplicates, links, malformed/encrypted entries, conflicting local/central metadata, and expanded size/ratio beyond limits. Each required managed entry occurs exactly once with exact size/hash. Unknown safe root-level entries are ignored for automatic installation; unknown dangerous structures reject the package. Scripts/readmes/licenses remain in the flat distribution for humans but are not executed or installed by automatic update.

PersistentData deletion requires the independently resolved exact Roaming app directory, no links, no other installation/process using it, and explicit native UI confirmation. Delete only known DB sidecars and validated app-managed asset identities. Preserve unknown files even inside the data root; show residual paths and never claim complete erasure when data cleanup was partial. Do not delete original imported images or wallpaper outside app data.

## 13. UninstallTransaction and conservative discovery

Windows Settings -> Installed apps and application Settings/About both invoke `desktop-todo-maintenance.exe --uninstall`; both present the **same** native confirmation. Default: **Keep local data and settings (Recommended)**. Destructive alternative: **Delete local data and settings**, showing the resolved data location and affected categories. No hidden data-deletion flag in the Windows uninstall registration.

```text
Starting -> LoadingInstallation -> AwaitingUserConfirmation
 -> TransitionToTemporaryHelper -> WaitingForMainAppExit
 -> RemovingIntegration -> RemovingRuntimeFiles -> RemovingEphemeralState
 -> OptionallyRemovingUserData -> Finalizing -> Completed
```

After confirmation acquire exclusion and mark receipt `Uninstalling`; validate again after transfer to the external runner. Cancellation before mutation leaves the installed state intact. Once cleanup starts it is idempotent: missing owned resources are success, access denied/locked resources remain pending with exact error codes, and retries resume. Never recreate shortcuts or registry entries merely because later deletion failed.

Preserve the receipt and active uninstall journal until all requested owned cleanup is complete. Do not erase the running journal in RemovingEphemeralState. Finalization records completion, removes receipt, then permits deferred runner/log cleanup. Unknown leftovers may produce `Completed` with explicit residuals because they are outside ownership; failed deletion of known requested resources is `Incomplete`, not success. Failure after removing the Installed apps entry must show a verified temporary-runner/manual-package recovery route so retry remains possible.

Deletion of personal data needs consent from the current native interaction; saved booleans in a mutable journal cannot authorize it after a crash. On resume reconfirm, defaulting to keep. An interrupted data deletion is irreversible and must be reported as partial; uninstall is cleanup-oriented, not rollback-oriented.

**Phase 1 notes (implemented).**

- The running helper copies itself to a session runner and exits before the transaction continues (the same runner-handoff design the update transaction reuses), so the installed helper image is never deleted while executing and destructive consent is never transferred through files or arguments.
- `Uninstalling` is itself resumable: an interrupted run leaves the receipt and the active uninstall journal in place, refuses ordinary launch and bootstrap while they exist, and the next helper run resumes to completion.
- Unknown resources are authority-bounded preservation, and **retaining a non-empty root because an unknown file or sentinel remains is a successful cleanup outcome**, not a failure.
- Deferred ephemeral cleanup is accepted policy: session-runner copies and state logs survive a completed uninstall until a later verified maintenance invocation removes them; they live entirely under the maintenance state root and are disclosed, not hidden.

**Conservative Discovery Mode:** for absent/corrupt receipts, disable automatic update and data deletion. Validate only the canonical default installation and independently corroborated exact known EXE/helper names, matching shortcut target, matching uninstall entry, and known maintenance session directories. Names alone in an arbitrary directory are insufficient evidence. Preserve unknown schemas/resources, unverified documents, and historical SDK leftovers. Do not recurse into directories merely because a receipt mentions them. If ownership cannot be established, report manual repair instead of deleting. Receipt loss may reduce completeness, never deletion safety.

## 14. Windows integration reconciliation

Desired installed state includes the user's shortcut preference, one uninstall entry, matching version/icon/location, and (future only) explicit startup preference. Shared reconciliation implements creation/update/removal for install, successful update, rollback, repair and uninstall. Scripts become callers; they must not grow independent registry logic.

Chosen non-MSI registration: `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\net.alanfloyd.desktop`, native x64 view. No HKLM, elevation, `WindowsInstaller=1`, or MSI product code. Microsoft WinGet's current ARP implementation explicitly reads user scope through HKCU and the x64 registry view. Microsoft's MSI documentation describes common value meanings, but is not a non-MSI installer specification; Windows 11 Installed apps behavior remains an end-to-end acceptance test. [Microsoft WinGet ARP source](https://github.com/microsoft/winget-cli/blob/master/src/AppInstallerRepositoryCore/Microsoft/ARPHelper.cpp), [Microsoft uninstall value reference](https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key).

| Value | Proposed value/type |
| --- | --- |
| DisplayName | `desktop-todo-widget` (REG_SZ) |
| DisplayVersion | Committed semantic version, e.g. `1.2.0` (REG_SZ) |
| Publisher | `Alan Floyd` (REG_SZ; release metadata must agree) |
| DisplayIcon | Quoted absolute main EXE path plus `,0` (REG_SZ) |
| InstallLocation | Validated absolute install root (REG_SZ) |
| UninstallString | `"<installRoot>\desktop-todo-maintenance.exe" --uninstall` (REG_SZ; executable quoted separately) |
| NoModify / NoRepair | `1` (REG_DWORD), until a real exposed repair UX exists |
| InstallationId | Current UUID, application-owned correlation value (REG_SZ; not authority) |

Do not expose QuietUninstallString initially. Helpers use CreateProcess with explicit executable and structured arguments, not a shell. Before changing an existing entry or shortcut, verify it belongs to this installation; collisions become conflicts, not permission to overwrite another app. Remove only known owned registry values; unexpected values/subkeys require conservative handling. Shortcut target must exactly match the canonical main EXE, not merely share a path prefix.

Steady-state invariant: **normalized EXE ProductVersion = receipt.currentVersion = Windows DisplayVersion** (stable `1.2.0` equals PE numeric `1.2.0.0` after specified normalization). Partial transitions are marked and recoverable. Reconciliation must read back all required fields. If it fails after HealthAck, retain Committing state and backups; retry on recovery, report maintenance incomplete, and do not announce success. Receipt and registry are not atomically updated together.

The Phase 1 implementation commit still carries the 1.1.0 version sources everywhere; the 1.2.0 bump belongs to release preparation, which must re-verify the full invariant including the release version itself before publishing.

## 15. InstallTransaction and bootstrap

`v1.1.x or earlier -> one manual trusted install -> v1.2.0 -> future Maintenance updates`. No purposeless bootstrap-only bridge release is needed. The initial v1.2.0 installation is the trust bootstrap: because the Maintenance helper itself is being introduced by that package, its embedded key store cannot authenticate the provenance of the package that supplied it. The user must obtain the first managed release through a trusted distribution path and may verify the published package hash independently. After installation, all self-updates require manifests authenticated by the embedded TrustedKeyStore. Authenticode can strengthen initial provenance later, independently of update-manifest signing.

Manual bootstrap accepts the ZIP-only distribution; sibling manifest/signature assets are not required for this first install. ValidatePackage checks the fixed payload layout and local file identities, without claiming publisher authentication. Bootstrap creates the InstallationReceipt and installs the embedded TrustedKeyStore, recording local runtime hashes as recovery evidence. Committed manifest/signature evidence may therefore be absent until the first successful signed self-update; preserve that absence and the validated local preimage hashes for rollback rather than fabricating signed evidence. This bootstrap exception never permits an unsigned self-update.

Install states: ValidatePackage -> ConfirmLocation/Adoption -> Lock -> WaitForExistingApp -> Stage/Backup -> InstallRuntime -> Verify -> Probation -> Reconcile -> WriteReceipt -> Complete. Journal each mutation; on failure restore a verified prior install or remove only newly created owned resources. Never remove retained business data. Adopt the default v1.1 install explicitly, preserve shortcut choice and data, and leave unknown historical files alone. Reject an existing managed `Uninstalling`/`RecoveryRequired` state until recovered.

Future `install.ps1` becomes a thin bootstrap to `desktop-todo-maintenance.exe --install`; `uninstall.ps1` becomes a compatibility wrapper to the same uninstaller, with a documented legacy fallback only for pre-maintenance installations. Current scripts are not changed here. The release ZIP remains flat with its seven existing files plus helper; signed manifest and signature are sibling release assets. Do not include the ZIP hash-bearing manifest inside the same ZIP and create a circular hash dependency.

**Phase 1 notes (implemented).** `install.ps1` is already the thin bootstrap: a payload carrying the helper is delegated (no paths passed — the helper derives the payload from its own location and the roots from Known Folders), exit codes propagate, and a custom destination is refused for managed payloads. A helper-less payload keeps the legacy direct-install path for pre-maintenance use, guarded: it refuses any install directory that already carries a receipt, so a pre-maintenance payload can never downgrade or trample a managed installation. `uninstall.ps1` is untouched legacy; it already refuses managed directories because the receipt matches none of its owned-file patterns. The supported destination is the single canonical per-user root; custom LOCALAPPDATA descendants remain a legacy-payload-only capability and are not adopted into the managed lifecycle.

## 16. Release / CI requirements

Implement a reproducible packaging workflow later:

1. Check immutable tag/commit and agreement of all version sources; run relevant Rust/frontend/install regression checks.
2. Build main app once with embedded frontend (`custom-protocol`) and native helper once for Windows x64. Inspect PE versions and Standard-only dependencies.
3. Assemble flat eight-file ZIP; hash final ZIP bytes and each updater-managed EXE. Generate `.zip.sha256` and manifest bytes once, then sign those exact bytes using protected release credentials.
4. Upload **identical** ZIP, sidecar, manifest and signature to GitHub and Gitee. Never rebuild or reserialize per source. Keep signing secrets outside repository/logs and deny signing to untrusted PR jobs.
5. Download every remote artifact and verify size, hashes, signature, names and equality across mirrors. Persist a release verification report tied to build commit and artifact digests.
6. Only after both are verified expose the version to updater discovery. Use adapter-recognized publication eligibility/draft handling; protocol 1 must not rely on a host's arbitrary latest ordering. A failed mirror publication leaves the new release ineligible and the previous eligible version unchanged.

Cross-host publication is not atomic. Adapters need contract tests for eligibility and exact-version lookup, including Gitee API authentication/rate-limit behavior. Repository addresses, protected key IDs, crypto crate choice, native UI binding, and precise provider publication mechanism must be pinned and reviewed during implementation before updater-visible release; they are not invented as existing infrastructure here. If one source becomes unavailable after publication, frozen-target fallback applies.

## 17. Failure matrix

| Failure | Required outcome |
| --- | --- |
| GitHub discovery timeout/rate limit | Bounded retry, then Auto discovery through Gitee; explicit-source mode reports failure. |
| Signature mismatch / untrusted key / wrong app | Fail closed; no staging execution or install mutation. |
| Unsupported schema/protocol | Explain need for bridge/manual upgrade; do not reinterpret fields. |
| Artifact failure after target selection | Exact-version, exact-manifest fallback only. |
| ZIP duplicate/traversal/hash mismatch | Reject and quarantine/delete only owned invalid download; current app stays usable. |
| Disk full before mutation | Abort safely, preserve old runtime; include backup/expanded-size budget in preflight. |
| Locked EXE / antivirus blocks replacement | Bounded retry with native error; inspect actual file state; rollback or RecoveryRequired. |
| Helper/process crash between two replacements | Admission remains closed via durable state; recover old complete pair. |
| New app crashes/DB open fails/HealthAck timeout | Stop identified child, rollback runtime; preserve all user data. |
| Old runtime cannot read new data schema | Release must have been blocked by compatibility gate; helper cannot repair business data. |
| Registry/shortcut write fails after health | Keep Committing; recover reconciliation, no false success. |
| Backup missing/corrupt | No destructive guessed restore; manual repair with preserved diagnostics/data. |
| Receipt missing/tampered | Conservative discovery, no data deletion, no arbitrary root permissions. |
| Uninstall interrupted after registry removal | Resume via verified runner/package; do not recreate removed integrations. |
| Unknown install/data files | Preserve and report leftovers. |
| Maintenance runner cannot delete itself | Defer exact runner cleanup after exit; no system-wide reboot action. |
| Other session or second helper active | Refuse concurrent transaction; no process-name kills. |

## 18. QA gates, observability, and implementation sequence

Maintenance logs contain session/installation IDs, operation, phase/generation, resource identity, hashes, durations, Win32 error codes, and recovery decision. Redact user paths in exported diagnostics; do not log nonce, environment, keys, tasks, Quick Links or other personal content. Local recovery logs may identify validated paths; exporting them is an explicit action. Apply bounded retention only to terminal sessions (initial policy: last 5 or 30 days); never evict an active session or its required backup to meet a quota. Offer open-log/manual recovery when native UI cannot complete.

Required acceptance gates for future implementation:

- Golden raw-byte signature fixtures, newline/BOM mutations, unknown key, K1-to-K2 bridge and unsupported protocol tests; no downgrades from stale signed metadata.
- GitHub/Gitee/Auto contract fixtures: disagreement, stale mirrors, transient errors, frozen-target fallback, equal-version different-byte rejection, partial publication.
- Malicious receipt/session/ZIP/path fixtures: system root, sibling-prefix tricks, junctions, hard links, ADS, traversal, case aliases, duplicate entries, bombs and TOCTOU replacement attempts.
- Crash injection **before and after each persisted intent and filesystem/registry mutation**, including helper transfer and receipt/registry commit split. Assert either a complete verified runtime or blocked RecoveryRequired, never a normally launched mixed set.
- Fresh install, manual v1.1 adoption, managed update, rollback, both uninstall entry points, default data retention, explicit data deletion, unknown file preservation, corrupt receipt, repeated uninstall and reinstall with a new UUID.
- Main/helper pair replacement on actual Windows 11, file locks/antivirus, space exhaustion, paths with spaces/non-ASCII, abandoned lock, two launches, two helpers and cross-session access.
- Wrong/stale HealthAck nonce/PID/creation time/version/session, hung/crashed child, timeout and live-process verification. Keep old app's DB/settings readable after probation and rollback.
- Windows Installed apps launch/quoting/version readback; Start Menu opt-out and conflict preservation; no HKLM writes or admin prompt.
- Confirm tasks/settings/history/images hashes unchanged by helper update/rollback, external wallpaper unchanged, shared WebView2 untouched, and exact ownership deletion with leftovers reported.
- Smoke all three Standard modes to detect lifecycle regressions without refactoring the frozen native boundary.

Implementation slices are: (1) resource/path/receipt/lock primitives with Windows fault tests; (2) native install/uninstall/recovery and bootstrap; (3) signed protocol and provider adapters; (4) update/HealthAck/rollback with self-replacement QA; (5) release signing/mirror gate and Settings/About UI. Slices (1), (2), and the Phase 1 uninstall-entry portion of (5) are implemented. Slices (3), (4), and the release-signing/mirror portion of (5) remain future work. Do not publish self-update until all mandatory safety gates pass.

### Phase 1 remaining hardening

Recorded so they are not lost with the implementation handoff; none blocks the Phase 1 acceptance:

- Retain validated preimages across interrupted installs for rollback-style recovery (currently only the Installing receipt marks the crash point; full preimage evidence belongs to the update machinery).
- QA isolation must cover WebView2 profile storage in addition to SQLite before a test instance runs against a managed install.
- Path validation does not yet verify the final handle's path, owner, and writable ACL; directory deletion still uses a metadata-checked path-based removal instead of a handle operation.
- The attack-test matrix (junction/reparse swaps, hard-link races, leaf replacement, cross-session locks, helper-crash cleanup) is a foundation, not full coverage; runtime hash verification and deletion still use separate opens.
- Legacy-instance detection is name/image-path based and intentionally fails closed; races and renamed portable copies sharing the database need review.
- The native consent/progress dialogs need a manual visual walkthrough; the Settings entry needs no new features but future polish belongs to the normal UI track.
- Release preparation must verify the flat eight-file payload and the four-way version invariant (`ProductVersion` = `Receipt.currentVersion` = Installed-apps `DisplayVersion` = release version).

### QA isolation principles (standing)

- QA capability is compile-time gated (`qa` / `maintenance-qa` features). Production binaries cannot recognize a QA identity, cannot be redirected to QA roots, and contain no test seams; this is verified per release-style build.
- Every QA harness that can mutate filesystem or registry state runs under a hard interlock: an explicit QA identity, sandbox roots inside the dedicated QA namespace under the real LocalAppData Known Folder, install/data roots that can never alias production, QA-only registry namespaces, and a runtime probe proving the helper binary is a QA build. Any unmet condition refuses the harness before the first mutation.
- The real production installation is snapshotted before and verified after every QA run; a changed production state fails acceptance unconditionally.
- Environment-redirected locations (for example a fake `%LOCALAPPDATA%` for legacy-path script tests) are never relied upon for helper or app code, which always resolve canonical roots from Known Folders.
