use crate::settings::{AppState, ProductWindowMode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};
use tauri::WebviewWindow;
use uuid::Uuid;

const MAX_BACKGROUND_BYTES: u64 = 32 * 1024 * 1024;
const MAX_AVATAR_BYTES: u64 = 8 * 1024 * 1024;

/// Largest source file the avatar picker will read without persisting it.
///
/// The picker no longer stores what the user selected: the frontend decodes the
/// chosen image, crops it to a square and scales it to [`AVATAR_EDGE_PX`], and
/// only that normalized PNG is persisted (a few tens of kilobytes). The source
/// cap therefore only bounds what can be read and handed over in one IPC payload,
/// and it covers a high-resolution phone photo or screenshot rather than the
/// 56 DIP circle those files would end up in. Above it the picker refuses with a
/// message the UI shows next to the control.
const MAX_AVATAR_SOURCE_BYTES: u64 = 24 * 1024 * 1024;

/// Edge length of a persisted avatar, in pixels.
///
/// Kept in step with `AVATAR_EDGE_PX` in `src/appearance.ts`, which performs the
/// resize; this constant documents the size the backend expects to receive and
/// bounds what it will store.
pub const AVATAR_EDGE_PX: u32 = 256;

/// Rejection messages the frontend maps to localized copy.
///
/// Stable, machine-checked phrases rather than free-form text: the frontend
/// matches them to show a translated message, and the unit tests below pin the
/// same phrases, so changing one here cannot silently degrade the UI to an
/// untranslated or generic message.
const ERROR_IMAGE_TOO_LARGE: &str = "selected image is too large";
const ERROR_IMAGE_UNSUPPORTED: &str = "unsupported image";
const ERROR_IMAGE_UNREADABLE: &str = "selected image could not be read";

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
    /// A user-chosen [`AppearanceSettings::custom_text_color`] for product body
    /// and secondary text. The palette direction (accent, semantic and overlay
    /// colours) is derived from that colour, so the rest of the surface keeps
    /// matching the text the user picked.
    Custom,
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
    /// Product text colour used when `text_contrast` is `custom`.
    pub custom_text_color: String,
    pub sampled_luminance: Option<f64>,
    /// Unknown members written by a future version of this nested document.
    ///
    /// The per-mode appearance profile is a persisted, independently evolvable
    /// object (its fields have grown before), and it lives inside the settings
    /// document that every startup rewrites — without this map, a rollback to
    /// an older runtime would permanently delete future material fields. The
    /// Phase 2A settings unknown-key contract requires nested objects to
    /// round-trip too, not only the top-level document. `normalize` replaces
    /// only invalid *values* of known fields and never consults this map.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
            // Matches the default graphite body text, so selecting Custom before
            // picking a colour does not change the surface.
            custom_text_color: "#d3dade".into(),
            sampled_luminance: None,
            extra: BTreeMap::new(),
        }
    }
}

