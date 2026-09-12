//! Product UI language resolution.
//!
//! v1 supports exactly three user choices — System, English, and Simplified
//! Chinese — and deliberately does not implement a general locale framework:
//! no region fallback chains, no traditional Chinese, no per-region variants.
//!
//! The persisted choice lives in `ProductSettings.language`; the concrete
//! language is derived from that choice plus the operating system locale. The
//! same derivation is used for the native menus here and, through
//! `ProductViewState.systemLocale`, by the frontend, so both surfaces agree.

use serde::{Deserialize, Serialize};

/// The user's persisted language preference.
///
/// The serialized names are the BCP-47-shaped tags the frontend persists and
/// renders (`system`, `en`, `zh-Hans`), so the settings document has one spelling
/// for each language instead of a Rust-side and a JavaScript-side variant.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub enum Language {
    /// Follow the operating system locale. This is the v1 default.
    #[default]
    #[serde(rename = "system")]
    System,
    /// Force English regardless of the system locale.
    #[serde(rename = "en")]
    English,
    /// Force Simplified Chinese regardless of the system locale.
    #[serde(rename = "zh-Hans")]
    SimplifiedChinese,
}

/// A concrete language the UI can be rendered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveLanguage {
    English,
    SimplifiedChinese,
}

impl Language {
    /// Resolves the concrete UI language for this preference.
    pub fn resolve(self, system_locale: &str) -> ActiveLanguage {
        match self {
            Self::English => ActiveLanguage::English,
            Self::SimplifiedChinese => ActiveLanguage::SimplifiedChinese,
            Self::System => {
                if is_simplified_chinese_locale(system_locale) {
                    ActiveLanguage::SimplifiedChinese
                } else {
                    ActiveLanguage::English
                }
            }
        }
    }
}

/// True when a BCP-47 tag names a Simplified Chinese UI.
///
/// `zh-CN` and `zh-SG` are the two Simplified Chinese regions; every other
/// `zh-Hans*` script tag is Simplified Chinese by declaration. Script subtags
/// win over a conflicting region (`zh-Hans-TW` is still Simplified), and
/// Traditional Chinese (`zh-TW`, `zh-HK`, `zh-Hant`) intentionally falls back
/// to English because v1 does not ship a Traditional Chinese catalog.
pub fn is_simplified_chinese_locale(locale: &str) -> bool {
    let trimmed = locale.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Accept POSIX forms such as `zh_CN.UTF-8` or `zh_CN@euro` as well as
    // BCP-47 forms such as `zh-Hans-CN`.
    let trimmed = trimmed.split(['.', '@']).next().unwrap_or(trimmed);
    let primary = trimmed
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if primary != "zh" {
        return false;
    }
    let subtags: Vec<String> = trimmed
        .split(['-', '_'])
        .skip(1)
        .map(|subtag| subtag.to_ascii_lowercase())
        .collect();
    if subtags.iter().any(|subtag| subtag == "hant") {
        return false;
    }
    if subtags.iter().any(|subtag| subtag == "hans") {
        return true;
    }
    subtags
        .iter()
        .any(|subtag| subtag == "cn" || subtag == "sg")
}

/// Native menu labels.
///
/// The native tray/context menu is the one product surface that is not built
/// from the frontend catalog, so its handful of fixed labels live here. They
/// mirror the frontend keys `menu.*`, and the same Simplified Chinese wording is
/// used in both places.
pub struct NativeLabels {
    pub window_mode: &'static str,
    pub sidebar: &'static str,
    pub floating: &'static str,
    pub desktop_experimental: &'static str,
    pub side: &'static str,
    pub left: &'static str,
    pub right: &'static str,
    pub lock_position: &'static str,
    pub always_on_top: &'static str,
    pub expand_floating: &'static str,
    pub collapse_floating: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
}

const ENGLISH_LABELS: NativeLabels = NativeLabels {
    window_mode: "Window mode",
    sidebar: "Sidebar",
    floating: "Floating",
    desktop_experimental: "Desktop — Experimental",
    side: "Side",
    left: "Left",
    right: "Right",
    lock_position: "Lock position",
    always_on_top: "Always on top",
    expand_floating: "Expand Floating",
    collapse_floating: "Collapse to Avatar Orb",
    settings: "Settings",
    quit: "Quit",
};

const SIMPLIFIED_CHINESE_LABELS: NativeLabels = NativeLabels {
    window_mode: "窗口模式",
    sidebar: "侧边栏",
    floating: "悬浮",
    desktop_experimental: "桌面 — 实验性",
    side: "停靠方向",
    left: "左侧",
    right: "右侧",
    lock_position: "锁定位置",
    always_on_top: "始终置顶",
    expand_floating: "展开悬浮窗",
    collapse_floating: "收起到头像球",
    settings: "设置",
    quit: "退出",
};

impl ActiveLanguage {
    /// Fixed labels for the native menus.
    pub fn labels(self) -> &'static NativeLabels {
        match self {
            Self::English => &ENGLISH_LABELS,
            Self::SimplifiedChinese => &SIMPLIFIED_CHINESE_LABELS,
        }
    }
}

