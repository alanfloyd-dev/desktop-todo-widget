fn main() {
    println!("cargo:rerun-if-changed=../Cargo.toml");
    let manifest =
        std::fs::read_to_string("../Cargo.toml").expect("read canonical product manifest");
    let package = manifest
        .split("[package]")
        .nth(1)
        .expect("package section")
        .split("\n[")
        .next()
        .expect("package fields");
    let version = package
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = "))
        .expect("product version")
        .trim_matches('"');
    println!("cargo:rustc-env=DTW_PRODUCT_VERSION={version}");
    let parts: Vec<u64> = version
        .split('.')
        .map(|s| s.parse().expect("numeric product version"))
        .collect();
    assert_eq!(parts.len(), 3);
    let numeric = (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16);
    let mut resource = tauri_winres::WindowsResource::new();
    resource.set("ProductName", "desktop-todo-widget").set("FileDescription", "desktop-todo-widget Maintenance")
        .set("ProductVersion", version).set("FileVersion", version)
        .set_version_info(tauri_winres::VersionInfo::PRODUCTVERSION, numeric)
        .set_version_info(tauri_winres::VersionInfo::FILEVERSION, numeric)
        .set_manifest(r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" /></dependentAssembly></dependency><trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false" /></requestedPrivileges></security></trustInfo></assembly>"#);
    resource
        .compile()
        .expect("compile native helper version/manifest resource");
}
