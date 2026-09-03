use crate::settings::AppState;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::WebviewWindow;
use uuid::Uuid;

const MAX_BACKGROUND_BYTES: u64 = 32 * 1024 * 1024;
const MAX_AVATAR_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundType {
    #[default]
    Glass,
    Solid,
    Gradient,
    Image,
    Wallpaper,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextContrast {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedContrast {
    #[default]
    Light,
    Dark,
}

impl ResolvedContrast {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Cover,
    Contain,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePosition {
    #[default]
    Center,
    Top,
    Bottom,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AppearanceSettings {
    pub background_type: BackgroundType,
    pub solid_color: String,
    pub glass_tint_color: String,
    pub glass_tint_opacity: f64,
    pub blur_px: f64,
    pub overlay_strength: f64,
    pub gradient_start_color: String,
    pub gradient_end_color: String,
    pub gradient_angle: f64,
    pub image_asset_id: Option<String>,
    pub image_fit: ImageFit,
    pub image_position: ImagePosition,
    pub background_opacity: f64,
    pub text_contrast: TextContrast,
    pub sampled_luminance: Option<f64>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            background_type: BackgroundType::Glass,
            solid_color: "#11191e".into(),
            glass_tint_color: "#11191e".into(),
            glass_tint_opacity: 0.78,
            blur_px: 14.0,
            overlay_strength: 0.18,
            gradient_start_color: "#11191e".into(),
            gradient_end_color: "#213747".into(),
            gradient_angle: 135.0,
            image_asset_id: None,
            image_fit: ImageFit::Cover,
            image_position: ImagePosition::Center,
            background_opacity: 0.94,
            text_contrast: TextContrast::Auto,
            sampled_luminance: None,
        }
    }
}

impl AppearanceSettings {
    pub fn normalize(&mut self) {
        self.solid_color = normalized_color(&self.solid_color, "#11191e");
        self.glass_tint_color = normalized_color(&self.glass_tint_color, "#11191e");
        self.gradient_start_color = normalized_color(&self.gradient_start_color, "#11191e");
        self.gradient_end_color = normalized_color(&self.gradient_end_color, "#213747");
        self.glass_tint_opacity = finite_clamp(self.glass_tint_opacity, 0.0, 1.0, 0.78);
        self.blur_px = finite_clamp(self.blur_px, 0.0, 24.0, 14.0);
        self.overlay_strength = finite_clamp(self.overlay_strength, 0.0, 0.72, 0.18);
        self.gradient_angle = finite_clamp(self.gradient_angle, 0.0, 360.0, 135.0);
        self.background_opacity = finite_clamp(self.background_opacity, 0.0, 1.0, 0.94);
        self.sampled_luminance = self
            .sampled_luminance
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0.0, 1.0));
        self.image_asset_id = self
            .image_asset_id
            .take()
            .filter(|value| is_managed_asset_id(value, AssetKind::Background));
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Background,
    Avatar,
}

impl AssetKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Background => "background-",
            Self::Avatar => "avatar-",
        }
    }

    fn max_bytes(self) -> u64 {
        match self {
            Self::Background => MAX_BACKGROUND_BYTES,
            Self::Avatar => MAX_AVATAR_BYTES,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPayload {
    pub asset_id: Option<String>,
    pub available: bool,
    pub data_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContrastResult {
    pub resolved: ResolvedContrast,
    pub representative_luminance: f64,
}

#[tauri::command]
pub fn resolve_appearance_contrast(
    mut appearance: AppearanceSettings,
    sampled_luminance: Option<f64>,
    previous: Option<ResolvedContrast>,
) -> ContrastResult {
    appearance.normalize();
    let representative = representative_luminance(&appearance, sampled_luminance);
    let resolved = match appearance.text_contrast {
        TextContrast::Light => ResolvedContrast::Light,
        TextContrast::Dark => ResolvedContrast::Dark,
        TextContrast::Auto => auto_contrast(representative, previous),
    };
    ContrastResult {
        resolved,
        representative_luminance: representative,
    }
}

#[tauri::command]
pub fn choose_local_asset(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    kind: AssetKind,
) -> Result<Option<AssetPayload>, String> {
    let Some(source) = choose_image_file(&window)? else {
        return Ok(None);
    };
    let metadata =
        fs::metadata(&source).map_err(|_| "selected image is unavailable".to_string())?;
    if metadata.len() > kind.max_bytes() {
        return Err("selected image is too large".into());
    }
    let bytes = fs::read(&source).map_err(|_| "selected image could not be read".to_string())?;
    let (extension, mime) = supported_image(&source, &bytes, false)?;
    let assets = managed_assets_dir(&state)?;
    fs::create_dir_all(&assets).map_err(|error| error.to_string())?;
    let asset_id = format!("{}{}.{}", kind.prefix(), Uuid::new_v4(), extension);
    let destination = assets.join(&asset_id);
    fs::write(&destination, &bytes).map_err(|error| error.to_string())?;
    Ok(Some(payload(asset_id, mime, bytes)))
}

#[tauri::command]
pub fn load_managed_asset(
    state: tauri::State<'_, AppState>,
    kind: AssetKind,
    asset_id: String,
) -> Result<AssetPayload, String> {
    if !is_managed_asset_id(&asset_id, kind) {
        return Ok(unavailable(Some(asset_id)));
    }
    let path = managed_assets_dir(&state)?.join(&asset_id);
    let Ok(metadata) = fs::metadata(&path) else {
        return Ok(unavailable(Some(asset_id)));
    };
    if metadata.len() > kind.max_bytes() {
        return Ok(unavailable(Some(asset_id)));
    }
    let Ok(bytes) = fs::read(&path) else {
        return Ok(unavailable(Some(asset_id)));
    };
    let Ok((_, mime)) = supported_image(&path, &bytes, false) else {
        return Ok(unavailable(Some(asset_id)));
    };
    Ok(payload(asset_id, mime, bytes))
}

#[tauri::command]
pub fn load_windows_wallpaper() -> Result<AssetPayload, String> {
    let Some(path) = current_windows_wallpaper() else {
        return Ok(unavailable(None));
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return Ok(unavailable(None));
    };
    if metadata.len() > MAX_BACKGROUND_BYTES {
        return Ok(unavailable(None));
    }
    let Ok(bytes) = fs::read(&path) else {
        return Ok(unavailable(None));
    };
    let Ok((_, mime)) = supported_image(&path, &bytes, true) else {
        return Ok(unavailable(None));
    };
    Ok(AssetPayload {
        asset_id: None,
        available: true,
        data_url: Some(data_url(mime, &bytes)),
    })
}

#[tauri::command]
pub fn discard_managed_asset(
    state: tauri::State<'_, AppState>,
    asset_id: String,
) -> Result<(), String> {
    let settings = state.snapshot()?;
    if settings.appearance_settings.image_asset_id.as_deref() == Some(asset_id.as_str())
        || settings.avatar_asset_id.as_deref() == Some(asset_id.as_str())
    {
        return Err("managed asset is still in use".into());
    }
    remove_managed_asset(&state, &asset_id)
}

pub fn cleanup_replaced_assets(
    state: &AppState,
    before_background: Option<&str>,
    before_avatar: Option<&str>,
    after_background: Option<&str>,
    after_avatar: Option<&str>,
) {
    for old in [before_background, before_avatar].into_iter().flatten() {
        if Some(old) != after_background && Some(old) != after_avatar {
            let _ = remove_managed_asset(state, old);
        }
    }
}

pub fn background_available(state: &AppState, settings: &AppearanceSettings) -> bool {
    match settings.background_type {
        BackgroundType::Glass | BackgroundType::Solid | BackgroundType::Gradient => true,
        BackgroundType::Image => settings.image_asset_id.as_deref().is_some_and(|id| {
            is_managed_asset_id(id, AssetKind::Background)
                && managed_assets_dir(state)
                    .map(|path| path.join(id).is_file())
                    .unwrap_or(false)
        }),
        BackgroundType::Wallpaper => current_windows_wallpaper().is_some_and(|path| path.is_file()),
    }
}

pub fn avatar_available(state: &AppState, asset_id: Option<&str>) -> bool {
    asset_id.is_some_and(|id| {
        is_managed_asset_id(id, AssetKind::Avatar)
            && managed_assets_dir(state)
                .map(|path| path.join(id).is_file())
                .unwrap_or(false)
    })
}

pub fn resolved_for_diagnostics(settings: &AppearanceSettings) -> ResolvedContrast {
    resolve_appearance_contrast(settings.clone(), settings.sampled_luminance, None).resolved
}

fn representative_luminance(settings: &AppearanceSettings, sampled: Option<f64>) -> f64 {
    let sampled = sampled
        .or(settings.sampled_luminance)
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0));
    let raw = match settings.background_type {
        BackgroundType::Solid => color_luminance(&settings.solid_color).unwrap_or(0.08),
        BackgroundType::Gradient => {
            let start = color_luminance(&settings.gradient_start_color).unwrap_or(0.08);
            let end = color_luminance(&settings.gradient_end_color).unwrap_or(0.16);
            (start + end) / 2.0
        }
        BackgroundType::Image | BackgroundType::Wallpaper => sampled.unwrap_or(0.18),
        BackgroundType::Glass => {
            let base = sampled.unwrap_or(0.16);
            let tint = color_luminance(&settings.glass_tint_color).unwrap_or(0.08);
            base * (1.0 - settings.glass_tint_opacity) + tint * settings.glass_tint_opacity
        }
    };
    let fallback = color_luminance("#11191e").unwrap_or(0.08);
    (raw * settings.background_opacity + fallback * (1.0 - settings.background_opacity))
        .clamp(0.0, 1.0)
}

fn auto_contrast(luminance: f64, previous: Option<ResolvedContrast>) -> ResolvedContrast {
    // A small hysteresis band keeps Auto stable near mid-gray when an image
    // sample or a slider changes by only a few points.
    match previous {
        Some(ResolvedContrast::Light) if luminance < 0.62 => ResolvedContrast::Light,
        Some(ResolvedContrast::Dark) if luminance > 0.48 => ResolvedContrast::Dark,
        _ if luminance >= 0.56 => ResolvedContrast::Dark,
        _ => ResolvedContrast::Light,
    }
}

fn normalized_color(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        trimmed.to_ascii_lowercase()
    } else {
        fallback.into()
    }
}

