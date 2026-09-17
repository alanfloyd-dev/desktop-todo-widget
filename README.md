**English** | [简体中文](README_ZH.md)

# desktop-todo-widget

A local-first Windows todo widget built around three window modes: **Sidebar**, **Floating**, and **Desktop**.

It focuses on lightweight task management, tray-first interaction, customizable appearance, Quick Links, weather, and native Windows integration.

`Rust` · `Tauri` · `Vue 3` · `TypeScript` · `WebView2` · `SQLite`

## Screenshots

<p align="center">
  <a href="docs/assets/demo.gif"><img src="docs/assets/demo.gif" width="420" alt="desktop-todo-widget demo"></a>
</p>

[Watch the MP4 recording](docs/assets/demo.mp4)

<table>
  <tr>
    <th align="center">Orb</th>
    <th align="center">Sidebar</th>
    <th align="center">Desktop</th>
  </tr>
  <tr>
    <td align="center"><a href="docs/assets/orb.png"><img src="docs/assets/orb.png" width="72" alt="Floating Orb"></a></td>
    <td align="center"><a href="docs/assets/sidebar.png"><img src="docs/assets/sidebar.png" width="190" alt="Sidebar mode"></a></td>
    <td align="center"><a href="docs/assets/desktop.png"><img src="docs/assets/desktop.png" width="220" alt="Desktop mode"></a></td>
  </tr>
</table>

## Download

