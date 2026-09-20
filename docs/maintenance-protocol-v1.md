# Maintenance protocol v1

Status: **contract for v1.2.0**. The Phase 1 subset below is implemented (commit `b381963`). The signed-update subset is **frozen protocol design (Phase 2A final freeze)**: every wire-level decision in this document is binding for the implementation, which has not started — no network code, provider adapter, download/staging, `UpdateSession`, HealthAck, or rollback code exists. See [Application Lifecycle & Maintenance Architecture](application-lifecycle.md) for ownership, state machines, recovery and rollout decisions. Examples use synthetic sizes/hashes; they are not installable release metadata.

**Implemented in Phase 1:** the `InstallationReceipt` codec and its validation, the manual bootstrap install transaction (with the bootstrap trust exception), the idempotent uninstall transaction with keep/remove local-data semantics, launch admission against the receipt, and Windows integration reconciliation. **Frozen as Phase 2A protocol design, not implemented:** `UpdateManifest` and signature envelopes, release sources, package download/staging, `UpdateSession`, HealthAck, rollback, and key rotation.

## Encoding and validation

All JSON documents are bounded UTF-8 without BOM, with duplicate object keys rejected and no comments. UUIDs are canonical lowercase hyphenated UUIDs; timestamps are RFC 3339 UTC; versions are SemVer without the tag's leading `v`; hashes are 64 lowercase hexadecimal characters; sizes are nonnegative integer byte counts representable exactly (maximum 2^53-1). Stable protocol 1 disallows prerelease/build suffixes for delivered versions. Every required size is also checked against smaller compiled resource limits.

Initial limits: manifest 256 KiB, signature envelope 4 KiB, receipt/session 256 KiB each, HealthAck 4 KiB, ZIP 256 MiB, total expanded data 512 MiB, each managed EXE 128 MiB, maximum 64 ZIP entries, maximum expansion ratio 100:1. Apply bounds while streaming, not after allocating/extracting. Changing these defaults requires reviewed policy and QA.

Required fields must exist with correct types. **Unknown-field policy is strict and uniform: protocol documents parse into closed structures, and unknown fields are rejected, not ignored.** Extension fields never grant new actions. Never use object member order as semantics. Signed manifest bytes are preserved unchanged even if JSON whitespace differs from examples. Strict struct deserialization is the rejection mechanism: protocol documents parse into closed structures whose deserializers reject duplicate fields, missing required fields, unknown fields, and trailing input.

| Document | Unknown-field policy |
| --- | --- |
| Signed envelope | Reject |
| Manifest top level | Reject |
| Selected platform asset object | Reject |
| `installFiles` / resource entries | Reject |
| UpdateSession | Reject |
| HealthAck | Reject |
| InstallationReceipt | Reject (`deny_unknown_fields`, Phase 1 implemented) |

The single open dimension is the manifest `assets`/platform **map**: it may contain platform keys this client does not recognize, and a Windows client ignores every platform except its own — but the selected `windows-x64` object itself is strictly closed. Adding any field is a schemaVersion bump, never something a schema-1 parser absorbs silently.

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

The updater distributes the standard flat release ZIP — the same release payload the manual bootstrap accepts — and no second update-package format exists. **The archive entry set MUST equal the compiled protocol package allowlist exactly** (currently nine entries: `desktop-todo-widget.exe`, `desktop-todo-maintenance.exe`, `install.ps1`, `uninstall.ps1`, `README.md`, `README_ZH.md`, `LICENSE`, `LICENSE_ZH.md`, `THIRD_PARTY_NOTICES.md`). The count itself is not the security property — exact allowlist equality is. A tenth — any non-allowlisted — entry rejects the entire package. The archive carries root-level regular files only: zero directories, zero nested paths, no absolute paths, no parent traversal, no backslash aliases, no ADS/colon forms, no trailing-dot/trailing-space aliases, no symlink/reparse-style entries, no case-insensitive duplicate logical names, only explicitly allowed compression methods, and bounded total uncompressed size. If a future protocol adds a legitimate support file, the package-allowlist rule must be extended by that same protocol change; a parser may never grow tolerant.

**Package authenticity set ≠ runtime mutation set.** The helper verifies the complete ZIP against the exact allowlist and the signed manifest hashes, but Protocol 1's self-update runtime mutation set is exactly the two executables. The seven distribution/support entries (`install.ps1`, `uninstall.ps1`, `README.md`, `README_ZH.md`, `LICENSE`, `LICENSE_ZH.md`, `THIRD_PARTY_NOTICES.md`) are validated as package entries yet never enter the runtime replacement or rollback transaction, are never extracted to the install root by an automatic update, and are never executed. An already-installed support snapshot may remain at its old version: self-update does not promise to refresh installed README/license/script/notice copies, and the rollback transaction is never widened for support files. Extraction happens only into the session staging root, and every staged managed file is re-verified at copy/replace time ([path safety](application-lifecycle.md#12-path-safety)).

