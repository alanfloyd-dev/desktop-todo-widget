use std::{env, fs, path::Path, path::PathBuf};

/// Payload file extensions copied beside the executable. The stager's agent
/// executables (`DeploymentAgent.exe`, `RestartAgent.exe`) and its own generated
/// manifest are intentionally excluded: they are not part of this deployment model.
const PAYLOAD_EXTENSIONS: [&str; 3] = ["dll", "winmd", "pri"];

/// Files that must exist in a staged payload before it counts as usable.
const REQUIRED_PAYLOAD_FILES: [&str; 3] = [
    "Microsoft.WindowsAppRuntime.dll",
    "wuceffectsi.dll",
    "composition-host.manifest",
];

fn main() {
    // Re-run when the staged payload changes so a freshly staged runtime is
    // always copied next to the executable.
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=../tools/windows-app-sdk/runtime");

    #[cfg(target_os = "windows")]
    {
        let manifest = resolve_embedded_manifest();
        let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("failed to run Tauri build script");
        stage_windows_app_sdk_payload();
    }

    #[cfg(not(target_os = "windows"))]
    tauri_build::build()
}

/// Manifest embedded into the executable.
///
/// The base manifest lives in the repository. When the self-contained Windows App
/// SDK payload has been staged, the WinRT activation entries generated from the
/// pinned runtime packages are merged in, so the activatable-class list cannot
/// drift from the runtime that actually ships beside the exe.
///
/// This is the load-bearing part of the deployment model: for an unpackaged
/// self-contained app there is no bootstrapper and no package graph, so Undocked
/// RegFree WinRT resolves the Windows App SDK activation classes from this
/// manifest plus the DLLs next to the exe.
#[cfg(target_os = "windows")]
fn resolve_embedded_manifest() -> String {
    let manifest_dir = manifest_dir();
    let base = manifest_dir.join("app.manifest");
    let activation = payload_dir(&manifest_dir).join("composition-host.manifest");

    let Ok(base_xml) = fs::read_to_string(&base) else {
        panic!("unable to read app manifest at {}", base.display());
    };

    if !activation.is_file() {
        eprintln!(
            "[phase7c3] windows_app_sdk_activation_manifest=absent; embedding the base manifest only"
        );
        return base_xml;
    }

    let Ok(activation_xml) = fs::read_to_string(&activation) else {
        eprintln!(
            "[phase7c3] activation_manifest_read_failed path={}",
            activation.display()
        );
        return base_xml;
    };

    eprintln!("[phase7c3] windows_app_sdk_activation_manifest=merged");
    merge_activation_manifest(&base_xml, &activation_xml)
}

/// Splices the generated WinRT activation blocks into the base application
/// manifest.
///
/// `activation` is an XML *fragment* wrapper:
///
/// ```xml
/// <asmv3:files xmlns:asmv3="...">
///   <asmv3:file name="wuceffectsi.dll">
///     <winrtv1:activatableClass ... />
///   </asmv3:file>
///   ...
/// </asmv3:files>
/// ```
///
/// Its inner markup is inserted verbatim before the base manifest's closing
/// `</assembly>`, preserving every opening and closing tag. A text-only copy of
/// the element names would silently drop the `</asmv3:file>` tags and produce a
/// manifest that Windows rejects with a side-by-side configuration error, so the
/// merged result is parsed back as XML before it is used.
#[cfg(target_os = "windows")]
fn merge_activation_manifest(base: &str, activation: &str) -> String {
    let Some(close_index) = base.rfind("</assembly>") else {
        panic!("the base app manifest has no closing </assembly> tag");
    };
    let Some(inner) = fragment_inner_markup(activation) else {
        panic!("the generated activation manifest has no root element");
    };

    let mut merged = String::with_capacity(base.len() + inner.len());
    merged.push_str(&base[..close_index]);
    merged.push_str(inner.trim());
    merged.push('\n');
    merged.push_str(&base[close_index..]);

    validate_manifest_xml(&merged);
    merged
}

/// Returns the markup between the activation fragment's root open and close tags.
#[cfg(target_os = "windows")]
fn fragment_inner_markup(fragment: &str) -> Option<&str> {
    let open_end = fragment.find('>')? + 1;
    let close_start = fragment.rfind("</")?;
    if close_start < open_end {
        return None;
    }
    Some(&fragment[open_end..close_start])
}