fn finite_clamp(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn color_luminance(value: &str) -> Option<f64> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    let channel = |offset| u8::from_str_radix(&value[offset..offset + 2], 16).ok();
    let linear = |channel: u8| {
        let normalized = f64::from(channel) / 255.0;
        if normalized <= 0.04045 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * linear(channel(0)?) + 0.7152 * linear(channel(2)?) + 0.0722 * linear(channel(4)?))
}

fn managed_assets_dir(state: &AppState) -> Result<PathBuf, String> {
    state
        .database
        .path()
        .parent()
        .map(|parent| parent.join("assets"))
        .ok_or_else(|| "app-data assets directory unavailable".into())
}

fn remove_managed_asset(state: &AppState, asset_id: &str) -> Result<(), String> {
    let kind = if asset_id.starts_with(AssetKind::Background.prefix()) {
        AssetKind::Background
    } else if asset_id.starts_with(AssetKind::Avatar.prefix()) {
        AssetKind::Avatar
    } else {
        return Err("invalid managed asset id".into());
    };
    if !is_managed_asset_id(asset_id, kind) {
        return Err("invalid managed asset id".into());
    }
    let path = managed_assets_dir(state)?.join(asset_id);
    if path.is_file() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn is_managed_asset_id(value: &str, kind: AssetKind) -> bool {
    !value.contains(['/', '\\'])
        && value.starts_with(kind.prefix())
        && value.len() < 96
        && value
            .rsplit_once('.')
            .is_some_and(|(_, extension)| matches!(extension, "png" | "jpg" | "jpeg" | "webp"))
}

pub fn normalize_avatar_asset_id(value: &mut Option<String>) {
    *value = value
        .take()
        .filter(|asset_id| is_managed_asset_id(asset_id, AssetKind::Avatar));
}

fn supported_image(
    path: &std::path::Path,
    bytes: &[u8],
    allow_bmp: bool,
) -> Result<(&'static str, &'static str), String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let signature = if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(("png", "image/png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("jpg", "image/jpeg"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else if allow_bmp && bytes.starts_with(b"BM") {
        Some(("bmp", "image/bmp"))
    } else {
        None
    };
    let Some((normalized_extension, mime)) = signature else {
        return Err("unsupported or corrupted image".into());
    };
    let extension_allowed = matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
        || (allow_bmp && extension == "bmp");
    if !extension_allowed {
        return Err("unsupported image extension".into());
    }
    Ok((normalized_extension, mime))
}

fn payload(asset_id: String, mime: &str, bytes: Vec<u8>) -> AssetPayload {
    AssetPayload {
        asset_id: Some(asset_id),
        available: true,
        data_url: Some(data_url(mime, &bytes)),
    }
}

fn unavailable(asset_id: Option<String>) -> AssetPayload {
    AssetPayload {
        asset_id,
        available: false,
        data_url: None,
    }
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(target_os = "windows")]
fn choose_image_file(window: &WebviewWindow) -> Result<Option<PathBuf>, String> {
    use std::{ffi::c_void, mem::size_of};

    #[repr(C)]
    struct OpenFileNameW {
        struct_size: u32,
        owner: isize,
        instance: isize,
        filter: *const u16,
        custom_filter: *mut u16,
        custom_filter_size: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        default_extension: *const u16,
        custom_data: isize,
        hook: *const c_void,
        template_name: *const u16,
        reserved: *mut c_void,
        reserved_value: u32,
        flags_ex: u32,
    }

    #[link(name = "Comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(value: *mut OpenFileNameW) -> i32;
        fn CommDlgExtendedError() -> u32;
    }

    let filter: Vec<u16> = "Images (*.png;*.jpg;*.jpeg;*.webp)\0*.png;*.jpg;*.jpeg;*.webp\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Choose a local image\0".encode_utf16().collect();
    let mut file = vec![0_u16; 32_768];
    let owner = window
        .hwnd()
        .map(|handle| handle.0 as isize)
        .map_err(|error| error.to_string())?;
    let mut dialog = OpenFileNameW {
        struct_size: size_of::<OpenFileNameW>() as u32,
        owner,
        instance: 0,
        filter: filter.as_ptr(),
        custom_filter: std::ptr::null_mut(),
        custom_filter_size: 0,
        filter_index: 1,
        file: file.as_mut_ptr(),
        max_file: file.len() as u32,
        file_title: std::ptr::null_mut(),
        max_file_title: 0,
        initial_dir: std::ptr::null(),
        title: title.as_ptr(),
        flags: 0x0000_0800 | 0x0000_1000 | 0x0008_0000,
        file_offset: 0,
        file_extension: 0,
        default_extension: std::ptr::null(),
        custom_data: 0,
        hook: std::ptr::null(),
        template_name: std::ptr::null(),
        reserved: std::ptr::null_mut(),
        reserved_value: 0,
        flags_ex: 0,
    };
    // SAFETY: every pointer in OPENFILENAMEW references a live buffer for the
    // duration of the modal call, and the owner HWND belongs to this window.
    let selected = unsafe { GetOpenFileNameW(&mut dialog) } != 0;
    if !selected {
        // A zero extended error is the documented user-cancel result.
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(format!("image picker failed: {error}"))
        };
    }
    let length = file
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(file.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(
        &file[..length],
    ))))
}

