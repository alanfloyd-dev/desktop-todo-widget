# Windows App SDK runtime: deployment model, payload provenance, version policy

This document is the authoritative description of how the product obtains the
Windows App SDK runtime used by the optional Windows CompositionController
hosting path. It exists so that a contributor never has to reverse-engineer the
payload from build output, and so that the version/EOL decision is explicit.

## Deployment model: self-contained, not framework-dependent

**Decision: unpackaged + self-contained.**

The runtime is shipped beside `alan-desktop.exe` (the internal executable name; the
public product name is `desktop-todo-widget`) and activated through Undocked
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
# list the pinned payload sets
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -ListRuntimes

# stage one of them (writes the active-runtime selector build.rs reads)
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 2.x
```

The stager holds a **pinned runtime-set table**. Each set names the official
Microsoft component packages by exact version **and** SHA256, from `api.nuget.org`.
The active set is **`2.x`** (migrated in Phase 7C.3-B2):

| Payload set | Component package | Version | SHA256 |
|---|---|---|---|
| `2.x` | `Microsoft.WindowsAppSDK.Foundation` | `2.3.9` | `230BC605A3FC9ED689B2117056C5274923BF58B453FA44EDDE18A168BBF628BE` |
| `2.x` | `Microsoft.WindowsAppSDK.InteractiveExperiences` | `2.1.6` | `DE7B5907C63C8A79606CCC8F0D98943B154A2E62312308187E8CDC3304FF3D0B` |
| `2.x` | `Microsoft.WindowsAppSDK.Base` | `2.0.4` | `E3E13478C4C80C59ED5F8F89542FE49A2985DAA484753E93A5858E90C2D46A4D` |
| `2.x` | `Microsoft.WindowsAppSDK` (umbrella) | `2.4.0` | `6EC2EBB6ADD33ECEBAC1F5773AD4CABE934B82FB18D7BEA98E011BB0FC0A37B9` |
| `1.8` (rollback) | `Microsoft.WindowsAppSDK.Foundation` | `1.8.260803002` | `B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E` |
| `1.8` (rollback) | `Microsoft.WindowsAppSDK.InteractiveExperiences` | `1.8.260708001` | `496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76` |

These come from the Windows App SDK **2.4.0** stable release (2.x line) and the
**1.8.11** release respectively. `Base` and the umbrella package carry no native
payload in this deployment model; they are pinned because the umbrella package
declares them as dependencies of the runtime, so their versions are part of the
provenance record.

For each payload component the stager:

1. downloads the `.nupkg` into `tools/windows-app-sdk/cache/` (git-ignored) and
   fails on any SHA256 mismatch;
2. copies `runtimes-framework/win-x64/native/*` and every `metadata/*.winmd`
   into `tools/windows-app-sdk/runtime/<set>/x64/`;
3. generates the WinRT activation fragment from the packages' own
   `runtimes-framework/package.appxfragment` files — never hand-maintained;
4. asserts that every copied `.dll` has a valid Authenticode signature and fails
   otherwise.

Both payload sets produce 48 files (20 `.dll`, 23 `.winmd`, 2 `.pri`, 2 `.exe`
agent utilities, 1 activation fragment). The two `.exe` files
(`DeploymentAgent.exe`, `RestartAgent.exe`) are not part of this deployment model
and are deliberately **not** copied next to the product executable.

## Versioned payload layout and rollback

```text
tools/windows-app-sdk/runtime/
  active-runtime.txt          generated selector consumed by build.rs (e.g. "2.x")
  1.8/x64/                    Windows App SDK 1.8 payload (kept for rollback)
  2.x/x64/                    Windows App SDK 2.x payload
```

Staging one runtime never deletes another, so A/B comparison and rollback need no
re-download. `-NoActivate` stages a payload without switching the selector.

To roll back to 1.8: restage `-Runtime 1.8` (or edit `active-runtime.txt`) and
rebuild. This path was verified in Phase 7C.3-B2 by rebuilding and launching.

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

- **We are now on a supported runtime.** Phase 7C.3-B2 migrated the payload to the
  Windows App SDK **2.x** line (umbrella 2.4.0: Foundation `2.3.9`,
  InteractiveExperiences `2.1.6`, Base `2.0.4`). See
  `docs/phase-7c3b2-winappsdk-2x-upgrade.md` for the migration evidence, including
  the Acrylic visual proof that validates the 2.x activation manifest.
- **1.8 is out of support** (end of servicing 2026-09-09) and is retained only as a
  staged rollback payload. Do not select it for a release.
- **Next milestone: 2.x end of servicing is 2027-04-29.** Add it to the maintenance
  calendar. Each version is serviced for 12 months.
- **Do not silently bump the pin.** Self-contained deployment is not serviceable and
  only the packaged MSBuild flow is documented for version upgrades, so an upgrade is
  a deliberate, separately verified task.
- The pinned versions, their hashes, and the version table above must be updated
  together; the payload is reproducible from the pins alone.
- Because the payload is application-owned, every future Windows App SDK servicing
  fix reaches users only through a new application release. That is the accepted
  cost of the self-contained model.

### Upgrade procedure (validated in Phase 7C.3-B2)

1. Confirm the current stable release and its EOS date on the
   [release channels](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/release-channels) page.
2. Download the umbrella `Microsoft.WindowsAppSDK` nupkg for that release and read
   its `.nuspec` to get the **exact component versions** it declares. In 2.x the
   umbrella and component packages are versioned independently, so the stable
   component versions will not match the umbrella version number.
3. Get each component's SHA256 from nuget.org and add a new entry to the
   `$RuntimeSets` table in `prepare-runtime-payload.ps1`. Do not edit an existing
   set — add a new one so the previous payload stays available for rollback.
4. Stage it: `prepare-runtime-payload.ps1 -Runtime <set>`. Confirm the SHA256
   checks, the Authenticode assertion, the file count, and that the generated
   fragment still contains the `SystemBackdrops` classes in `wuceffectsi.dll`.
5. Run the build gates and launch both windowed and composition modes in debug and
   release.
6. **Verify Acrylic visually on the Glass + Floating-expanded host.** This is the
   only product combination that creates a `DesktopAcrylicController`, so it is the
   only one that proves the new activation manifest is correct. API/state success
   alone is not sufficient.
7. Only then repoint the selector and retire the previous payload, if desired.

Microsoft publishes no step-by-step X→Y upgrade guide for unpackaged self-contained
apps; the 2.0 release notes only advise removing and re-adding the package reference,
which does not apply to this NuGet-staging approach. Verification therefore comes
from the steps above.

## Known limitations

- `Microsoft.WindowsAppRuntime.dll` reports a file version of `2.0` for the current
  2.x payload; that is the WinAppSDK platform MajorMinor, not the product version.
  The payload is identified by package version + SHA256, not by the DLL file version.
- The payload is x64 only. Adding arm64 means staging a second architecture
  directory and mapping the build target in `build.rs` (`PAYLOAD_ARCHITECTURE`).
- The manifest fragment repeats the `winrtv1` namespace declaration on each
  `activatableClass` element. Harmless but verbose; left as generated.
- The runtime files are copied into the profile output directory by `build.rs`,
  which covers `cargo build` for both profiles. A distribution archive must
  include those files next to the executable; `tauri build` bundling and
  installer work is out of scope for this phase.
