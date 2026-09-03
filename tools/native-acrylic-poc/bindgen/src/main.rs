use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let poc = generator
        .parent()
        .ok_or("bindgen project must remain inside the PoC")?;
    let metadata =
        poc.join("target/self-contained/stage/interactive-experiences/metadata/10.0.18362.0");
    let microsoft_ui = metadata.join("Microsoft.UI.winmd");
    let microsoft_foundation = metadata.join("Microsoft.Foundation.winmd");
    if !microsoft_ui.is_file() || !microsoft_foundation.is_file() {
        return Err("run scripts/prepare-self-contained.ps1 before generating bindings".into());
    }
    let output = poc.join("src/winappsdk.rs");

    windows_bindgen::bindgen([
        "--in".into(),
        "default".into(),
        microsoft_ui.to_string_lossy().into_owned(),
        microsoft_foundation.to_string_lossy().into_owned(),
        "--out".into(),
        output.to_string_lossy().into_owned(),
        "--reference".into(),
        "windows".into(),
        "--filter".into(),
        "Microsoft.UI.WindowId".into(),
        "Microsoft.UI.ClosableNotifierHandler".into(),
        "Microsoft.UI.Composition.ICompositionSupportsSystemBackdrop".into(),
        "Microsoft.UI.Composition.SystemBackdrops.DesktopAcrylicController".into(),
        "Microsoft.UI.Composition.SystemBackdrops.SystemBackdropConfiguration".into(),
        "Microsoft.UI.Composition.SystemBackdrops.DesktopAcrylicKind".into(),
        "Microsoft.UI.Composition.SystemBackdrops.SystemBackdropState".into(),
        "Microsoft.UI.Composition.SystemBackdrops.SystemBackdropTheme".into(),
    ])?;
    Ok(())
}