#[cfg(not(target_os = "windows"))]
fn choose_image_file(_window: &WebviewWindow) -> Result<Option<PathBuf>, String> {
    Err("local image picker is currently available on Windows only".into())
}

#[cfg(target_os = "windows")]
fn current_windows_wallpaper() -> Option<PathBuf> {
    use std::ffi::c_void;

    type Hkey = isize;
    const HKEY_CURRENT_USER: Hkey = 0x8000_0001_u32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: Hkey,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            value_type: *mut u32,
            data: *mut c_void,
            data_size: *mut u32,
        ) -> i32;
    }
    let wide = |value: &str| {
        value
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let key = wide(r"Control Panel\Desktop");
    let name = wide("WallPaper");
    let mut buffer = vec![0_u16; 32_768];
    let mut bytes = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    // SAFETY: the output buffer is writable for `bytes`; both registry strings
    // are terminated and no returned path leaves this privacy boundary.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..length]);
    (!value.trim().is_empty()).then(|| PathBuf::from(value))
}

#[cfg(not(target_os = "windows"))]
fn current_windows_wallpaper() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_graphite_frost_glass() {
        let settings = AppearanceSettings::default();
        assert_eq!(settings.background_type, BackgroundType::Glass);
        assert_eq!(settings.text_contrast, TextContrast::Auto);
        assert_eq!(settings.blur_px, 14.0);
    }

    #[test]
    fn invalid_values_are_clamped_or_replaced() {
        let mut settings = AppearanceSettings {
            solid_color: "not-a-color".into(),
            glass_tint_opacity: -4.0,
            blur_px: 200.0,
            overlay_strength: 2.0,
            gradient_angle: 900.0,
            background_opacity: -1.0,
            image_asset_id: Some("../../private.png".into()),
            ..AppearanceSettings::default()
        };
        settings.normalize();
        assert_eq!(settings.solid_color, "#11191e");
        assert_eq!(settings.glass_tint_opacity, 0.0);
        assert_eq!(settings.blur_px, 24.0);
        assert_eq!(settings.overlay_strength, 0.72);
        assert_eq!(settings.gradient_angle, 360.0);
        assert_eq!(settings.background_opacity, 0.0);
        assert!(settings.image_asset_id.is_none());
    }

    #[test]
    fn contrast_covers_dark_light_gradient_image_and_manual_modes() {
        let mut settings = AppearanceSettings {
            background_type: BackgroundType::Solid,
            solid_color: "#000000".into(),
            ..AppearanceSettings::default()
        };
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), None, None).resolved,
            ResolvedContrast::Light
        );
        settings.solid_color = "#ffffff".into();
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), None, None).resolved,
            ResolvedContrast::Dark
        );
        settings.background_type = BackgroundType::Gradient;
        settings.gradient_start_color = "#f5f5f5".into();
        settings.gradient_end_color = "#ffffff".into();
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), None, None).resolved,
            ResolvedContrast::Dark
        );
        settings.gradient_start_color = "#000000".into();
        settings.gradient_end_color = "#111111".into();
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), None, None).resolved,
            ResolvedContrast::Light
        );
        settings.background_type = BackgroundType::Image;
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), Some(0.9), None).resolved,
            ResolvedContrast::Dark
        );
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), Some(0.1), None).resolved,
            ResolvedContrast::Light
        );
        settings.text_contrast = TextContrast::Light;
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), Some(1.0), None).resolved,
            ResolvedContrast::Light
        );
        settings.text_contrast = TextContrast::Dark;
        assert_eq!(
            resolve_appearance_contrast(settings, Some(0.0), None).resolved,
            ResolvedContrast::Dark
        );
    }

    #[test]
    fn contrast_hysteresis_is_stable_around_threshold() {
        assert_eq!(
            auto_contrast(0.58, Some(ResolvedContrast::Light)),
            ResolvedContrast::Light
        );
        assert_eq!(
            auto_contrast(0.52, Some(ResolvedContrast::Dark)),
            ResolvedContrast::Dark
        );
        assert_eq!(
            auto_contrast(0.70, Some(ResolvedContrast::Light)),
            ResolvedContrast::Dark
        );
        assert_eq!(
            auto_contrast(0.40, Some(ResolvedContrast::Dark)),
            ResolvedContrast::Light
        );
    }

    #[test]
    fn image_signatures_reject_corruption_and_accept_fixtures() {
        assert!(supported_image(std::path::Path::new("fixture.png"), b"broken", false).is_err());
        assert_eq!(
            supported_image(
                std::path::Path::new("fixture.png"),
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                false
            ),
            Ok(("png", "image/png"))
        );
        assert_eq!(base64(b"Alan"), "QWxhbg==");
    }

    #[test]
    fn managed_asset_ids_cannot_escape_app_data() {
        assert!(is_managed_asset_id(
            "avatar-00000000-0000-0000-0000-000000000000.png",
            AssetKind::Avatar
        ));
        assert!(!is_managed_asset_id(
            "../avatar-private.png",
            AssetKind::Avatar
        ));
        assert!(!is_managed_asset_id(
            "background-safe.svg",
            AssetKind::Background
        ));
    }
}
