# Contributing

Thanks for helping improve Alan Desktop. The project is currently Windows-first and intentionally keeps its product layer small.

## Before opening a change

- Keep work inside the requested phase; do not bundle future Todo, weather, insights, updater, or sync features into foundation changes.
- Prefer focused Vue/TypeScript presentation code and small Rust capability/repository boundaries.
- Do not move product logic into the Win32 adapter.
- Do not add telemetry, accounts, or a remote service without an explicit project decision.

For bugs, open the Developer section in Settings and use **Copy diagnostics**. Review the text before posting it. The generated report is intentionally allowlisted and excludes personal content and paths.

## Development checks

```powershell
pnpm install
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Native mode changes also require manual Floating → Sidebar → Floating and Floating → Desktop → Floating checks on Windows. Desktop changes should repeat the interaction and Win+D gates documented in `docs/desktop-mode.md`.

## Code and comments

- Comment non-obvious constraints, not routine syntax.
- In Win32 code, explain invariants, call ordering, undocumented Shell assumptions, and failure modes.
- Every new unsafe block needs a nearby `SAFETY:` comment describing why pointers, handles, buffers, callbacks, or lifetimes are valid.
- Preserve the same outer Tauri HWND and WebView2 controller across Desktop transitions.
- Do not replace conditional lifecycle recovery with polling, a global Win+D shortcut, or always-on-top.

## Database changes

Add migrations in ascending order and record the version in the same transaction as the schema mutation. Migrations must be idempotent and compatible with an existing user database. Do not place user databases or copied diagnostic output in the repository.

## Licensing and provenance

Contributions are accepted under the repository MIT License. Do not copy code from repositories without a compatible license. When adapting a substantial external implementation, record the source, license, and what was changed. Documentation and API behavior references should be linked where they materially influenced a platform workaround.
