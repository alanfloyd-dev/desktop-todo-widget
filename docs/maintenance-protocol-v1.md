# Maintenance protocol v1

Status: **design contract for v1.2.0; no shipping implementation**. See [Application Lifecycle & Maintenance Architecture](application-lifecycle.md) for ownership, state machines, recovery and rollout decisions. Examples use synthetic sizes/hashes; they are not installable release metadata.

## Encoding and validation

All JSON documents are bounded UTF-8 without BOM, with duplicate object keys rejected and no comments. UUIDs are canonical lowercase hyphenated UUIDs; timestamps are RFC 3339 UTC; versions are SemVer without the tag's leading `v`; hashes are 64 lowercase hexadecimal characters; sizes are nonnegative integer byte counts representable exactly (maximum 2^53-1). Stable protocol 1 disallows prerelease/build suffixes for delivered versions. Every required size is also checked against smaller compiled resource limits.

Initial limits: manifest 256 KiB, signature envelope 4 KiB, receipt/session 256 KiB each, HealthAck 4 KiB, ZIP 256 MiB, total expanded data 512 MiB, each managed EXE 128 MiB, maximum 64 ZIP entries, maximum expansion ratio 100:1. Apply bounds while streaming, not after allocating/extracting. Changing these defaults requires reviewed policy and QA.

Required fields must exist with correct types. Unknown optional fields may be ignored, but unknown resource identities, enum values, schemaVersion or updaterProtocol are rejected for mutation. Extension fields never grant new actions. Never use object member order as semantics. Signed manifest bytes are preserved unchanged even if JSON whitespace differs from examples.

## UpdateManifest

```json
{
  "schemaVersion": 1,
  "appId": "net.alanfloyd.desktop",
  "channel": "stable",
  "version": "1.2.0",
  "publishedAt": "2026-10-01T00:00:00Z",
  "notes": "Application lifecycle management.",
  "updaterProtocol": 1,
  "assets": {
    "windows-x64": {
      "filename": "desktop-todo-widget-v1.2.0-windows-x64.zip",
      "size": 3000000,
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "installFiles": [
        {
          "identity": "mainExecutable",
          "filename": "desktop-todo-widget.exe",
          "size": 5500000,
          "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        {
          "identity": "maintenanceHelper",
          "filename": "desktop-todo-maintenance.exe",
          "size": 800000,
          "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        }
      ]
    }
  }
}
```

The manifest contains no provider URLs, destination paths, shell commands, registry paths, or uninstall actions. Both fixed identities are required exactly once. Each filename must match the compiled identity map; version, package name and platform must agree. `notes` is bounded plain text, never executable HTML. Unknown platforms may be ignored by an unsupported client, but windows-x64 must exist for this client's update.

`update-manifest.json.sig` is a bounded JSON envelope with `schemaVersion: 1`, `algorithm: "Ed25519"`, `keyId: <known key ID>`, `signature: <base64 encoding of exactly 64 bytes>`. Parsing this small untrusted envelope merely selects an already embedded key; it cannot import keys or select arbitrary algorithms. The signature covers exactly the raw bytes of `update-manifest.json`, not the envelope or a reserialized document. Unknown key/algorithm fails closed. Use strict verification from a mature crypto library. Manifest and signature stay outside the ZIP to avoid a self-referential package hash.

Main backend order: bounded download -> verify signature -> strict parse -> validate app/channel/version/schema/protocol/platform/policy -> freeze manifest digest -> resolve exact asset -> download/hash -> ZIP structure validation -> extract only the two managed files -> verify each file. Helper repeats signature and policy checks, then staged and copied replacement hashes, and finally installed hashes. `session.json` never substitutes for signed bytes.

## InstallationReceipt

| Field | Type / meaning |
| --- | --- |
| schemaVersion | Integer `1`; independent of manifest/session schema |
| appId | Exact `net.alanfloyd.desktop` |
| installationId | UUID, generated at first managed installation |
| currentVersion | Last committed version; nullable only in initial Installing state |
| lifecycleState | Installing / Installed / Updating / Uninstalling / RecoveryRequired |
| installRoot | Absolute snapshot; must match independently derived canonical root |
| dataRoot | Absolute Roaming app-data snapshot; never a deletion capability |
| installedAt / updatedAt | UTC timestamps; metadata, not authority |
| runtimeResources | Array of unique `{identity, version, size, sha256}` for current managed EXEs |
| supportResources | Exact policy-known optional documentation identities with recorded hashes; empty allowed |
| integrationResources | Identity and desired present/absent state; shortcut opt-out retained |
| maintenance | `{updaterProtocol, receiptGeneration, activeSessionId, lastCompletedSessionId, committedManifestSha256}`; nullable session/digest fields where not yet applicable |

The receipt does not carry an arbitrary deletion list. Runtime hashes describe observed installation state; publisher authentication comes from verified manifests, not these hashes. Verify them against actual files and retained signed release evidence before update/rollback. Atomically replace receipt generations and preserve prior generation in active recovery state. During uninstall retain the receipt until requested owned resources are removed or explicitly reported as incomplete.

