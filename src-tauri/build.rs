fn main() {
    println!("cargo:rerun-if-changed=app.manifest");

    #[cfg(target_os = "windows")]
    {
        // The base manifest carries what the product itself needs (the
        // Common-Controls dependency). The WinRT activation entries of the
        // retired composition path are no longer merged in: a standard-only
        // build has no RegFree WinRT surface, so the embedded manifest is the
        // repository file verbatim.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
        let base = std::path::Path::new(&manifest_dir).join("app.manifest");
        let base_xml = std::fs::read_to_string(&base)
            .unwrap_or_else(|error| panic!("unable to read app manifest at {}: {error}", base.display()));
        let windows = tauri_build::WindowsAttributes::new().app_manifest(base_xml);
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("failed to run Tauri build script");
    }

    #[cfg(not(target_os = "windows"))]
    tauri_build::build()
}