/// Rejects a merged manifest Windows would refuse to load.
///
/// The tag-balance check is deliberately simple: it catches the failure this
/// function exists to prevent (dropped closing tags) without pulling in an XML
/// parser as a build dependency.
#[cfg(target_os = "windows")]
fn validate_manifest_xml(manifest: &str) {
    for element in ["asmv3:file", "winrtv1:activatableClass", "assembly", "dependency"] {
        let open_tag = format!("<{element}");
        let close_tag = format!("</{element}>");
        let opens = manifest.matches(&open_tag).count();
        let self_closed = manifest
            .match_indices(&open_tag)
            .filter(|(index, _)| {
                manifest[*index..]
                    .find('>')
                    .is_some_and(|end| manifest[*index..index + end].ends_with('/'))
            })
            .count();
        let closes = manifest.matches(&close_tag).count();
        assert_eq!(
            opens - self_closed,
            closes,
            "merged manifest is unbalanced for <{element}>: {opens} open, {self_closed} self-closed, {closes} closed"
        );
    }
}

/// Copies the staged self-contained Windows App SDK payload beside the
/// executable for the active profile (debug or release).
///
/// The runtime is loaded with `LoadLibraryW("Microsoft.WindowsAppRuntime.dll")`
/// and activated through Undocked RegFree WinRT, so the whole payload has to sit
/// next to `alan-desktop.exe`. A missing or incomplete payload is a build error:
/// shipping an executable that cannot start its composition path must not be
/// possible, and the message names the exact command that produces the payload.
#[cfg(target_os = "windows")]
fn stage_windows_app_sdk_payload() {
    let manifest_dir = manifest_dir();
    let source = payload_dir(&manifest_dir);
    let Some(destination) = executable_dir() else {
        eprintln!(
            "[phase7c3] windows_app_sdk_payload=skipped reason=no_profile_dir out_dir={}",
            env::var("OUT_DIR").unwrap_or_default()
        );
        return;
    };

    if let Err(reason) = validate_payload(&source) {
        panic!(
            "{reason}\n\
             The optional Windows CompositionController hosting path needs the self-contained\n\
             Windows App SDK runtime beside the executable. Stage it with:\n\
             \x20   powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1\n\
             See docs/windows-app-sdk-runtime.md for the deployment model and version policy."
        );
    }

    let Ok(entries) = fs::read_dir(&source) else {
        panic!("staged Windows App SDK payload is unreadable: {}", source.display());
    };

    let mut copied = 0_usize;
    let mut failures: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_payload_file(&path) {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        if let Err(error) = fs::copy(&path, destination.join(name)) {
            failures.push((name.to_string_lossy().into_owned(), error.to_string()));
        } else {
            copied += 1;
        }
    }

    if !failures.is_empty() {
        for (name, error) in &failures {
            eprintln!("[phase7c3] windows_app_sdk_payload_copy_failed file={name} error={error}");
        }
        panic!(
            "failed to copy {} Windows App SDK payload file(s) into {}",
            failures.len(),
            destination.display()
        );
    }

    eprintln!(
        "[phase7c3] windows_app_sdk_payload=staged files={copied} target={}",
        destination.display()
    );
}

#[cfg(target_os = "windows")]
fn validate_payload(source: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "staged Windows App SDK payload not found at {}",
            source.display()
        ));
    }
    let missing: Vec<&str> = REQUIRED_PAYLOAD_FILES
        .iter()
        .copied()
        .filter(|name| !source.join(name).is_file())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "staged Windows App SDK payload at {} is incomplete; missing: {}",
            source.display(),
            missing.join(", ")
        ))
    }
}

#[cfg(target_os = "windows")]
fn is_payload_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| PAYLOAD_EXTENSIONS.contains(&extension))
}

#[cfg(target_os = "windows")]
fn manifest_dir() -> PathBuf {
    env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Staged payload root produced by `tools/windows-app-sdk/prepare-runtime-payload.ps1`.
#[cfg(target_os = "windows")]
fn payload_dir(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("..")
        .join("tools")
        .join("windows-app-sdk")
        .join("runtime")
        .join(PAYLOAD_ARCHITECTURE)
}

/// Payload architecture. Only x64 is staged today; adding arm64 means staging a
/// matching directory and mapping the build target here.
#[cfg(target_os = "windows")]
const PAYLOAD_ARCHITECTURE: &str = "x64";

#[cfg(target_os = "windows")]
fn executable_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    // .../<target>/<profile>/build/<crate>-<hash>/out
    let profile_dir = out_dir.ancestors().nth(3)?;
    Some(profile_dir.to_path_buf())
}
