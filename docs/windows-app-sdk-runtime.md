# Windows App SDK runtime: deployment model, payload provenance, version policy

This document is the authoritative description of how the product obtains the
Windows App SDK runtime used by the optional Windows CompositionController
hosting path. It exists so that a contributor never has to reverse-engineer the
payload from build output, and so that the version/EOL decision is explicit.

## Deployment model: self-contained, not framework-dependent

**Decision: unpackaged + self-contained.**

The runtime is shipped beside `alan-desktop.exe` and activated through Undocked
RegFree WinRT. There is no bootstrapper call and no package graph.

Why not framework-dependent:

- `MddBootstrapInitialize` for the Windows App SDK 1.8 Framework returns
  `0x80670016` (package dependency criteria could not be resolved) on the
  development machine, because no `Microsoft.WindowsAppRuntime.1.8` Framework
  package is registered for the interactive user. See `docs/native-composition.md`.
- Framework-dependent unpackaged deployment is the smallest payload but depends
  on machine state the product cannot control. A self-contained payload is the
  only model this project has actually validated end to end.

Trade-off, stated plainly: self-contained adds ~17 MB of runtime files next to
the executable, and Microsoft documents that this deployment mode is **not
serviceable** — "The Windows App SDK version distributed with your app can be
updated only by releasing a new version of your app. You're responsible for
integrating servicing updates of the Windows App SDK into your app."
([deployment overview](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/deploy-overview))

The default windowed hosting path does not use this runtime at all. Acrylic and
the composition host are the only consumers.

## Payload provenance

Nothing about the payload is copied from another project's build output, and no
runtime binary is committed to the repository. The payload is downloaded and
verified on demand by:

```powershell
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1
```

The stager pins two official Microsoft component packages by exact version **and**
SHA256, from `api.nuget.org`:

| Component package | Version | SHA256 |
|---|---|---|
| `Microsoft.WindowsAppSDK.Foundation` | `1.8.260803002` | `B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E` |
| `Microsoft.WindowsAppSDK.InteractiveExperiences` | `1.8.260708001` | `496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76` |

These are the Windows App SDK **1.8.11** component packages; the resolved runtime
version is **1.8.260804001**.

For each package the stager:

1. downloads the `.nupkg` into `tools/windows-app-sdk/cache/` (git-ignored) and
   fails on any SHA256 mismatch;
2. copies `runtimes-framework/win-x64/native/*` and every `metadata/*.winmd`
   into `tools/windows-app-sdk/runtime/x64/`;
3. generates the WinRT activation fragment from the packages' own
   `runtimes-framework/package.appxfragment` files — never hand-maintained;
4. asserts that every copied `.dll` has a valid Authenticode signature and fails
   otherwise.

Output: 48 files (20 `.dll`, 23 `.winmd`, 2 `.pri`, 1 activation fragment,
2 `.exe` agent utilities) in `tools/windows-app-sdk/runtime/x64/`, all
git-ignored. The two `.exe` files (`DeploymentAgent.exe`, `RestartAgent.exe`) are
not part of this deployment model and are deliberately **not** copied next to the
product executable.

## Build pipeline

`src-tauri/build.rs` consumes the staged payload and produces a self-consistent
build output directory for **both** profiles:

