# desktop-todo-update-core

Pure Rust implementation of the signed-update protocol core for
Maintenance Protocol 1 (Phase 2B), shared verbatim by the product binary
(`alan-desktop`) and the offline maintenance helper
(`desktop-todo-maintenance`). Each binary embeds its own compiled trust
store and verifies independently at runtime; a verification claim from one
process is coordination, never evidence for the other.

The wire-level rules this crate implements are frozen in
[docs/maintenance-protocol-v1.md](../../docs/maintenance-protocol-v1.md)
(Phase 2A final freeze): canonical signed bytes, `SignedEnvelopeV1`, the
compiled read-only `TrustedKeyStore` with SHA-256 key ids, canonical Base64,
closed-struct parsing, manifest semantic validation, and the bounded
candidate-eligibility planner with the candidate-ineligible /
candidate-invalid distinction.

Deliberate boundaries — enforced by this crate's dependency list, not by
convention: no network, no filesystem, no Tauri, no SQLite, no Win32, no
async runtime. Dependencies are `serde`/`serde_json`, `sha2`,
`ed25519-dalek` (minimal features; signing is used only by offline test
fixtures), and `base64`. The canonical version comparator is the Phase 1.5
`Version` type, which moved here and is re-exported by the maintenance
crate — there is no second parser.

The production trust store (`src/compiled.rs::production_trust_store`) is
intentionally empty until the publisher provisions real v1.2 public keys at
the release-signing provisioning gate. Test keys exist only under `tests/`
and must never be compiled into a production store or used to sign a
release.