/// The operating system's preferred UI locale, read from the platform once per
/// process and cached.
///
/// An OS display-language change requires an application restart to be picked
/// up, which matches how the rest of the product treats startup-time platform
/// facts. Only the language subtag matters for v1, so an unreadable or
/// unexpected value simply yields an empty string and resolves to English
/// instead of failing startup.
pub fn system_locale() -> &'static str {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| platform_locale().unwrap_or_default())
}

/// The locale tag handed to the frontend for `System` resolution.
///
/// The frontend applies the same Simplified Chinese rule as [`Language::resolve`],
/// so exposing the raw system locale keeps a single source of truth instead of
/// duplicating locale detection in JavaScript. An empty string means the system
/// locale could not be read and resolves to English on both sides.
pub fn system_locale_hint() -> &'static str {
    system_locale()
}

#[cfg(target_os = "windows")]
fn platform_locale() -> Option<String> {
    windows_locale().or_else(posix_locale)
}

#[cfg(not(target_os = "windows"))]
fn platform_locale() -> Option<String> {
    posix_locale()
}

/// `LC_ALL`, `LC_MESSAGES`, then `LANG`, in POSIX precedence order.
fn posix_locale() -> Option<String> {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(name) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value == "C" || value == "POSIX" {
            continue;
        }
        // `en_US.UTF-8@euro` → `en_US`
        let value = value.split(['.', '@']).next().unwrap_or(value);
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn windows_locale() -> Option<String> {
    use std::ffi::c_void;

    type Hkey = isize;
    const HKEY_CURRENT_USER: Hkey = 0x8000_0001_u32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const ERROR_MORE_DATA: i32 = 234;
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
    let read = |sub_key: &str, name: &str| -> Option<String> {
        let key = wide(sub_key);
        let name = wide(name);
        let mut buffer = vec![0_u16; 1_024];
        let mut bytes = (buffer.len() * std::mem::size_of::<u16>()) as u32;
        // SAFETY: the output buffer is writable for `bytes` bytes; both
        // registry strings are NUL-terminated. A larger-than-buffer value is
        // rejected below rather than read.
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
        if status == ERROR_MORE_DATA {
            return None;
        }
        if status != 0 {
            return None;
        }
        // `PreferredUILanguages` is a REG_MULTI_SZ list; the first entry is the
        // UI language. For a single REG_SZ the first NUL ends the value.
        let length = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        let value = String::from_utf16_lossy(&buffer[..length]);
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    // `LocaleName` is the user's locale; `PreferredUILanguages` is the display
    // language list. Both are BCP-47, and the locale is the closer match for
    // the "System" UI language choice.
    read(r"Control Panel\International", "LocaleName")
        .or_else(|| read(r"Control Panel\Desktop", "PreferredUILanguages"))
        .or_else(posix_locale)
}

#[cfg(test)]
mod tests {
    use super::{is_simplified_chinese_locale, ActiveLanguage, Language};

    #[test]
    fn system_language_follows_simplified_chinese_locales() {
        for locale in [
            "zh-CN",
            "zh_CN",
            "zh-CN.UTF-8",
            "zh-SG",
            "zh-Hans",
            "zh-Hans-CN",
            "zh-Hans-TW",
            "ZH-cn",
        ] {
            assert_eq!(
                Language::System.resolve(locale),
                ActiveLanguage::SimplifiedChinese,
                "{locale} should resolve to Simplified Chinese"
            );
        }
    }

    #[test]
    fn traditional_chinese_falls_back_to_english_in_v1() {
        for locale in ["zh-TW", "zh-HK", "zh-Hant", "zh-Hant-HK"] {
            assert_eq!(
                Language::System.resolve(locale),
                ActiveLanguage::English,
                "{locale} has no v1 catalog and must fall back to English"
            );
        }
    }

    #[test]
    fn non_chinese_locales_resolve_to_english() {
        for locale in ["en-US", "ja-JP", "de-DE", "fr", "", "  "] {
            assert_eq!(Language::System.resolve(locale), ActiveLanguage::English);
        }
    }

    #[test]
    fn explicit_choice_overrides_the_system_locale() {
        assert_eq!(
            Language::English.resolve("zh-CN"),
            ActiveLanguage::English
        );
        assert_eq!(
            Language::SimplifiedChinese.resolve("en-US"),
            ActiveLanguage::SimplifiedChinese
        );
        // The default preference is System.
        assert_eq!(Language::default(), Language::System);
    }

    #[test]
    fn locale_helper_ignores_unknown_languages() {
        assert!(!is_simplified_chinese_locale("zhx-CN"));
        assert!(!is_simplified_chinese_locale("en-US"));
    }

    /// The persisted names are part of the settings contract with the frontend,
    /// which sends and reads `system` / `en` / `zh-Hans`.
    #[test]
    fn language_serializes_with_the_frontend_tags() {
        for (language, tag) in [
            (Language::System, "\"system\""),
            (Language::English, "\"en\""),
            (Language::SimplifiedChinese, "\"zh-Hans\""),
        ] {
            assert_eq!(serde_json::to_string(&language).expect("serialize"), tag);
            assert_eq!(
                serde_json::from_str::<Language>(tag).expect("deserialize"),
                language
            );
        }
        assert!(serde_json::from_str::<Language>("\"english\"").is_err());
    }
}
