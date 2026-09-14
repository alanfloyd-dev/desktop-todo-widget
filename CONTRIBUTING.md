# Contributing

**English** | [简体中文](CONTRIBUTING_ZH.md)

Thanks for helping improve desktop-todo-widget. The project is Windows-first and intentionally keeps its product layer small.

This is a small, actively maintained project. Bug reports, code review, Windows platform expertise, and cleaner implementations of areas that could be better are all genuinely useful.

## Reporting bugs

Settings → Developer → **Copy diagnostics** produces an issue-ready text report. Review it before posting. The report is built from an explicit allowlist and excludes tasks, profile values, weather location, Quick Link URLs, asset filenames/paths, wallpaper paths, and precise database paths.

## Before opening a change

- Keep the pull request focused on one change. Do not bundle unrelated features into documentation, appearance, or platform work.
- Explain any platform-specific behavior: the Win32/WebView2 assumption behind it, and what happens when it does not hold.
- Add tests where practical. Native mode changes also require manual Floating → Sidebar → Floating and Floating → Desktop → Floating checks on Windows; Desktop changes should repeat the interaction and Win+D gates documented in [docs/desktop-mode.md](docs/desktop-mode.md).
- Standard mode must not regress while you are changing Enhanced. Standard is the v1 default and the accessibility path.
- Keep product logic out of the Win32 adapter, and do not add telemetry, accounts, or a remote service without an explicit project decision.

## Data and migrations

- Preserve user data. Migrations must be idempotent and compatible with an existing user database.
- Add migrations in ascending order and record the version in the same transaction as the schema mutation.
- Destructive migrations are not acceptable, and neither is renaming the app-data directory, database filename, or tray id — those keep their internal `alan-desktop` spelling so existing installs keep their data.
- Do not place user databases or copied diagnostic output in the repository.

## Code and comments

- Comment non-obvious constraints, not routine syntax.
- In Win32 code, explain invariants, call ordering, undocumented Shell assumptions, and failure modes.
- Every new `unsafe` block needs a nearby `SAFETY:` comment describing why pointers, handles, buffers, callbacks, or lifetimes are valid.
- Preserve the same outer Tauri HWND and WebView2 controller across Desktop transitions.
- Do not replace conditional lifecycle recovery with polling, a global Win+D shortcut, or always-on-top.

## Windows internals and large orchestration modules

Some Windows-specific modules, especially window-mode and product-window orchestration, are currently larger and more coupled than the project ultimately aims for. In particular, `src-tauri/src/window_mode.rs` and `src-tauri/src/product_window.rs` are known to be large.

This is known technical debt, not something that has gone unnoticed. These areas contain behavior shaped by extensive testing around desktop attachment, WebView2 hosting, DPI, input routing, window styles, Win+D behavior, taskbar/Alt+Tab semantics, and recovery paths. Several of the constraints are undocumented platform behavior, so the current structure often reflects what was verified to work rather than what would be cleanest to read.

Large structural refactors in these files should not be submitted casually. Prefer small, behavior-preserving changes with focused tests and manual verification. Refactors that alter module boundaries should be discussed in an issue first, and every refactor must come with focused tests plus the manual mode-transition and Desktop interaction checks described in [Before opening a change](#before-opening-a-change) and [docs/desktop-mode.md](docs/desktop-mode.md).

## Development checks

```powershell
pnpm install
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

## AI-assisted contributions

AI-assisted pull requests are welcome, but you must review, test, and understand the changes you submit. Please say so in the description when a change was AI-assisted, and do not open a PR you cannot explain or debug.

## Licensing and provenance

Contributions are accepted under the repository [MIT License](LICENSE). Do not copy code from repositories without a compatible license. When adapting a substantial external implementation, record the source, license, and what was changed. Documentation and API behavior references should be linked where they materially influenced a platform workaround.
