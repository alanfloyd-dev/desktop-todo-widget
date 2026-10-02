# Release signing operator workflow (Phase 3A)

Status: **operator runbook for the v1.2 signed-update release chain**. It documents the offline key ceremony, trust-root provisioning, and exact-byte manifest signing implemented in Phase 3A. The frozen wire rules live in [Maintenance protocol v1](maintenance-protocol-v1.md) ("Canonical signed bytes", "Trust store and key rotation"); this page adds no protocol rules. All examples use obvious placeholders — never real key material.

## Scope

Implemented tooling:

- The compiled production trust root: `src-tauri/update-core/src/production_keys.rs` (the single source of truth for `production_trust_store()`, compiled into **both** the product binary and the maintenance helper).
- The offline release signer: `src-tauri/release-signer` (`desktop-todo-release-signer`). Offline by construction: no HTTP, no provider API, no Tauri, no SQLite, no async runtime.
- Manifest pre-sign validation, exact-byte Ed25519 signing, and a mandatory production-equivalent self-verification round-trip (`update_core::verify_and_parse`) before any envelope is written.
- Package/EXE fact computation (SHA-256 + byte size) for authoring the manifest.

Implemented in commit `712a7bb` as separate operator tooling (`desktop-todo-release-pipeline`; never a runtime component): release package assembly, release facts, deterministic manifest generation, provider publishing orchestration, remote read-back verification, and mirror consistency. That pipeline delegates cryptographic signing to this signer and is never a trust authority; it has been exercised only against local mock/integration provider tests and a local rehearsal prepare. Still not performed: real GitHub/Gitee provider smoke (including Gitee multipart online compatibility), the real production key ceremony, the first signed RC, and any production publish. Deliberately out of scope, never planned: CI-held signing keys.

## Ground rules

1. **Private keys never touch the repository, CI, logs, or this tool's output.** The signer reads the seed from a local file, keeps it in memory for the operation, and prints nothing derived from it except the *public* key id.
2. The production trust root is **public** material: raw 32-byte Ed25519 public keys and their key ids only.
3. The signature always covers the **exact raw bytes** of `update-manifest.json` as served. The signer never reserializes, pretty-prints, normalizes line endings, or canonicalizes the signed document.
4. Until a real production public key is provisioned, the compiled trust store is **empty and every update fails closed**. Provisioning it is a reviewed release gate.
5. **Signing successfully requires the signing key to already be present in the compiled production trust store.** The signer's own key is never a trust authority: `sign` checks the derived key id for exact membership in the compiled store *before* signing, then re-verifies the produced envelope through that same compiled store. An unprovisioned key — including any valid Ed25519 key and the empty store itself — fails closed (exit code 20) before any output exists. No valid-but-untrusted Ed25519 private key can ever produce a publishable release.

## 1. Key ceremony (offline, outside the repo)

Ordering matters: the keypair is generated **outside the repository**, its PUBLIC key is provisioned into the compiled trust root (step 2) and committed, and only then can `sign` succeed — the production signer refuses any key that is not already a provisioned compiled trust root.

Run on a trusted offline machine. The tool can generate the pair, or you may bring your own Ed25519 seed (see key-file format below):

```text
desktop-todo-release-signer generate-keypair --seed-out <32-byte-private-seed-path> --public-out <32-byte-public-key-path>
```

- `--seed-out` receives the raw 32-byte private seed. Store it offline (encrypted media, dual control as your operational policy dictates). **Never commit it, never copy it into the repository, CI, or release artifacts.**
- `--public-out` receives the raw 32-byte public key. This file (or its bytes) is the only material that enters the repository.
- The command prints the **key id**: the lowercase SHA-256 hex of the raw public key — the frozen derivation (`trust.rs::derive_key_id`).

Key-file format is strict: exactly 32 raw bytes, regular file, no symlink/reparse indirection, no other encoding (no PEM/hex/base64). The signer refuses anything else.

You can re-derive the key id at any time:

```text
desktop-todo-release-signer key-id --seed <32-byte-private-seed-path>
desktop-todo-release-signer key-id --public <32-byte-public-key-path>
```

## 2. Provision the public key into the compiled trust root

Print the exact provisioning table entry from the public key:

```text
desktop-todo-release-signer provision-entry --public <32-byte-public-key-path>
```

Paste the printed `ProvisionedKey { .. }` entry into `PRODUCTION_KEYS` in `src-tauri/update-core/src/production_keys.rs`, review the diff, and commit it. Provisioning is deterministic and machine-checked:

- The consistency tests pin every entry: the declared key id must equal the derived SHA-256 of the raw bytes, and duplicate ids fail.
- `store_from` re-validates at construction: a declared id that contradicts its own bytes, a duplicate id, or invalid material **fails closed** — the store is never built from an unvalidated entry.
- Both binaries compile the same table from the same source file; the main/helper parity tests (`main_trust_store_is_the_shared_compiled_production_store`, `helper_trust_store_is_the_shared_compiled_production_store`) pin that neither side carries a local override.

## 3. Build release binaries