`update-manifest.json.sig` is a bounded JSON envelope with `schemaVersion: 1`, `algorithm: "Ed25519"`, `keyId: <known key ID>`, `signature: <base64 encoding of exactly 64 bytes>`. **Canonical base64 form (frozen):** the signature text uses the RFC 4648 standard alphabet with canonical padding, contains no whitespace, decodes to exactly 64 bytes, and re-encoding the decoded bytes must reproduce the original string exactly — any input a tolerant decoder would accept but that fails this canonical round-trip is rejected. Protocol 1 carries exactly one signature per envelope; multi-signature envelopes are rejected. Parsing this small untrusted envelope merely selects an already embedded key; it cannot import keys or select arbitrary algorithms. Unknown key/algorithm fails closed. Use strict verification from a maintained crypto library; cryptographic primitives are never hand-implemented. `keyId` is the 64-character lowercase hexadecimal SHA-256 digest of the 32-byte raw Ed25519 public key — a derived, self-describing identifier. Two compiled keys can therefore share an id only through a SHA-256 collision; a trust store built or read with duplicate ids is malformed and fails closed. The concrete key values, repository addresses, and crypto crate are pinned and reviewed at implementation time ([release requirements](application-lifecycle.md#16-release--ci-requirements)); Ed25519 itself is the frozen algorithm choice. Manifest and signature stay outside the ZIP to avoid a self-referential package hash. The envelope carries no transport metadata: no provider URLs, release ids, or download hosts.

### Canonical signed bytes

The signature covers **the exact raw bytes of `update-manifest.json` as served by the source**, and nothing else — never the envelope, never a reserialized, re-encoded, or whitespace-normalized document. There is no canonical-JSON layer and therefore no serializer, field-order, or whitespace ambiguity to mis-implement. The verified byte string is preserved byte-identically: first in the session staging directory, then, on commit, at the installed evidence paths; the helper always re-verifies the signature over those persisted bytes before parsing them. Verification and parsing are ordered so the parser can only ever see the byte string the signature was verified over:

1. Bounded fetch of the raw manifest bytes (≤ 256 KiB) and raw envelope bytes (≤ 4 KiB); both are kept as opaque byte strings.
2. Strict-parse the envelope: untrusted input that only selects a compiled key and algorithm, both of which must already be known or verification fails.
3. Verify the Ed25519 signature over the raw manifest bytes with that compiled key.
4. Validate the same bytes' encoding: valid UTF-8, no BOM, within the size bound — the same rules as every protocol JSON document.
5. Strict-parse the same bytes into the manifest structure: duplicate object keys and trailing bytes are rejected by the closed struct deserialization.
6. Validate semantics (app, channel, version, schema, protocol, platform, policy, asset and file entries), then freeze the target: SHA-256 of the raw manifest bytes plus every identity field.

A failure at any step ends the candidate; no repair, re-encoding, or retry-with-different-bytes path exists.

The schemaVersion-1 manifest field set is frozen: `schemaVersion`, `appId`, `channel`, `version`, `publishedAt`, `notes`, `updaterProtocol`, and `assets` (per platform: `filename`, `size`, `sha256`, `installFiles`). Adding any field is a schemaVersion 2 event. A parser that understands schema 1 may ignore object fields it does not know, but unknown `schemaVersion` or `updaterProtocol`, unknown enum values, and unknown `installFiles` identities are always rejected — an entry naming a file outside the compiled identity map can never become a managed resource.

### Version selection

`version` in the signed manifest is the security fact; provider release titles, tags, and ordering are discovery metadata with no authority. Numeric SemVer components are compared, never lexicographic strings. Protocol 1 delivers stable versions only: a manifest carrying a prerelease or build suffix is malformed. Against the installed version: a greater target is an update; an equal target means "up to date" — same-version reinstall through the updater does not exist (explicit repair is a manual bootstrap operation); a lower target is refused as a downgrade. No source adapter can reclassify these outcomes, and any future downgrade or recovery delivery must be an explicit protocol extension, never a discovery-side behavior. Because signatures prove authenticity, not freshness, a stale-but-genuine manifest may be replayed at a client indefinitely; refusing below-installed versions is the complete and deliberate response, and update availability (being shown the newest release at all) is not a property this protocol claims.

### Installed version authority

The installed version is defined by a **consistency invariant among mutually verifying security facts, not a priority chain** ("signed evidence beats receipt beats PE" is explicitly not the rule). In a managed installation that has committed signed evidence:

```text
signed committed manifest targetVersion
  == Receipt.currentVersion
  == main executable PE ProductVersion   (normalized: 1.2.0 == 1.2.0.0)
  == Installed-apps DisplayVersion       (steady state)
```

Any inconsistency means: no normal candidate selection may run, the freshness anchor must not be lowered by editing the receipt, no "pick whichever looks most trustworthy and continue" path exists — the installation is in a repair/recovery conflict, and `RecoveryRequired` when it cannot be proven consistent.

Between the manual trusted bootstrap and the first successful signed self-update there may be no committed signed manifest evidence. The **provisional anchor** then requires at least `Receipt.currentVersion == main EXE ProductVersion` (same normalization); any mismatch fails closed.

A signature proves publisher authorization, never freshness or newestness. Every version comparison uses the one canonical comparator implemented in Phase 1.5 (`maintenance::version::Version`: three numeric stable components, u16, no prerelease/build suffix, no leading zeros). Protocol 1 introduces no semver crate and no second comparator.

Main backend order: bounded download -> verify signature -> strict parse -> validate app/channel/version/schema/protocol/platform/policy -> freeze manifest digest -> resolve exact asset -> download/hash -> ZIP structure validation -> extract only the two managed files -> verify each file. Helper repeats signature and policy checks, then staged and copied replacement hashes, and finally installed hashes. `session.json` never substitutes for signed bytes.

**Candidate enumeration and eligibility.** A ReleaseSource exposes bounded candidate enumeration — conceptually `listCandidates(channel, boundedLimit)` plus an exact-version asset resolver — never an open-ended "fetch the latest" query. Enumeration proceeds newest-to-oldest, and **the freeze happens only after the complete candidate-eligibility predicate succeeds**, never after signature/protocol validation alone:

```text
discover bounded candidates
  → for candidate newest → oldest:
      publisher-authorized        (trusted key + valid signature over exact bytes)
      AND schema/protocol supported
      AND version > current       (canonical comparator; no downgrade/equal)
      AND source→target hop eligible  (persistent-data rollback compatibility
                                       against the actual installed source version)
      AND current trust-store/key-rotation path permits it
  → all pass
  → accept + trusted-target-persisted + FREEZE
```

A candidate that fails only eligibility is **candidate-ineligible; the bounded enumeration continues** to the next-older candidate: keyId not trusted by this client, updaterProtocol not executable, target version ≤ current, source→target hop not eligible, platform not applicable. A candidate that is malformed or fails verification is **candidate-invalid; that candidate is rejected and the bounded enumeration may likewise continue** through the same provider's older candidates — malformed envelope/manifest, bad signature, canonical base64 violation, semantic manifest violation. The rationale is deliberate: the provider sits outside the trust boundary, so a compromised publisher account can publish garbage as its latest; letting that garbage block a still-valid, correctly signed bridge adds no security and only grants the provider a denial-of-service lever. The bounded limit caps the scan. What can never continue is the target itself: **after `trusted-target-persisted`, any manifest or package disagreement is a session failure — the target is never switched** (the exact-byte resume rule above; the frozen-tuple fallback rules below). This scan predicate is also what makes key-rotation bridges reachable (see [Trust store and key rotation](#trust-store-and-key-rotation)); it is normal candidate selection, not trust fallback.

After the target freezes, artifact transport failures may retry and may continue from the alternate provider **only** for the exact frozen tuple (version, platform, package filename, size, SHA-256, manifest digest); every mirror attempt re-hashes the full package against the manifest. Three fallback kinds are distinct: candidate fallback (choosing which release to pursue) happens only during discovery, before any manifest is accepted; artifact mirror fallback (which host serves the already-chosen bytes) is allowed under exact-match conditions only; trust fallback (accepting a target that failed verification because another source vouches for it) is forbidden and does not exist. Different manifest bytes for the same version from another provider are a mirror-integrity failure that ends the session — never "pick the one that downloads". Provider metadata (release id, tag, published timestamps, URLs) never overrides a signed manifest field. The user's GitHub/Gitee/Auto choice is a transport preference only; all sources meet the identical signature trust, and Auto is a discovery policy, not a third trust mechanism. The helper never sees any of this: it is an offline executor that knows only the session id, canonical staging root, signed bytes, its compiled trust store, the current receipt, and the local package — and it re-verifies signature, protocol, target identity, and every hash itself. A "verified" claim from the main app is coordination, not evidence.

**Auto policy (frozen).** Auto runs a complete bounded candidate discovery against GitHub. Only when the GitHub discovery layer is unavailable as a whole, and no trusted/frozen target exists yet, may Auto switch to Gitee for candidate discovery. Auto never queries both providers and compares versions, and never re-queries Gitee for a higher version because GitHub returned a valid-but-older result — that availability tradeoff is a recorded design property, not a defect. Once any provider produces an accepted trusted target, the target is frozen immediately; the other provider can then serve only as an artifact mirror for the exact frozen tuple, and candidate discovery never re-runs to replace it.

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

**Phase 1 implementation decisions (binding for the implemented codec):**

- The receipt uses `deny_unknown_fields` with a closed resource/lifecycle enum. This is deliberately stricter than the generic extension-field rule above: for the receipt specifically, an unknown field or identity indicates a foreign or future schema and must fail deserialization rather than be ignored, because receipt fields carry deletion-adjacent authority.
- An `Installing` receipt is published before the first mutation and carries the **target version** in `currentVersion`; `currentVersion` is nullable only in that initial state. The Installing receipt is the durable crash marker: rerunning the same trusted installer resumes it (keeping `installationId`), and a receipt claiming a newer version than the installer refuses as a downgrade.
- Support-resource ownership from a previous receipt is inherited across reinstall only for compiled-known support identities whose on-disk file still matches the recorded size and SHA256; everything else becomes unowned. Unknown resources are never adopted into any recorded list.
- Local SHA256 fingerprints in `runtimeResources`/`supportResources` are recovery/ownership evidence only, exactly as the design states; publisher authentication never derives from them.

## UpdateSession and journal

Location is derived from canonical maintenance root plus validated session UUID. The handoff may carry `--update --session-id <UUID> --expected-manifest-sha256 <64 hex>`; there is no arbitrary `--execute-script` or authoritative `--install-root`. `--install`, `--uninstall`, and `--recover --session-id <UUID>` select fixed operations. The selected operation must agree with the persisted session type.

**Frozen target binding.** `--expected-manifest-sha256` is the non-path immutable value that binds the helper to the exact target the user approved: the SHA-256 of the raw signed manifest bytes at consent time. The helper resolves the sessionId only under the canonical staging root, reads the persisted raw manifest/envelope, independently verifies the signature over those bytes, recomputes the manifest digest, and requires exact equality with the CLI value — then persists that digest into its own journal **before** any destructive work. A session directory swapped to a different otherwise-valid signed target fails this equality check and is refused. The CLI value is a binding check, not an authority source: the helper trusts only its own re-verification of the persisted signed bytes; the expected digest only decides *which* verified target is the approved one.

The session UUID is an opaque locally generated identity and the only handle a caller may name. The future frontend command surface is exactly `check_for_updates`, `download_update`, and `install_ready_update(sessionId)`; a sessionId is resolved strictly against the local session registry (the validated sessions directory), never accepted or constructed as a path. Package, manifest, envelope, and staging locations derive solely from canonical roots plus that UUID; frontend, provider, or session content can never name a download, staging, or install path.

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

**Receipt/session boundary.** The InstallationReceipt describes what this machine has installed; the UpdateSession describes what an update is trying to turn it into. They never merge. The session records the frozen target, source metadata, package digests, phase, preimage identity, and probation state — as coordination and diagnostics. It holds no resource authority: replacement and deletion permission still derive only from compiled policy, independently validated canonical paths, a valid current receipt, and the re-verified signed manifest. Because the session lives in user-writable space, every binding it carries (roots, versions, hashes, slot paths) is re-derived or recomputed at use; a session whose claims disagree with independently derived state is a recovery conflict, never an instruction. An edited session can therefore only make the updater refuse, never widen it.

**Durable milestones.** Discovery, candidate selection, and manifest fetch/verify are network-and-memory operations; nothing about a rejected or still-unverified candidate is ever durable. The first durable event is **trusted-target-persisted**: the moment verification of a candidate succeeds, the updater atomically publishes the session record containing the raw signed manifest/envelope bytes and the frozen target digest under the session directory — before any package download begins. That durable write, not the in-memory verification, is what makes the target frozen; a crash before it leaves only an incomplete session directory for bounded cleanup, with no durable state and no admission effect. Download progress itself stays in memory: a crash during download loses only the partial file. The durable milestones with crash-recovery meaning are:

| Durable milestone | Why durable | Crash semantics | Resume |
| --- | --- | --- | --- |
| trusted-target-persisted | Freezes the only target this session may install and makes the exact signed bytes durable | Re-open: re-verify the signature over the persisted bytes; reuse or re-fetch the exact artifact | Idempotent; no install mutation yet |
| staged | Marks package and extracted payload fully verified by the helper, so applying needs no network | Re-verify staged hashes; any mismatch means re-download or fail | Idempotent |
| handed-off (receipt `Updating` + helper journal) | Transfers authority to the helper; admission must stay closed from here | The helper journal decides per the [recovery boundaries](application-lifecycle.md#9-durable-journals-and-recoverytransaction) and the [receipt/journal lattice](#receiptjournal-write-ordering-and-crash-lattice) | Helper-only resume/rollback |
| committed | Publishes the target receipt and signed evidence; ends probation | Finish integration/receipt reconciliation; cleanup later | Idempotent reconciliation |
| rolled-back / recovery-required (terminal) | Final verdict; `rolled-back` preserves old-install launchability | Verify the restored old set; bounded cleanup; diagnostics retained | No resume; a new session is required |

**`Failed` is bounded by the handoff.** A session may end `Failed` only before the destructive handoff (trusted-target-persisted or staged milestones: discovery, transport, verification, staging, user-refusal, LocalConflict) — the installed runtime is untouched and a new session may be started. Once the receipt is `Updating` and the destructive mutation has begun, `Failed` does not exist: every terminal outcome is exactly `Committed`, `RolledBack`, or `RecoveryRequired`. No ambiguous `Failed` state is legal after a partial apply.

**Resume is exact-byte.** Once `trusted-target-persisted`, the UpdateSession is permanently bound to the exact persisted raw signed manifest bytes, their SHA-256, and the frozen target. Every resume path reads the persisted raw bytes, independently verifies the signature again, recomputes the digest, and continues only when all bindings still match. Forbidden on resume: re-fetching "the same" manifest from the network into the existing session, continuing with different bytes obtained by a re-fetch, and substituting parsed/reserialized content for the original bytes. If the persisted raw bytes are lost or damaged, the current session has failed and is not recoverable; any re-discovery requires a **new** UpdateSession.

### Receipt/journal write ordering and crash lattice

The receipt and the session/journal are separate durable files and are never atomically committed together; safety comes from a fixed write ordering in which **the journal always leads and the receipt always follows verified state**. The orderings are frozen:

- **Handoff:** (1) durable session/journal with the staged milestone and handoff intent; (2) receipt → `Updating` with `activeSessionId`. The journal leads: a crash between (1) and (2) leaves an orphan prepared session, never an authority transfer without evidence.
- **Apply:** each replacement is journaled intent-before/completion-after; the receipt stays `Updating` throughout.
- **Probation-ready:** journaled in the session; the exclusive application lease is released only after this record is durable; the receipt stays `Updating`.
- **Commit:** (1) journal commit-intent with accepted health evidence; (2) receipt → `Installed` at the target version; (3) session finalized `committed` (journal published as committed evidence). The journal leads here too: a crash between (1) and (2) is recoverable intent, a crash after (2) is a committed installation whose bookkeeping merely lags.
- **Rollback:** (1) journal rollback intent; (2) restore/verify each old resource; (3) receipt → `Installed` at the previous version; (4) session finalized `rolled-back`.

Every crash gap therefore has exactly one legal reading; no combination may require guessing, and any combination that cannot be proven is `RecoveryRequired`:

| Crash between | Observed receipt | Observed journal/session | Legal recovery action |
| --- | --- | --- | --- |
| journal created, receipt still `Installed` | `Installed` | prepared/staged session, no authority transfer | Not an active update: normal launch is unaffected; the session may be resumed by its owner or discarded by bounded cleanup. No destructive action without the `Updating` receipt. |
| receipt → `Updating`, valid journal | `Updating` + activeSessionId | valid, any apply phase | Helper-only resume via the [frozen recovery entrypoint](application-lifecycle.md#9-durable-journals-and-recoverytransaction); continue or roll back per journal evidence. |
| receipt → `Updating`, journal missing/corrupt | `Updating` | unusable | `RecoveryRequired` — the runtime state cannot be proven; explicit repair/manual install. |
| commit intent journaled, receipt still `Updating` | `Updating` | commit intent + accepted health evidence | Verify the installed target set, finish receipt/integration reconciliation, publish receipt → `Installed`, finalize session. Do not re-run replacement blindly. |
| receipt → `Installed` (target), session unfinalized | `Installed` (target) | valid, pre-finalization | The receipt is authoritative: the installation is committed. Verify installed set + integration, finalize the session `committed`, release admission, bounded cleanup. Never roll back. |
| receipt → `Installed` (target), journal corrupt/unverifiable | `Installed` (target) | unusable | `RecoveryRequired` for bookkeeping only if runtime verification fails; if the target runtime verifies against the retained signed evidence, finalize with explicit disclosure of the lost journal. |
| rollback intent journaled, receipt still `Updating` | `Updating` | rollback intent, restoration possibly partial | Resume restoration per journal hashes to a verified old set, then receipt → `Installed` (previous version), finalize `rolled-back`. |
| old set restored, receipt still `Updating` | `Updating` | restoration complete | Verify old runtime, publish receipt → `Installed` (previous version), finalize `rolled-back`. |
| any state with conflicting or unreproducible evidence | — | — | `RecoveryRequired`; no guessed destructive rollback. |

UninstallSession shares envelope fields and logging, but has cleanup progress per owned identity and current-interaction data consent instead of update target/hash/probation. Consent records are audit metadata, not authority after restart. RecoverySession records the parent interrupted session and decision; it does not introduce a more permissive path policy.

**Phase 1 implementation decision (binding):** the implemented uninstall transaction realizes this contract without a full session envelope — the `Installing`/`Uninstalling` receipt plus a minimal active-uninstall journal are the durable state, the transaction is idempotent and resumable (an interrupted run is finished by the next helper invocation), and deletion authority always derives from a valid receipt plus fresh consent, never from the journal.

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

The nonce and processCreatedAt placeholders are descriptive, not valid fixture values. Helper retains the process handle returned when it creates the child, verifies it is alive and matches image, PID and creation time, and accepts only the marker for its current AwaitingHealthAck phase. Delete/reject previous markers before a new probation attempt, rotate nonce, and never use timestamp freshness alone. The protocol fixes the invariant, not the number: the timeout is finite, is measured monotonically from successful process creation, and no accepted ACK within it means the update is not healthy — rollback follows, never indefinite waiting or automatic commit. Sixty seconds is the implementation default, tunable by QA before protocol 1 ships; it is not a protocol-version invariant, and no session-controlled unbounded timeout exists. Network, weather, and any other optional content are irrelevant to health.

The child receives `DTW_MAINTENANCE_SESSION_ID`, `DTW_MAINTENANCE_NONCE`, and a fixed-purpose launch mode only in its own environment block. It validates current installation/session bindings, does not forward these variables, and writes health only after core initialization. These values do not authorize filesystem operations. During probation ordinary user mutations remain gated until the helper publishes committed or rollback state.

Probation admission is narrow by design: the child is recognized only by its environment bindings matching the `Updating` receipt's active session and exact target version; it may run sufficiently to initialize and write the HealthAck, and nothing else. Every conflicting maintenance action (uninstall, bootstrap, another update) is refused while probation is unresolved. When the updater is implemented, launch admission gains an explicit `PostUpdateProbation` classification for this context alongside Managed/Unmanaged/Development; until then no admission state stands in for it.

The marker proves an initialization checkpoint, not future reliability. If the child exits before acceptance, rollback. If it crashes after durable commit, ordinary crash recovery applies; Maintenance does not automatically undo arbitrary later user data changes. The complete frozen lease choreography — helper exclusive-lease release before probation, the child's shared lease, and the no-deadlock argument — is in [HealthAck and application data compatibility](application-lifecycle.md#10-healthack-and-application-data-compatibility).

## Trust store and key rotation

The `TrustedKeyStore` is compiled into the binaries and is read-only at runtime: a fixed set of entries, each `{keyId (SHA-256 of the raw public key), raw 32-byte Ed25519 public key, enabled (protocol 1 has no retired flag — an entry is either present and trusted or absent)}`. Remote content cannot add, replace, enable, or disable a key; a manifest that references an id absent from the store fails closed. Store size is bounded and duplicate ids are malformed. Both the main application and the helper embed the same store, and the helper verifies with its own copy — never with a store supplied by the caller or the session.

**Bootstrap trust.** v1.1.x has no updater and no trust root, so it can never authenticate a v1.2.0 package; and the key store inside a v1.2.0 package cannot prove the provenance of the package that delivered it. The first managed installation is therefore always the manual trusted bootstrap defined in [InstallTransaction](application-lifecycle.md#15-installtransaction-and-bootstrap): the user obtains the package from a trusted distribution path, may verify the published `.sha256` sidecar, and runs the install themselves. After that, every self-update requires a manifest signed by a compiled key. No circular bootstrap exists and none may be introduced.

**Bridge rotation.** Replacing a signing key is a two-release bridge: version N signs with key A and embeds a store containing A + B; from N+1 on, releases sign with B and may drop A once no supported client needs it. An old client accepts the bridge because A signed it; the new store arrives as verified payload content, not as remote instruction. Rollback across a rotation is bounded: a client that rolled back to a version trusting only A can no longer verify a B-signed manifest and simply stays on its version until it re-applies the bridge — rollback never grants an exception to signature rules. A manifest may not be double-signed under protocol 1 (one envelope, one key); multi-signature is a possible future extension requiring a new envelope schema. Key ids are content digests, so cross-key collision is a SHA-256 collision, treated as store corruption (fail closed), not as an alias.

**Bridge reachability.** Candidate selection must let a lagging client find the reachable next hop within the bounded candidate set. Frozen example: a client at vN trusting only K1 scans newest-to-oldest — vN+2 (signed by K2) is currently untrusted and skipped; vN+1, the bridge signed by K1 whose runtime compiles the K1+K2 store, is trusted and executable, so it is selected; after restarting under the bridge the client trusts K2 and later discovers and installs vN+2. This is normal candidate selection, never trust fallback, under these rules: every candidate must independently satisfy normal signature verification; a remote manifest can never extend the currently running binary's trust store; new trust roots enter the next generation's compiled store only through an executable release signed by an already-trusted key. A provider may hide the bridge — an availability failure, not a trust bypass — and can never forge one. Clients beyond the publisher's support window may be required to perform a manual trusted bootstrap. Publisher operational obligation: a bridge release must remain discoverable on the providers for as long as its support window is open.

**Compromise and loss.** If the private key is lost with no trusted alternative, delivery of updates stops; recovery is a manual trusted bootstrap (a new first install), not a protocol bypass. If a private key is compromised, attacker-crafted signatures verify as genuine, so no client-side rule can distinguish them: halt publication with the compromised key, inform users through independently authenticated channels, and have users reinstall manually from the new trust root. No online revocation, transparency log, or key-blocking infrastructure is claimed or planned for protocol 1.

## Compatibility and commit rules

Schema 1 does not imply protocol 1 support. Main app and currently running helper each advertise compiled supported protocols and independently reject unsupported targets. Protocol 1 manifests cannot remove old required resources or add new managed identities by suggestion; such changes require an explicitly supported policy/protocol evolution and bridge release.

Only stable versions are currently delivered; compare semantic components, not lexicographic strings. CI verifies all source versions and PE ProductVersion. Normalize the PE numeric trailing revision zero for the steady-state invariant; reject other unexpected version discrepancies. Future beta channels/version encoding need an explicit extension.

Update success means accepted HealthAck **and** durable target receipt **and** verified Windows integration **and** committed journal. Until then keep backups and admission closed. After committed state, failed ephemeral cleanup is a warning rather than runtime rollback. Uninstall success means requested known owned resources were cleaned and the receipt retired; unknown residuals and deferred temporary-runner removal are disclosed separately.

## Adversarial review record (Phase 2A)

Definitive answers frozen with this protocol; each relies only on rules stated above.

- **Q11 — New app migrates the DB, then crashes before HealthAck. Can the old runtime safely roll back?** Yes, and only because of the [persistent-data policy](application-lifecycle.md#10-healthack-and-application-data-compatibility): protocol 1 makes ordinary self-update ineligible unless every migration and persistent write executable before durable commit is backward-compatible with the previous runtime. Rollback restores runtime files and never touches the DB, WAL/SHM, settings, or assets; the old runtime then reads the post-migration data it was release-gated to support. A release whose migration is not backward-compatible is not deliverable by ordinary self-update at all — manual bootstrap is required.
- **Q12 — Power loss after the journal is durable but before the receipt becomes `Updating`.** The lattice's first row: receipt `Installed`, prepared/staged session. No authority was transferred, admission is unaffected, normal launch works, and the session may be resumed or discarded by bounded cleanup. No destructive action is legal without the `Updating` receipt.
- **Q13 — Power loss after the receipt becomes `Installed` at the target but before session finalization.** The receipt is authoritative: the update is committed. Recovery verifies the installed set and integration against the retained signed evidence, finalizes the session `committed`, releases admission, and cleans up. Rollback is not a legal reading of this state.
- **Q14 — Machine reboots halfway through apply. Which executable invokes recovery?** Exactly one frozen entrypoint: the main application's launch admission — before any DB/UI work — detects receipt `Updating` plus a valid journal, launches the canonical installed `desktop-todo-maintenance.exe` (compiled identity, `--recover --session-id <UUID>`) and exits. The helper is never absent mid-apply because replacement is an atomic same-volume move: at any instant it is the old or the new verified bytes, and a protocol-1 helper of either version can resume a protocol-1 journal. If even that entrypoint is gone, the manual route in [RecoveryTransaction](application-lifecycle.md#9-durable-journals-and-recoverytransaction) applies.
- **Q15 — An attacker swaps the session directory to another valid signed target after consent, before the helper starts.** The handoff's `--expected-manifest-sha256` binds the helper to the approved target: it re-verifies the signature over the persisted bytes, recomputes the digest, requires equality with the CLI value, and journals that digest before any destructive work. A different validly-signed target mismatches and is refused. The session stays zero-authority; the swap can only cause a refusal.
- **Q16 — The helper verifies the ZIP, then the extracted payload is modified before apply.** The helper alone controls extraction, and nothing is trusted between verification and use: every staged file is re-verified by fresh opens and hash checks at copy/replace time, and the ZIP itself is re-hashed. Modified bytes fail verification and the transaction refuses or rolls back; unverified content is never launched or installed.

### Final freeze review (Q1–Q15)

Definitive answers after absorbing the parallel audits; each relies only on rules frozen above.

- **Q1 — Signed evidence / receipt / PE version disagree: which wins?** None. They are mutually verifying security facts under the [installed version authority](#installed-version-authority); a disagreement is a repair/recovery conflict (RecoveryRequired when unprovable), never a selection input and never resolved by "picking a winner".
- **Q2 — A v1.2 client facing a K2-signed latest and a K1-signed bridge.** The bounded enumeration applies the complete eligibility predicate newest-to-oldest: the K2 latest is candidate-ineligible (unknown key, skipped), the K1 bridge is candidate-valid and eligible (signature, protocol, version, and v1.2→v1.4-style hop compatibility all pass), so it is accepted and frozen; after restarting under the bridge the client trusts K2 and later installs the latest. Normal candidate selection, not trust fallback; a garbage K2-signed latest published by a compromised account cannot block the bridge because candidate-invalid results continue the bounded scan.
- **Q3 — GitHub discovery succeeds but only offers an older version: does Auto ask Gitee for a newer one?** No. Auto switches to Gitee only when GitHub discovery is unavailable as a whole and no target is frozen; a valid-but-older GitHub result completes discovery. The availability tradeoff is a recorded design property.
- **Q4 — Raw manifest bytes lost after trusted-target-persisted: re-fetch and continue the session?** No. Resume is exact-byte; lost persisted bytes fail the session terminally, and re-discovery requires a new UpdateSession.
- **Q5 — A base64 decoder accepts non-canonical padding or whitespace.** The protocol rejects it: canonical standard-alphabet form, exact padding, no whitespace, 64 decoded bytes, and a mandatory canonical re-encode round-trip. Tolerant-decoder acceptance without round-trip equality is a verification failure.
- **Q6 — May v1.2 → v1.6 skip intermediate versions unconditionally?** No. The rollback-compatibility baseline is the actual installed source version (v1.2), not the target's previous release; the direct hop exists only if every pre-HealthAck durable write v1.6 can perform is proven v1.2-compatible. Otherwise the planner routes through a bridge release or manual bootstrap — it selects the highest reachable safe next hop, not unconditionally the highest version.
- **Q7 — v1.3 probation runs additive migration 4, then rolls back to v1.2: must v1.2 refuse to open because it sees migration version 4?** No. A schema upper-bound guard is deliberately not adopted in Protocol 1 — it would negate the backward compatibility the release gate proved. An older runtime may observe newer migration versions provided the actual resulting schema is compatible.
- **Q8 — v1.3 settings write a key v1.2 does not know; after rollback, may v1.2's load/persist delete it?** No. From the first self-maintaining release, evolvable settings documents must round-trip preserve unknown object members (including nested ones); missing fields may default, unknown fields are re-emitted. The current runtime implements this contract (commit `8ff0301`), and pre-HealthAck normalization may not delete data the source runtime considers valid.
- **Q9 — A release ZIP carrying THIRD_PARTY_NOTICES.md: still legal?** Yes. The package rule is exact equality with the compiled allowlist, currently nine entries including THIRD_PARTY_NOTICES.md; "nine" itself is not the security property.
- **Q10 — One extra arbitrary README-copy.txt in the ZIP?** Rejected — the entry set must equal the compiled allowlist exactly; any tenth entry rejects the entire package.
- **Q11 — Does self-update replace README/LICENSE/THIRD_PARTY_NOTICES?** No. The authenticity set (nine entries) is validated, but the runtime mutation set is exactly the two executables; installed support copies may stay old and are never part of replacement or rollback.
- **Q12 — Why is a stale temp whose name exactly matches the reserved namespace not an unknown-preserve object?** Because the namespace `<known-stem>.<canonical-uuid>.<tmp|installing>` is compiled-policy-owned: the stem comes from the closed compiled allowlist, the UUID is canonical, the suffix is exact, and the file is a regular non-reparse file under a canonical maintenance root. It is maintenance's own ephemeral resource, not an unknown file; the grammar is exact, never a wildcard glob.
- **Q13 — Receipt locally edited to a lower version while committed signed evidence and PE remain higher: can candidate selection accept a replay downgrade?** No. The evidence set is inconsistent → repair/recovery conflict, candidate selection does not run at all; and the downgrade refusal compares against the consistent anchor, which a receipt edit cannot lower.
- **Q14 — Main approved target A; the session contents are later swapped to a validly signed target B. Can the helper install B?** No. The handoff carries `--expected-manifest-sha256` (the approved digest); the helper re-verifies the persisted bytes, recomputes the digest, requires equality, and journals it before any destructive work. B mismatches and is refused; the session remains zero-authority.
- **Q15 — The bridge release disappeared from the provider candidate window; what happens to a K1-only client?** It can no longer find a reachable next hop: the bounded enumeration finds nothing eligible above its version, so it stays on its version — an availability failure, not a trust bypass. Recovery is a manual trusted bootstrap once the client is beyond the publisher's support window; within the window, the publisher's operational obligation is to keep the bridge discoverable.