Releases are published on [GitHub Releases](https://github.com/alanfloyd-dev/desktop-todo-widget/releases).

**Current release: v1.1.0** — `desktop-todo-widget-v1.1.0-windows-x64.zip`. Extract it and run:

`desktop-todo-widget.exe`

The v1.1.0 archive also includes `install.ps1` and `uninstall.ps1` for optional per-user installation and removal (see [Install](#install)).

The portable workflow remains supported: extracting the ZIP and running the executable requires no installation.

**Previous release: v1.0.1** — `desktop-todo-widget-v1.0.1-windows-x64.zip`.

See [RELEASE_NOTES.md](RELEASE_NOTES.md) for the v1.1.0 changes.

### Windows compatibility

- Windows 11 is currently validated. Windows 10 compatibility is not yet formally validated.
- WebView2 Runtime is required.

## Install

`install.ps1` and `uninstall.ps1` ship beside the executable in the release payload (v1.0.1 and later) and install or remove a per-user copy of the app. They are plain PowerShell scripts (Windows PowerShell 5.1 or later) and need no administrator rights.

```powershell
# from the extracted release payload, with install.ps1 beside the executable
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

`-ExecutionPolicy Bypass` applies to that one invocation only. There is no reason to change the machine or user execution policy to run these scripts.

What `install.ps1` does:

- copies the executable into `%LOCALAPPDATA%\Programs\desktop-todo-widget\`,
- installs the executable as `desktop-todo-widget.exe` whatever the build output's internal name was,
- creates a per-user Start Menu shortcut unless `-NoStartMenuShortcut` is given,
- supports re-running as an in-place upgrade: it stops a running instance started from that directory, replaces program files, and prunes payload files the new build no longer ships.

What it deliberately does not do: write to Program Files or any machine-wide location, touch the registry or `PATH`, or delete user data. The payload directory it is pointed at must contain the executable plus the runtime files that ship beside it; an incomplete payload fails the install instead of producing a broken one.

User data stays in `%APPDATA%\net.alanfloyd.desktop\` — database, settings, appearance profiles, managed images, and the weather cache — and survives both install and uninstall.

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1                  # keeps your data
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -RemoveUserData  # warns, then removes the data too
```

`uninstall.ps1` stops the app if it is running, removes the Start Menu shortcut and the program files, and keeps `%APPDATA%\net.alanfloyd.desktop\` unless `-RemoveUserData` is passed. That flag prints the exact paths before deleting and asks for confirmation; `-Force` skips the prompt for scripted use, and a host that cannot prompt keeps the data. Deletion is bounded: every file must be inside the resolved install directory and must look like a program file (executable, documentation, log files, or runtime payload files an older version may have installed), and anything unexpected — a foreign file or a subdirectory — stops the uninstall instead of being deleted.

Both scripts are verified by a simulation harness that runs them against a throwaway sandbox with a fake `%LOCALAPPDATA%`, `%APPDATA%`, payload, and user-data directory: clean install, overwrite upgrade, both uninstall modes, and the refusal paths.

```powershell
pwsh -File scripts/verify-install-scripts.ps1
```

The portable ZIP remains the primary distribution; nothing about extracting and running the executable changed.

## Features

**Todo lifecycle** — add, edit, complete, reopen, cancel, carry, delete, and reorder. Carry is history-preserving: the original row is marked `carried` and a linked successor is created for the next task day in one SQLite transaction.

**Window modes** — Sidebar, Floating, and Desktop share one window and one WebView; Floating collapses to a 56 DIP avatar Orb. See [Window modes](#window-modes).

**Visible Settings entry** — the expanded footer carries a Settings gear next to the profile identity in Floating expanded, Sidebar, and Desktop (never in the Orb). It opens the same Settings surface as the right-click menu entry and the tray, so Settings is discoverable without knowing about the context menu.

**Appearance profiles** — each window mode keeps its own profile: Glass, Solid, two-stop Gradient, a managed local image, or the current Windows wallpaper, plus tint, opacity, blur, overlay, image fit/position, and an optional custom text colour.

**Quick Links** — add, edit, reorder, and delete your own links; only `http://` and `https://` URLs are accepted.

**Weather** — optional current conditions and today's high/low with a locally cached snapshot and a clear "not configured" state.

**Review** — read-only Daily, Weekly, and Monthly Review aggregated from local task history. No scores, trends, or advice.

**Language** — English, Simplified Chinese, or System (follows the Windows display language).

**Windows integration** — tray-resident, no taskbar button, no ordinary Alt+Tab entry, right-click context menu on the widget and the Orb, and per-mode window geometry that survives restarts. Verified at 150% display scaling.

## Window modes

**Sidebar** — edge-oriented widget mode. Occupies the monitor work-area height, docks to the left or right edge, and remembers side and width. Dragging Floating near an edge enters Sidebar.

**Floating** — a movable, frameless window and the first-run default. A profile that has never been used opens it **expanded**, so a first run shows the widget itself — date line, tasks, footer, and the Settings gear — instead of a 56 DIP Orb that is easy to miss in a screen corner. It can collapse to the Orb and expand again, and it keeps its expanded size and Orb anchor as independent saved values.

**Desktop** — a desktop-hosted frameless widget. It is reparented as a child of the Windows desktop host, so the wallpaper, desktop icons, and the native desktop context menu stay usable around it. It is movable and resizable when unlocked, with geometry independent from Floating, and always-on-top is unavailable.

### First run

The first run is the only time the product chooses a presentation: a profile with no stored settings document yet is created with Floating expanded, the documented default size, and the **Gradient** material. Gradient is the documented graphite palette (`#11191e` → `#213747` at 135°) at the documented opacity, and it is the fresh default because a translucent tint over an arbitrary wallpaper competes with the widget's own text, while the Gradient keeps a defined surface that reads the same over dark, light, saturated, and textured desktops. An existing profile is never re-defaulted — it is loaded exactly as stored, including its Floating presentation and its material — so upgrading cannot change the state a user left the widget in. The visible Settings gear is present from that first expanded frame.

### Desktop material fallback

Desktop uses its documented translucent Graphite material instead of Glass. This is a design and platform constraint, not a regression:

- Desktop is hosted as a child window under the Windows desktop hierarchy (`SHELLDLL_DefView`), which is not a documented public embedding API.
- Window backdrop effects need top-level HWND semantics; a `SHELLDLL_DefView` child is not one.
- Desktop therefore always uses the documented translucent Graphite fallback.

See [docs/desktop-mode.md](docs/desktop-mode.md) for the attach/detach lifecycle and host discovery details.

## Rendering

The product has a single windowed WebView2 backend (`ICoreWebView2Controller`): there is one hosting path, no user-selectable backend, and full Windows UI Automation exposure. Window materials are resolved by the Tauri window-effects request plus CSS material layers; see [Appearance profiles](#appearance-profiles).

## Appearance profiles

Appearance is per window mode: editing Sidebar's material cannot change Floating's. Each profile stores the background type, tint, opacity, blur, and overlay values, image fit and position, the text contrast mode, and the custom text colour.

- Background types: Glass, Solid, Gradient, managed local image, current Windows wallpaper.
- Text contrast: Auto, Light, Dark, or Custom. Auto samples solid colours and gradient stops directly, and samples image/wallpaper data once at 32×32.
- Missing, corrupt, or oversized images resolve to the safe Glass fallback instead of breaking the surface.
- Supported image files are PNG, JPEG, and WebP. A picked background is kept at full size; a picked **profile avatar** is normalized first — decoded, oriented, centre-cropped to a square, scaled to 256×256, and stored as a PNG that keeps transparency — so a multi-megabyte photo never ends up in the profile. A file too large to read (over 24 MB), or one the decoder cannot handle, is refused with a message next to the button that opened the picker.

See [docs/appearance.md](docs/appearance.md).

## Quick Links

Quick Links are user-managed name/URL pairs shown as a product section: add, edit, reorder, delete. Names and URLs are your own data and are never translated. A link is validated as `http`/`https` with a host when it is saved and again immediately before it is opened. An empty list simply hides the section.

## Weather and Review

Weather is optional ambient information rather than a startup dependency. Location search runs only after an explicit action, you must pick a result yourself, and forecasts then use the saved coordinates and timezone. Cached snapshot freshness is graded, and a failed refresh keeps the last valid cache. Weather data by [Open-Meteo.com](https://open-meteo.com/) under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); attribution is shown in Settings and in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Review is a read-only projection over raw task history: Daily (one task day), Weekly (Monday–Sunday), or Monthly (calendar month). It reports Planned, Completed, Carried, Cancelled, and Pending counts plus task-day and category distributions, and it stores no report snapshots.

See [docs/weather.md](docs/weather.md) and [docs/reviews.md](docs/reviews.md).

## Data and privacy

- **Local-first.** Tasks, settings, appearance profiles, Quick Links, profile identity, and the weather cache live on this machine.
- **No account.** There is no sign-in, hosted identity, or sync server.
- **No intentional upload.** Task, profile, appearance, and Quick Link data are not sent anywhere by the application.
- **Weather is the one remote call.** Open-Meteo receives the geocoding and forecast request needed to answer a location you configured. With no configured location, no weather request is made.
- **Diagnostics are opt-in.** Settings → Developer → Copy diagnostics produces an allowlisted, privacy-minimized report with runtime/window metadata and non-sensitive appearance state. It excludes task content, profile values, weather location, asset filenames/paths, and exact database paths.
- **Uninstalling keeps your data.** `uninstall.ps1` removes the program files only; `%APPDATA%\net.alanfloyd.desktop\` is deleted solely with the explicit `-RemoveUserData` flag, after a printed warning and confirmation.

### Internal compatibility identifiers

Some shipping identifiers keep their pre-release `alan-desktop` spelling on purpose, because renaming them would strand existing user data or break upgrades. They are internal names, not the product name:

- `alan-desktop` — the Rust crate name, the private `package.json` name, and therefore the built executable `alan-desktop.exe`. `install.ps1` installs it as `desktop-todo-widget.exe`, which is only a file name: nothing in the product reads its own executable name.
- `alan-desktop.sqlite3` — the local database file, under the app-data directory `net.alanfloyd.desktop`.
- `alan-desktop-tray` — the tray icon id.

The public product name is `desktop-todo-widget`, which is what the tray tooltip, the tray/context menu heading, and the window title use. See [docs/data-model.md](docs/data-model.md).

## Development note

This project is built with significant AI assistance.

I am a geology student rather than a computer science student, and my main technical direction is Python and data analysis for scientific work. Windows internals, Rust, Tauri, and WebView2 are not my primary stack, so I do not claim deep expertise in every implementation detail.

That said, this is not a one-shot AI-generated repository. I remain involved in product design, architecture decisions, testing, debugging, manual QA, and release review. The codebase includes comments, tests, and technical documentation intended to make the implementation easier to inspect and maintain.

The project is actively maintained, and I expect to keep improving it as I learn more.

Contributions are very welcome — especially bug reports, code review, Windows platform expertise, and cleaner implementations of areas that could be improved.

AI-assisted contributions are also welcome, but please review, test, and understand the changes you submit.

## Architecture

High level: Vue 3 + TypeScript render the product surface and own presentation state; Rust owns OS paths, persistence, the window/tray lifecycle, and the Win32 boundary; the Win32/WebView2 layer stays behind a narrow adapter. The UI never manipulates HWNDs or SQLite directly.

- **Tauri** — application shell, window/tray lifecycle, IPC commands and events.
- **Rust** — product settings, SQLite repository and migrations, task lifecycle, reviews, weather adapter, appearance validation.
- **Vue 3 + TypeScript** — presentation, per-mode layout, settings, review, weather display.
- **WebView2** — windowed rendering surface.
- **SQLite** — local task/history/category/weather storage; typed JSON settings in `app_settings`.

See [ARCHITECTURE.md](ARCHITECTURE.md) and [docs/](docs/) for the module-by-module detail.

### Windows-specific maintenance note

Some Windows-specific window hosting and mode-management code is intentionally conservative and currently more centralized than ideal. In particular, `src-tauri/src/window_mode.rs` and `src-tauri/src/product_window.rs` carry most of it.

This is known technical debt rather than something unexamined: those modules contain behavior shaped by real bug fixing and QA around platform compatibility, lifecycle, input, DPI, desktop hosting, window styles, Win+D behavior, taskbar/Alt+Tab semantics, and recovery paths.

Refactoring is planned after v1 stabilization, but behavioral stability takes priority over structural cleanup. Changes there are best kept small and behavior-preserving, and their constraints are documented in [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/desktop-mode.md](docs/desktop-mode.md).

## Building from source

Requirements: Windows 11, Node.js 20+, pnpm, Rust stable with the MSVC target, the Windows SDK, and the WebView2 Runtime.

```powershell
pnpm install
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle
```

### Verification

Four scripts cover the parts of this project that unit tests cannot reach. All of them stop any running instance first, and none of them touches a profile without a verified copy:
```powershell
pwsh -File scripts/verify-no-console-window.ps1   # release has no console window; debug still does
pwsh -File scripts/verify-first-use-ux.ps1        # fresh-profile first run, material, Settings gear, restart, quit
pwsh -File scripts/verify-avatar-flow.ps1         # avatar picker matrix, normalization, persistence, errors
pwsh -File scripts/verify-install-scripts.ps1     # install / upgrade / uninstall in a throwaway sandbox
```

- `verify-no-console-window.ps1` checks the PE subsystem of both builds and performs a shell launch (`explorer.exe`, the double-click path), using the debug build as a positive control so a passing run proves the detector can see console windows at all.
- `verify-first-use-ux.ps1` drives the built application over its WebView2 DevTools endpoint. The product resolves its data directory through the Windows known folder, so a fresh-profile run cannot be sandboxed through the environment: the wrapper copies the real `%APPDATA%\net.alanfloyd.desktop` aside, verifies the copy by size and SHA-256 before removing anything, and restores and re-verifies it afterwards.
- `verify-avatar-flow.ps1` runs the same way and additionally drives the real native file picker through `scripts/avatar-picker-drive.ps1` (UI Automation) with generated synthetic images, then checks what was stored, rendered, and reported for each one.
- `verify-install-scripts.ps1` runs `install.ps1` and `uninstall.ps1` against a fake `%LOCALAPPDATA%`, `%APPDATA%`, payload, and user-data directory, including the refusal paths.

`pnpm tauri:dev` runs the Vite dev server for frontend work. Production builds embed the compiled frontend and serve it through the Tauri custom protocol (`custom-protocol` feature), so a release build never depends on a dev server.

Release builds are linked as a Windows GUI-subsystem application (`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` in `src-tauri/src/main.rs`), so double-clicking the built executable does not open a console window. Debug builds keep the console, so `eprintln!` diagnostics stay visible during development. In every profile the app also writes its diagnostics to `qa-diagnostics.log` beside the executable, and panic reports are appended to that same file, so a release crash is still recorded without a console.

`pnpm tauri build --no-bundle` produces the executable, and `bundle.active` is `false`: the Tauri bundler is disabled, and installation is the per-user script described in [Install](#install) rather than a generated MSI/EXE package.

## Known limitations

- **Desktop material** — Desktop always uses the translucent Graphite fallback. It is hosted as a child of the Windows desktop hierarchy while window backdrop effects need top-level HWND semantics.
- **Windows-specific implementation** — the product depends on Windows/WebView2/Tauri-specific behavior and may need compatibility updates as those platforms evolve.
- **Desktop host is undocumented** — `Progman`, `WorkerW`, and `SHELLDLL_DefView` topology can change across Windows updates and Explorer restarts.
- **Scope** — Windows 11 validated, no updater and no bundled MSI/EXE package (installation is the per-user script in [Install](#install)), no sync, and no localization beyond English and Simplified Chinese.

## Roadmap

Planned after v1 stabilization:

- Reports: broader factual review surfaces on the existing task history.
- Componentization: smaller, clearer frontend and Rust boundaries.
- Settings organization: group the current settings surface more deliberately.
- Packaging and update delivery: signing, a packaged installer, and an update mechanism on top of the existing per-user install/uninstall scripts.

Deferred work is tracked in [FUTURE.md](FUTURE.md). Apart from the install and uninstall scripts described in [Install](#install), nothing in this section is implemented yet.

## Contributing

Contributions are welcome. Short version:

- Keep pull requests focused on one change.
- Explain platform-specific behavior and the Windows/WebView2 assumptions behind it.
- Add tests where practical; native mode changes still need the manual mode-transition checks.
- Preserve user data. Migrations must be idempotent, and destructive migrations are not acceptable.
- AI-assisted pull requests are welcome, but you must review, test, and understand what you submit.

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE). Copyright (c) 2026 Alan Floyd.

Dependency licenses and provenance notes are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