Exactly the frozen build contract (see [application-lifecycle.md §16](application-lifecycle.md#16-release--ci-requirements)):

```text
pnpm tauri build --no-bundle
cargo build --release -p desktop-todo-maintenance --manifest-path src-tauri/Cargo.toml
```

Build the signer through the canonical parent manifest (never a nested manifest — that would fork `Cargo.lock`):

```text
cargo build --release -p desktop-todo-release-signer --manifest-path src-tauri/Cargo.toml
```

## 4. Assemble the package and compute facts

Assemble the flat nine-entry release ZIP (the exact compiled package allowlist — see protocol doc). Then compute the facts the manifest must carry, from the **final** package bytes and the **final** EXE bytes:

```text
desktop-todo-release-signer package-facts --package <final-zip-path> --main-exe <desktop-todo-widget.exe> --helper-exe <desktop-todo-maintenance.exe>
```

Output: package SHA-256 + size, and per-identity (mainExecutable / maintenanceHelper) SHA-256 + size. The two EXE inputs must be given together — the manifest requires exactly the two fixed identities.

## 5. Author the manifest

Write `update-manifest.json` with the frozen schema-1 field set exactly (`docs/maintenance-protocol-v1.md`, "UpdateManifest"): `schemaVersion`, `appId`, `channel`, `version`, `publishedAt`, `notes`, `updaterProtocol`, `assets` (per platform: `filename`, `size`, `sha256`, `installFiles`). Fill in the facts from step 4. The signer does **not** correct manifests; it validates and refuses:

- wrong `updaterProtocol` / `schemaVersion` / app id / channel / version grammar → refused
- unknown fields anywhere except the open platform map → refused
- hash/size bounds and identity/filename agreement → refused on mismatch

Decide the exact final bytes now (indentation, trailing newline, ordering): whatever bytes you sign are the bytes that must be served, and the helper re-verifies those persisted bytes at install time.

## 6. Sign (offline, exact bytes)

```text
desktop-todo-release-signer sign --manifest <update-manifest.json> --key <32-byte-private-seed-path> [--out <update-manifest.json.sig>] [--overwrite]
```

Behavior (every gate must pass; the first failure exits without producing output):

1. Validates the manifest (frozen encoding/parse/semantic rules) — an invalid manifest fails **before** signing.
2. **Trust gate:** derives the key id from the supplied signing key and requires it to be an exact member of the compiled `production_trust_store()` — before any signing. An unprovisioned key fails with exit code 20; nothing is signed and no output exists.
3. Signs the exact raw file bytes with Ed25519 (deterministic RFC 8032; same input → same signature).
4. Builds the frozen envelope: `{"schemaVersion":1,"algorithm":"Ed25519","keyId":"<64 lowercase hex>","signature":"<canonical padded RFC 4648 base64 of exactly 64 bytes>"}` — no whitespace, no trailing newline.
5. Runs the mandatory self-verification: the produced envelope + the same manifest bytes go through the production-equivalent verifier (`update_core::verify_and_parse`) **against the same compiled production trust store**, and the frozen manifest digest is cross-checked.
6. Publishes the envelope atomically (sibling temp file → rename). An existing output is never silently overwritten; pass `--overwrite` explicitly.

Exit codes are typed: 2 usage, 10 manifest read, 11 manifest invalid, 12 key read, 13 key format, 14 signing, 15 output write, 16 self-verification, 17 output collision, 18 verification, 19 facts read, 20 signing key not provisioned in the compiled production trust store.

## 7. Independent self-check

Re-verify the artifact set exactly as a client will:

```text
desktop-todo-release-signer verify --manifest <update-manifest.json> --envelope <update-manifest.json.sig> --public <32-byte-public-key-path>
```

(The `--seed` form derives the same public half.) This runs the identical verifier the product and helper use, and the report includes `keyIdProvisioned` — whether the key that verified the envelope is the actual compiled production trust root. Only `true` pairs are publishable. Then confirm the printed `manifestSha256` is the value you will bind into the handoff/consent chain, and that version/package facts match your release.

## 8. Publish

Upload **identical** `update-manifest.json`, `update-manifest.json.sig`, the ZIP, and the ZIP `.sha256` sidecar to every provider — never rebuild or re-sign per source (protocol doc, "Compatibility and commit rules"). Publishing orchestration for GitHub and Gitee is implemented (commit `712a7bb`, `desktop-todo-release-pipeline`) with an explicit collision/idempotence policy, remote read-back verification, and mirror consistency by artifact digests — but no real provider publish has been executed: real GitHub/Gitee smoke and Gitee multipart online compatibility remain unverified, no production key ceremony has happened, no first signed RC exists, and production publishing stays subject to the §16 release-requirements gates (notices completeness, cargo audit/deny, real remote read-back/mirror consistency). The signing steps above are unchanged: the private seed never enters the publishing layer, and ordering is still key ceremony → public-key provisioning → build → sign → publish.

## What must NEVER be committed or logged

- The private seed (any encoding), any file derived from it except the public key.
- Seed material in command history (prefer a file path passed to the tool; shell history may record the *path*, never the bytes).
- Test fixture seeds (`dtw-test-KEY*`, `dtw-signer-TEST-ONLY-*`) in `PRODUCTION_KEYS` — they are TEST ONLY / NON-PRODUCTION and are kept strictly under `#[cfg(test)]` / test fixtures.
- Any "fallback" or placeholder trust root in production.

## Key rotation

Protocol 1 rotation is the two-release bridge described in the frozen protocol doc ("Trust store and key rotation"): the bridge release signs with the old key and compiles a store carrying old + new. Operationally: provision the new public key **alongside** the old one in `PRODUCTION_KEYS`, publish the bridge, then sign subsequent releases with the new key and drop the old entry once no supported client needs it. No remote rotation, revocation server, or manifest-carried trust exists in protocol 1.
