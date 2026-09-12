# Third-party licenses and implementation provenance

desktop-todo-widget's own source is licensed under MIT. Dependencies remain under their respective licenses; the project MIT license does not relicense them.

## Direct Rust dependencies

| Dependency | Declared license |
| --- | --- |
| Tauri / tauri-build | Apache-2.0 OR MIT |
| wry (vendored Windows composition patch, `vendor/wry`) | Apache-2.0 OR MIT |
| serde / serde_json | MIT OR Apache-2.0 |
| rusqlite | MIT |
| reqwest | MIT OR Apache-2.0 |
| chrono | MIT OR Apache-2.0 |
| uuid | Apache-2.0 OR MIT |
| url | MIT OR Apache-2.0 |
| webview2-com | MIT |
| windows / windows-core / windows-numerics | MIT OR Apache-2.0 |

## Direct frontend and build dependencies

| Dependency | Declared license |
| --- | --- |
| Vue, Vite, @vitejs/plugin-vue, vue-tsc | MIT |
| @tauri-apps/api / @tauri-apps/cli | Apache-2.0 OR MIT |
| TypeScript | Apache-2.0 |

The checked Windows Cargo dependency graph also contains compatible permissive dependencies under MIT, Apache-2.0, BSD, Zlib, Unicode-3.0, CC0, 0BSD, and Unlicense terms. Several transitive parser/style crates declare MPL-2.0. They are consumed as unmodified external dependencies; their files remain governed by MPL-2.0 and are not relicensed by this repository.

A future binary release pipeline should generate and ship a complete dependency notice bundle for the exact release lockfiles. v1 ships no installer or release pipeline (the Tauri bundler is disabled), so a `pnpm tauri build --no-bundle` output carries only the executable and its staged runtime payload.

## Windows App SDK runtime payload

The Enhanced rendering backend is hosted through the Windows Composition APIs and therefore ships the self-contained Windows App SDK runtime beside the executable. Those files are Microsoft redistributable components, downloaded from `api.nuget.org` at the pinned versions recorded in [docs/windows-app-sdk-runtime.md](docs/windows-app-sdk-runtime.md), verified by SHA256 and Authenticode before staging, and governed by Microsoft's own license terms rather than by this repository's MIT license. No runtime binary is committed to the repository, and the Standard backend does not use this runtime at all.

## Local appearance assets

The appearance system adds no third-party appearance service or image-analysis dependency. Backgrounds and avatars selected by the user remain local, and current wallpaper access uses the Windows Registry API. Users remain responsible for the rights to images they select; the application copies them only into its own app-data assets directory for local use.

## Weather and geocoding data

Weather data by [Open-Meteo.com](https://open-meteo.com/). Open-Meteo API data are provided under the [Creative Commons Attribution 4.0 International license](https://creativecommons.org/licenses/by/4.0/). Location search uses the Open-Meteo Geocoding API and its GeoNames-derived location database. The application maps provider weather codes into a smaller internal condition vocabulary and caches only that normalized result.

The hosted Free/Open-Access API is also subject to Open-Meteo's current service terms, including non-commercial-use and request-limit conditions. This repository's MIT license covers its source code; it does not grant rights to a third-party hosted service. Commercial redistribution or operation must use an Open-Meteo offering and terms appropriate to that use.

## Windows desktop references

The native adapter was implemented in-project from Win32/WebView2 API behavior and Phase 1 experiments. Architecture references and their known license status are listed in `docs/desktop-mode.md`. No source was copied wholesale from those projects. Repositories for which a suitable license was not established were treated as read-only context and no code was copied.