impl AppearanceSettings {
    pub fn normalize(&mut self) {
        self.solid_color = normalized_color(&self.solid_color, "#11191e");
        self.glass_tint_color = normalized_color(&self.glass_tint_color, "#11191e");
        self.gradient_start_color = normalized_color(&self.gradient_start_color, "#11191e");
        self.gradient_end_color = normalized_color(&self.gradient_end_color, "#213747");
        self.custom_text_color = normalized_color(&self.custom_text_color, "#d3dade");
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

/// One complete appearance per window mode.
///
/// Sidebar, Floating, and Desktop are separate products on screen: each owns its
/// own material, wallpaper/image, opacity, overlay, and text presentation. Every
/// appearance consumer therefore selects the profile of the *current* window mode
/// instead of reading one shared value, which is what keeps a Desktop edit from
/// repainting Floating and makes a mode switch restore that mode's own look.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AppearanceProfiles {
    pub sidebar: AppearanceSettings,
    pub floating: AppearanceSettings,
    pub desktop: AppearanceSettings,
    /// Unknown members written by a future version of this object (for
    /// example a profile for a mode this runtime does not know). Round-tripped
    /// verbatim per the Phase 2A settings unknown-key contract; a rollback to
    /// this runtime must not delete them.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl AppearanceProfiles {
    /// Copies one appearance into every mode.
    ///
    /// Used by the legacy migration: a pre-profile document had a single
    /// appearance for all three modes, and fanning it out is what preserves an
    /// existing user's look across the upgrade.
    pub fn from_all(settings: AppearanceSettings) -> Self {
        Self {
            sidebar: settings.clone(),
            floating: settings.clone(),
            desktop: settings,
            extra: BTreeMap::new(),
        }
    }

    pub fn for_mode(&self, mode: ProductWindowMode) -> &AppearanceSettings {
        match mode {
            ProductWindowMode::Sidebar => &self.sidebar,
            ProductWindowMode::Floating => &self.floating,
            ProductWindowMode::Desktop => &self.desktop,
        }
    }

    pub fn normalize(&mut self) {
        self.sidebar.normalize();
        self.floating.normalize();
        self.desktop.normalize();
    }

    /// Every managed background image still referenced by a profile.
    ///
    /// Asset cleanup and discard have to consider all three: a file released by
    /// the profile being edited is still in use when another mode references it.
    pub fn background_asset_ids(&self) -> impl Iterator<Item = &str> {
        [&self.sidebar, &self.floating, &self.desktop]
            .into_iter()
            .filter_map(|profile| profile.image_asset_id.as_deref())
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

    /// Largest payload this kind may *persist*.
    fn max_bytes(self) -> u64 {
        match self {
            Self::Background => MAX_BACKGROUND_BYTES,
            Self::Avatar => MAX_AVATAR_BYTES,
        }
    }

    /// Largest source file that may be *read* for this kind.
    ///
    /// An avatar is read only to be normalized, so the source may legitimately be
    /// far larger than the artifact that gets stored.
    fn max_source_bytes(self) -> u64 {
        match self {
            Self::Background => MAX_BACKGROUND_BYTES,
            Self::Avatar => MAX_AVATAR_SOURCE_BYTES,
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
        TextContrast::Custom => custom_contrast(&appearance.custom_text_color)
            .unwrap_or_else(|| auto_contrast(representative, previous)),
        TextContrast::Auto => auto_contrast(representative, previous),
    };
    ContrastResult {
        resolved,
        representative_luminance: representative,
    }
}

/// Opens the picker and returns the chosen image.
///
/// `persist` defaults to `true`, which is the background-image contract: the
/// picked file is validated and copied into the managed assets directory, and the
/// payload carries the new asset id.
///
/// `persist = false` is the avatar contract. The chosen file is validated and
/// returned as a data URL but **nothing is written**: the frontend decodes it,
/// normalizes it to [`AVATAR_EDGE_PX`] square, and stores that through
/// [`store_managed_asset`]. That keeps the original — which may be a multi-megabyte
/// photo — out of the profile entirely, and it means the source cap used here
/// ([`AssetKind::max_source_bytes`]) is about what can be read, not about what is
/// kept.
#[tauri::command]
pub fn choose_local_asset(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    kind: AssetKind,
    persist: Option<bool>,
) -> Result<Option<AssetPayload>, String> {
    let persist = persist.unwrap_or(true);
    let Some(source) = choose_image_file(&window)? else {
        return Ok(None);
    };
    let metadata = fs::metadata(&source).map_err(|_| ERROR_IMAGE_UNREADABLE.to_string())?;
    let cap = if persist {
        kind.max_bytes()
    } else {
        kind.max_source_bytes()
    };
    if metadata.len() > cap {
        return Err(ERROR_IMAGE_TOO_LARGE.into());
    }
    let bytes = fs::read(&source).map_err(|_| ERROR_IMAGE_UNREADABLE.to_string())?;
    let (extension, mime) = supported_image(&source, &bytes, false)?;
    if !persist {
        return Ok(Some(AssetPayload {
            asset_id: None,
            available: true,
            data_url: Some(data_url(mime, &bytes)),
        }));
    }
    let assets = managed_assets_dir(&state)?;
    fs::create_dir_all(&assets).map_err(|error| error.to_string())?;
    let asset_id = format!("{}{}.{}", kind.prefix(), Uuid::new_v4(), extension);
    let destination = assets.join(&asset_id);
    fs::write(&destination, &bytes).map_err(|error| error.to_string())?;
    Ok(Some(payload(asset_id, mime, bytes)))
}

/// Stores a normalized image the frontend produced.
///
/// The avatar flow is: pick (validate, hand over, store nothing) → normalize in
/// the WebView (decode, EXIF orientation, centre crop, scale to
/// [`AVATAR_EDGE_PX`], re-encode as PNG so alpha survives) → this command. It is
/// deliberately the only way a normalized image reaches the profile, so the file
/// that ends up beside the database is small and the format is one the app itself
/// chose.
///
/// The payload must be a `data:` URL whose declared media type matches the file's
/// own signature, and the result is bounded by [`AssetKind::max_bytes`]. Nothing
/// here trusts the caller's declared type: a mismatch is a rejection, not a
/// stored file with a lying extension.
#[tauri::command]
pub fn store_managed_asset(
    state: tauri::State<'_, AppState>,
    kind: AssetKind,
    data_url: String,
) -> Result<AssetPayload, String> {
    let (bytes, declared_mime) = decode_image_data_url(&data_url)?;
    if bytes.len() as u64 > kind.max_bytes() {
        return Err(ERROR_IMAGE_TOO_LARGE.into());
    }
    let (extension, mime) = image_signature(&bytes, false)?;
    if mime != declared_mime {
        return Err(format!("{ERROR_IMAGE_UNSUPPORTED}: payload declares {declared_mime} but contains {mime}").into());
    }
    let assets = managed_assets_dir(&state)?;
    fs::create_dir_all(&assets).map_err(|error| error.to_string())?;
    let asset_id = format!("{}{}.{}", kind.prefix(), Uuid::new_v4(), extension);
    fs::write(assets.join(&asset_id), &bytes).map_err(|error| error.to_string())?;
    Ok(payload(asset_id, mime, bytes))
}

/// Splits and decodes a base64 `data:image/...` URL.
///
/// Strict on purpose: the three media types the product itself produces are the
/// only ones accepted, and the `;base64` marker is required. Anything else is a
/// programming error or a hostile payload, not a user's image.
fn decode_image_data_url(value: &str) -> Result<(Vec<u8>, &'static str), String> {
    let (header, encoded) = value
        .split_once(',')
        .ok_or_else(|| ERROR_IMAGE_UNREADABLE.to_string())?;
    let mime = match header {
        "data:image/png;base64" => "image/png",
        "data:image/jpeg;base64" => "image/jpeg",
        "data:image/webp;base64" => "image/webp",
        _ => return Err(ERROR_IMAGE_UNSUPPORTED.into()),
    };
    let bytes = base64_decode(encoded).ok_or_else(|| ERROR_IMAGE_UNREADABLE.to_string())?;
    if bytes.is_empty() {
        return Err(ERROR_IMAGE_UNREADABLE.into());
    }
    Ok((bytes, mime))
}

/// Decodes standard base64, returning `None` for anything malformed.
///
/// Rejects non-alphabet characters, wrong padding, and truncated group lengths, so
/// a corrupt payload can never be written as a file that merely looks like an
/// image to the extension check.
fn base64_decode(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits = 0_u32;
    let mut padding = 0_usize;
    for byte in value.bytes() {
        if byte == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None;
        }
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            // Whitespace inside a data URL is not part of the encoding.
            b'\r' | b'\n' => continue,
            _ => return None,
        } as u32;
        accumulator = (accumulator << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    // The remaining bits must be the zero padding of a complete group.
    if bits >= 6 || padding > 2 {
        return None;
    }
    if accumulator & ((1 << bits) - 1) != 0 {
        return None;
    }
    if (output.len() + padding) % 3 != 0 {
        return None;
    }
    Some(output)
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
    if settings
        .appearance_profiles
        .background_asset_ids()
        .any(|in_use| in_use == asset_id)
        || settings.avatar_asset_id.as_deref() == Some(asset_id.as_str())
    {
        return Err("managed asset is still in use".into());
    }
    remove_managed_asset(&state, &asset_id)
}

/// Deletes background/avatar files that the new settings no longer reference.
///
/// Background images are per-profile now, so "still referenced" has to be asked
/// across all three profiles: an image the edited mode just dropped is still
/// live when another mode kept it.
pub fn cleanup_replaced_assets(
    state: &AppState,
    before: &AppearanceProfiles,
    before_avatar: Option<&str>,
    after: &AppearanceProfiles,
    after_avatar: Option<&str>,
) {
    for old in before.background_asset_ids().chain(before_avatar) {
        if !after.background_asset_ids().any(|kept| kept == old) && after_avatar != Some(old) {
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

/// Resolves the palette direction a custom text colour implies.
///
/// Bright custom text is light-on-dark text, so it keeps the light palette —
/// including the darkening readability scrim — that the accent, semantic, and
/// warning colours are tuned for; a dark custom colour does the opposite. This
/// is what lets a custom colour answer a background the background heuristic
/// misjudges, instead of fighting it. Returns `None` only for an unparsable
/// colour, which `normalize` has already replaced.
fn custom_contrast(color: &str) -> Option<ResolvedContrast> {
    color_luminance(color).map(|luminance| {
        if luminance >= 0.5 {
            ResolvedContrast::Light
        } else {
            ResolvedContrast::Dark
        }
    })
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

/// Identifies an image from its own bytes, ignoring any file name.
///
/// The signature is the authority for what the bytes are; [`supported_image`]
/// additionally requires the file extension to agree with it, which only makes
/// sense for a file the user picked, not for a payload the app itself produced.
fn image_signature(bytes: &[u8], allow_bmp: bool) -> Result<(&'static str, &'static str), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Ok(("png", "image/png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(("jpg", "image/jpeg"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Ok(("webp", "image/webp"))
    } else if allow_bmp && bytes.starts_with(b"BM") {
        Ok(("bmp", "image/bmp"))
    } else {
        Err(ERROR_IMAGE_UNSUPPORTED.into())
    }
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
    let (normalized_extension, mime) = image_signature(bytes, allow_bmp)?;
    let extension_allowed = matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
        || (allow_bmp && extension == "bmp");
    if !extension_allowed {
        return Err(format!("{ERROR_IMAGE_UNSUPPORTED} extension").into());
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
            custom_text_color: "not-a-color".into(),
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
        assert_eq!(settings.custom_text_color, "#d3dade");
        assert!(settings.image_asset_id.is_none());
    }

    #[test]
    fn appearance_profiles_select_the_profile_of_the_requested_mode() {
        let profiles = AppearanceProfiles {
            sidebar: AppearanceSettings {
                solid_color: "#010203".into(),
                ..AppearanceSettings::default()
            },
            floating: AppearanceSettings {
                solid_color: "#040506".into(),
                ..AppearanceSettings::default()
            },
            desktop: AppearanceSettings {
                solid_color: "#070809".into(),
                ..AppearanceSettings::default()
            },
            ..AppearanceProfiles::default()
        };
        assert_eq!(
            profiles.for_mode(ProductWindowMode::Sidebar).solid_color,
            "#010203"
        );
        assert_eq!(
            profiles.for_mode(ProductWindowMode::Floating).solid_color,
            "#040506"
        );
        assert_eq!(
            profiles.for_mode(ProductWindowMode::Desktop).solid_color,
            "#070809"
        );
        assert_eq!(profiles.background_asset_ids().count(), 0);
    }

    /// A profile edit is local: writing through one mode's handle must leave the
    /// other two exactly as they were.
    #[test]
    fn editing_one_profile_leaves_the_others_untouched() {
        let mut profiles = AppearanceProfiles::from_all(AppearanceSettings::default());
        profiles.desktop.background_type = BackgroundType::Solid;
        assert_eq!(
            profiles
                .for_mode(ProductWindowMode::Sidebar)
                .background_type,
            BackgroundType::Glass,
            "Sidebar must not inherit a Desktop edit"
        );
        assert_eq!(
            profiles
                .for_mode(ProductWindowMode::Floating)
                .background_type,
            BackgroundType::Glass
        );
        assert_eq!(
            profiles
                .for_mode(ProductWindowMode::Desktop)
                .background_type,
            BackgroundType::Solid
        );
    }

    #[test]
    fn only_referenced_profiles_keep_a_background_asset_alive() {
        let shared = "background-00000000-0000-0000-0000-000000000000.png";
        let profiles = AppearanceProfiles {
            sidebar: AppearanceSettings::default(),
            floating: AppearanceSettings {
                image_asset_id: Some(shared.into()),
                ..AppearanceSettings::default()
            },
            desktop: AppearanceSettings {
                image_asset_id: Some(shared.into()),
                ..AppearanceSettings::default()
            },
            ..AppearanceProfiles::default()
        };
        let mut released = profiles.clone();
        released.desktop.image_asset_id = None;
        assert!(
            released.background_asset_ids().any(|id| id == shared),
            "the Floating profile still references the image"
        );
        released.floating.image_asset_id = None;
        assert!(!released.background_asset_ids().any(|id| id == shared));
    }

    /// Custom text has to set the palette direction itself: a bright custom
    /// colour is light text and keeps the light accent/semantic palette, so the
    /// readability scrim stays behind it. The background sample deliberately
    /// disagrees here.
    #[test]
    fn custom_text_colour_decides_the_palette_direction() {
        let mut settings = AppearanceSettings {
            text_contrast: TextContrast::Custom,
            custom_text_color: "#f5f7f8".into(),
            ..AppearanceSettings::default()
        };
        // A near-white background would resolve Dark under Auto.
        settings.background_type = BackgroundType::Solid;
        settings.solid_color = "#ffffff".into();
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), None, None).resolved,
            ResolvedContrast::Light
        );
        settings.custom_text_color = "#12181c".into();
        assert_eq!(
            resolve_appearance_contrast(settings.clone(), Some(0.0), None).resolved,
            ResolvedContrast::Dark
        );
        // An unusable colour never reaches the contrast decision: `normalize`
        // replaces it with the default custom colour, which is light text.
        assert_eq!(custom_contrast("chartreuse"), None);
        settings.custom_text_color = "chartreuse".into();
        settings.solid_color = "#ffffff".into();
        assert_eq!(
            resolve_appearance_contrast(settings, None, None).resolved,
            ResolvedContrast::Light
        );
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

    /// The avatar source cap exists so a multi-megabyte photo can be read and
    /// normalized; the *stored* avatar stays under the small artifact bound.
    #[test]
    fn avatar_source_cap_covers_photos_the_stored_artifact_must_not() {
        assert!(
            AssetKind::Avatar.max_source_bytes() > AssetKind::Avatar.max_bytes(),
            "the picker must be able to read a photo it will not keep"
        );
        assert_eq!(AssetKind::Avatar.max_source_bytes(), 24 * 1024 * 1024);
        assert_eq!(AssetKind::Avatar.max_bytes(), 8 * 1024 * 1024);
        // Backgrounds keep one cap: they are stored at full resolution.
        assert_eq!(
            AssetKind::Background.max_source_bytes(),
            AssetKind::Background.max_bytes()
        );
        // The frontend resizes to this edge; the two must not drift apart.
        assert_eq!(AVATAR_EDGE_PX, 256);
    }

    /// Base64 decoding is strict: the normalizer's output must round-trip, and a
    /// malformed payload must never be written as a file.
    #[test]
    fn base64_decoding_round_trips_and_rejects_malformed_input() {
        for payload in [
            &b""[..],
            &b"a"[..],
            &b"ab"[..],
            &b"abc"[..],
            &b"Alan Floyd"[..],
            &[0x89, b'P', b'N', b'G', 0xff, 0x00, 0x7f][..],
        ] {
            let encoded = base64(payload);
            assert_eq!(
                base64_decode(&encoded).as_deref(),
                Some(payload),
                "round trip failed for {encoded}"
            );
        }
        // Whitespace inside the payload is tolerated, everything else is not.
        assert_eq!(base64_decode("QWxh\nbg==").as_deref(), Some(&b"Alan"[..]));
        assert_eq!(base64_decode("QWxh bg=="), None);
        assert_eq!(base64_decode("****"), None);
        assert_eq!(base64_decode("QQ="), None, "a short final group is malformed");
        assert_eq!(base64_decode("Q==="), None);
        assert_eq!(base64_decode("="), None);
        assert_eq!(base64_decode("QQ==QQ=="), None, "padding must come last");
    }

    /// The store path accepts exactly the three media types the product produces,
    /// and the declared type must match the bytes.
    #[test]
    fn data_url_decoding_is_strict_about_type_and_content() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        let encoded = base64(&png);
        let (bytes, mime) =
            decode_image_data_url(&format!("data:image/png;base64,{encoded}")).expect("png");
        assert_eq!(bytes, png);
        assert_eq!(mime, "image/png");

        for header in ["data:image/jpeg;base64", "data:image/webp;base64"] {
            assert_eq!(
                decode_image_data_url(&format!("{header},{encoded}")).expect("accepted").1,
                header.trim_start_matches("data:").trim_end_matches(";base64"),
                "{header} must be accepted"
            );
        }
        for rejected in [
            "",
            "data:image/gif;base64,R0lGOD",
            "data:image/png,R0lGOD",
            "data:text/plain;base64,QQ==",
            "data:image/png;base64,****",
            "data:image/png;base64,",
        ] {
            assert!(
                decode_image_data_url(rejected).is_err(),
                "{rejected} must be rejected"
            );
        }

        // A payload whose bytes are not an image is refused before anything is
        // written, and by signature rather than by the declared type.
        let gif = b"GIF89a....";
        assert_eq!(
            image_signature(gif, false).unwrap_err(),
            ERROR_IMAGE_UNSUPPORTED
        );
    }

    /// The rejection vocabulary is the contract the frontend maps to localized
    /// copy, so the phrases themselves are pinned here: renaming one without
    /// updating `src/components/SettingsPanel.vue` breaks this test instead of
    /// silently showing a generic message.
    #[test]
    fn rejection_messages_stay_renderable_by_the_frontend() {
        assert_eq!(ERROR_IMAGE_TOO_LARGE, "selected image is too large");
        assert_eq!(ERROR_IMAGE_UNSUPPORTED, "unsupported image");
        assert_eq!(ERROR_IMAGE_UNREADABLE, "selected image could not be read");

        let panel = include_str!("../../src/components/SettingsPanel.vue");
        for (constant, message, phrase) in [
            ("ERROR_IMAGE_TOO_LARGE", ERROR_IMAGE_TOO_LARGE, "too large"),
            ("ERROR_IMAGE_UNSUPPORTED", ERROR_IMAGE_UNSUPPORTED, "unsupported"),
            ("ERROR_IMAGE_UNREADABLE", ERROR_IMAGE_UNREADABLE, "could not be read"),
        ] {
            assert!(
                message.contains(phrase),
                "{constant} must contain the phrase the UI matches on"
            );
            assert!(
                panel.contains(&format!("\"{phrase}\"")),
                "SettingsPanel.vue no longer maps {constant} ({phrase})"
            );
        }
    }

    /// The avatar flow must normalize before storing, store through the dedicated
    /// command, and report failures inside the row that produced them.
    ///
    /// These are source pins for the frontend half of the pipeline, in the same
    /// style as the footer tests in `settings.rs`: the backend contract above and
    /// the UI that consumes it have to move together.
    #[test]
    fn avatar_flow_normalizes_then_stores_and_reports_inline() {
        let panel = include_str!("../../src/components/SettingsPanel.vue");
        let appearance = include_str!("../../src/appearance.ts");

        // The picker is asked not to persist the original file.
        assert!(
            panel.contains("choose_local_asset\", { kind, persist: false }"),
            "the avatar picker must not store the original file"
        );
        // The normalized image is what gets stored.
        assert!(panel.contains("normalizeAvatarImage("));
        assert!(panel.contains("store_managed_asset"));
        // Normalization is a square centre crop at the documented edge, encoded as
        // PNG so alpha survives.
        assert!(appearance.contains("export const AVATAR_EDGE_PX = 256"));
        assert!(appearance.contains("imageOrientation: \"from-image\""));
        assert!(appearance.contains("toDataURL(\"image/png\")"));
        // Errors render next to the control that produced them, not at the panel
        // bottom where the user cannot see them.
        assert!(
            panel.contains("assetError?.kind === 'avatar'"),
            "the avatar row must render its own error"
        );
        assert!(
            panel.contains("assetError?.kind === 'background'"),
            "the background row must render its own error"
        );
        assert!(
            !panel.contains("v-if=\"assetMessage\""),
            "the panel-bottom error message must be gone"
        );

        // Both languages have the copy for every rejection reason.
        let catalog = include_str!("../../src/i18n/catalog.ts");
        for key in [
            "settings.error.imageUnsupported",
            "settings.error.imageTooLarge",
            "settings.error.imageUnreadable",
        ] {
            assert_eq!(
                catalog.matches(&format!("\"{key}\":")).count(),
                2,
                "{key} must exist in English and Simplified Chinese"
            );
        }
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