For the initial manual trusted bootstrap, committedManifestSha256 and installed manifest/signature evidence may be absent: the ZIP does not contain the sibling release assets. Its validated local preimage hashes support recovery, not publisher authentication. The first self-update still requires a signed target manifest and retains the bootstrap preimage for rollback; successful commit establishes signed installed-release evidence. See [bootstrap trust boundary](application-lifecycle.md#15-installtransaction-and-bootstrap).

## UpdateSession and journal

Location is derived from canonical maintenance root plus validated session UUID. The handoff may carry `--update --session-id <UUID>`; there is no arbitrary `--execute-script` or authoritative `--install-root`. `--install`, `--uninstall`, and `--recover --session-id <UUID>` select fixed operations. The selected operation must agree with the persisted session type.

| Field | Contract |
| --- | --- |
| schemaVersion / updaterProtocol | Independent integers, both supported before mutation |
| operation / sessionId / installationId / appId | Typed operation and bindings validated against installation |
| fromVersion / toVersion | Frozen previous/target versions; target equals signed manifest |
| source | Discovery source plus actual download source/attempt metadata; never authority |
| manifestSha256 / packageSha256 / packageSize | Bind exact accepted manifest and ZIP; independently recomputed |
| phase / generation | State enum and monotonically increasing write generation |
| installRoot / stagingRoot | Absolute diagnostic snapshots, validated against derived locations |
| createdAt / updatedAt | UTC metadata |
| healthNonce | Random 256-bit secret, stored only in user-restricted session state, never logs/CLI |
| parentProcess / probationProcess | PID, process creation time, exact canonical image identity; PID alone is insufficient |
| resources | Identity-indexed old/new hash/size, old-present flag, operation intent/completion and fixed backup slot |
| previousReceipt / previousIntegration | Validated snapshots for rollback, never instructions or arbitrary registry values |
| acceptedHealth / commitIntent | Durable helper-validated outcome and decision boundary, absent before validation |
| lastError | Bounded structured operation/resource/Win32 code; privacy-filtered message |

`resources` may reference only fixed slots derived as `.maintenance/<UUID>/<identity>.<role>`. No arbitrary relative destination is accepted. An unexpected preimage hash is a repair conflict, not permission to overwrite unknown content. A fresh install's absent preimage allows removal only of that transaction's verified newly created file.

Session writes follow write-new-generation -> flush -> atomic publish. Persist intent before mutation and result after verifying the actual state. Recovery handles a missing result as ambiguous intent by inspecting old/new/backup hashes. Torn or conflicting generations never get merged speculatively. Keep source and destination file handles/identities through critical operations wherever Windows API semantics allow.

UninstallSession shares envelope fields and logging, but has cleanup progress per owned identity and current-interaction data consent instead of update target/hash/probation. Consent records are audit metadata, not authority after restart. RecoverySession records the parent interrupted session and decision; it does not introduce a more permissive path policy.

## HealthAck

```json
{
  "schemaVersion": 1,
  "sessionId": "c9dccaf6-4532-43be-9e12-3064876ef5b1",
  "installationId": "e80a74ca-2a09-47cb-bcba-a6766fbc4b08",
  "version": "1.2.0",
  "nonce": "<base64 32 random bytes from authorized child environment>",
  "pid": 12345,
  "processCreatedAt": "<Windows process creation FILETIME as decimal string>",
  "initializedAt": "2026-10-01T00:00:05Z"
}
```

The nonce and processCreatedAt placeholders are descriptive, not valid fixture values. Helper retains the process handle returned when it creates the child, verifies it is alive and matches image, PID and creation time, and accepts only the marker for its current AwaitingHealthAck phase. Delete/reject previous markers before a new probation attempt, rotate nonce, and never use timestamp freshness alone. Initial timeout policy is 60 seconds measured monotonically from successful process creation. Timeout triggers rollback, not indefinite waiting or automatic commit. Windows QA may adjust the default before protocol 1 ships; no session-controlled unbounded timeout.

The child receives `DTW_MAINTENANCE_SESSION_ID`, `DTW_MAINTENANCE_NONCE`, and a fixed-purpose launch mode only in its own environment block. It validates current installation/session bindings, does not forward these variables, and writes health only after core initialization. These values do not authorize filesystem operations. During probation ordinary user mutations remain gated until the helper publishes committed or rollback state.

The marker proves an initialization checkpoint, not future reliability. If the child exits before acceptance, rollback. If it crashes after durable commit, ordinary crash recovery applies; Maintenance does not automatically undo arbitrary later user data changes.

## Compatibility and commit rules

Schema 1 does not imply protocol 1 support. Main app and currently running helper each advertise compiled supported protocols and independently reject unsupported targets. Protocol 1 manifests cannot remove old required resources or add new managed identities by suggestion; such changes require an explicitly supported policy/protocol evolution and bridge release.

Only stable versions are currently delivered; compare semantic components, not lexicographic strings. CI verifies all source versions and PE ProductVersion. Normalize the PE numeric trailing revision zero for the steady-state invariant; reject other unexpected version discrepancies. Future beta channels/version encoding need an explicit extension.

Update success means accepted HealthAck **and** durable target receipt **and** verified Windows integration **and** committed journal. Until then keep backups and admission closed. After committed state, failed ephemeral cleanup is a warning rather than runtime rollback. Uninstall success means requested known owned resources were cleaned and the receipt retired; unknown residuals and deferred temporary-runner removal are disclosed separately.