1. Reads `src-tauri/app.manifest` (the product's own base manifest) and splices in
   the staged `composition-host.manifest` activation fragment before
   `</assembly>`. The merged result is tag-balance validated and then embedded
   into the executable by `tauri-build`. The merge is a real XML operation; a
   text-only copy of element names drops `</asmv3:file>` tags and yields a
   manifest Windows rejects with a side-by-side configuration error.
2. Copies every `.dll` / `.winmd` / `.pri` from the payload into the profile
   output directory (`target/debug/` or `target/release/`) beside the executable.
3. **Fails the build** when the payload is missing or incomplete, naming the
   exact command that produces it. Shipping an executable that cannot start its
   composition path must not be possible.

`src-tauri/app.manifest` therefore holds only the product's own requirements
(Common Controls v6). The Windows App SDK activation classes live in the
generated fragment, so the activatable-class list cannot drift from the runtime
that actually ships.

Verified: a `cargo build --release` into an empty `target/release/` produces
`alan-desktop.exe` plus 46 runtime files, with **no** extra agent executables and
**no** dependency on the repository tree at runtime. The release executable
launches from a copied directory outside the repository and reaches
`[wry] webview_controller_type=ICoreWebView2CompositionController` with
`windows_app_runtime_preloaded=true`.

## How the runtime is initialized

The runtime is loaded with `LoadLibraryW("Microsoft.WindowsAppRuntime.dll")` and
its exported `WindowsAppRuntime_EnsureIsLoaded` entry point, from `run()` before
any window or WebView exists (see `composition_host::runtime` for why the timing
is load-bearing: calling `LoadLibraryW` from inside Wry's WebView creation stack
deadlocks on the Windows loader lock).

Honest caveat for reviewers: `WindowsAppRuntime_EnsureIsLoaded` is a real,
non-private export, but it has **no Microsoft Learn API reference page** and its
own implementation literally does nothing — its source is:

```cpp
// Do nothing. Just exist so consumers can import a reference to ensure the DLL is loaded
STDAPI WindowsAppRuntime_EnsureIsLoaded() { return S_OK; }
```

([WindowsAppSDK source](https://github.com/microsoft/WindowsAppSDK/blob/main/dev/WindowsAppRuntime_DLL/WindowsAppRuntime_EnsureIsLoaded.cpp))

It exists only so that Microsoft's Undocked RegFree WinRT auto-initializer has
something to import. The **documented, supported** mechanism for this deployment
model is Undocked RegFree WinRT, driven by the `WindowsAppSdkUndockedRegFreeWinRTInitialize`
project property in MSBuild, or — for a non-MSBuild build like this one — by
embedding the WinRT activation manifest, which is exactly what `build.rs` does.
Treat the manifest as the contract and `EnsureIsLoaded` as the DLL-load trigger.

Related documented caveat: on the explicit-load path, a damaged self-contained
configuration causes `DebugBreak(); exit(HRESULT_FROM_WIN32(lastError));` — a
hard process exit rather than a graceful error. That is why the stager verifies
signatures and `build.rs` verifies required files at build time.

## Version and end-of-support policy

The Windows App SDK is governed by the
[Microsoft Modern Lifecycle](https://learn.microsoft.com/en-us/lifecycle/policies/modern).
Each release is serviced for **12 months**. The authoritative table is
[Windows App SDK release channels](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/release-channels)
(“Release lifecycle”):

| Version | Original release | Latest patch | Patch date | Support level | End of servicing |
|---|---|---|---|---|---|
| 2.0 (2.x line) | 04/29/2026 | 2.4.0 | 08/13/2026 | Current | 04/29/2027 |
| **1.8** | 09/09/2025 | 1.8.260804001 | 08/13/2026 | Maintenance | **09/09/2026** |
| 1.7 | 03/18/2025 | 1.7.260224002 | 03/10/2026 | Out of Support | 03/18/2026 |

**Policy for this repository:**

- **We are on an out-of-support runtime.** Windows App SDK **1.8 reached end of
  servicing on 2026-09-09**. The latest stable line is **2.x** (latest stable
  **2.4.0**, released 08/13/2026, end of servicing 04/29/2027). Microsoft's own
  page still labels 1.8 "Maintenance" and still files it under supported
  downloads, but the published end-of-servicing date has passed; do not treat 1.8
  as supported.
- **Do not silently bump the pin.** Self-contained deployment is not serviceable
  and only the packaged MSBuild flow is documented for version upgrades, so the
  upgrade is a deliberate, separately verified task (see below).
- The pinned versions, their hashes, and the version table above must be updated
  together; the payload is reproducible from the pins alone.

### Upgrading to the 2.x line (next task, not done here)

1. Get the exact component package names, versions and SHA256 values for the
   target Windows App SDK release from nuget.org.
2. Update `$FoundationPackage` / `$InteractivePackage` / `$ResolvedRuntimeVersion`
   in `prepare-runtime-payload.ps1` and the table above.
3. Re-run the stager; confirm the signature check passes and the fragment still
   contains the `SystemBackdrops` classes.
4. `cargo build --release` and confirm the release directory is self-consistent.
5. Re-run the composition smoke test on an interactive desktop, specifically
   covering the **Glass + Floating-expanded** host, because that is the only
   combination that creates a `DesktopAcrylicController` and therefore the only
   one that proves the manifest's Acrylic activation entries resolve.

Microsoft publishes no step-by-step X→Y upgrade guide for unpackaged
self-contained apps; the 2.0 release notes only advise removing and re-adding the
package reference, which does not apply to this NuGet-staging approach. Verification
must therefore come from the steps above.

## Known limitations

- Windows App SDK 1.8 is out of support as of 2026-09-09; upgrade to 2.x pending.
- The payload is x64 only. Adding arm64 means staging a second directory and
  mapping the build target in `build.rs` (`PAYLOAD_ARCHITECTURE`).
- The manifest fragment repeats the `winrtv1` namespace declaration on each
  `activatableClass` element. Harmless but verbose; left as generated.
- The runtime files are copied into the profile output directory by `build.rs`,
  which covers `cargo build` for both profiles. A distribution archive must
  include those files next to the executable; `tauri build` bundling and
  installer work is out of scope for this phase.
