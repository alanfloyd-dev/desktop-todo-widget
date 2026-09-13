// Prevents an additional console window on Windows release builds.
//
// The Windows linker defaults to the console subsystem, so a double-clicked
// release executable made Windows allocate a visible console window for a
// product that has no console UI. This attribute selects the GUI subsystem for
// release builds only:
//
//   * `tauri dev` / `cargo build` (debug) keep the console, so `eprintln!`
//     diagnostics stay visible while developing.
//   * Release builds have no console window; their diagnostics are unaffected
//     because `qa_diagnostics` writes the same records to a log file beside the
//     executable in every profile, and mirrors panic reports there too (see
//     `qa_diagnostics::install_panic_logging`).
//
// This is the standard Rust/Tauri entry-point attribute for the Windows
// subsystem; no platform code reads it at runtime.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    alan_desktop_lib::run();
}
