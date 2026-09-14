//! Pure geometry for the product window.
//!
//! Work-area selection, rect math, and DIP ↔ physical conversion. Every item
//! here is a pure function of its inputs: no window mutation, no persistence,
//! no transition state, no platform calls. The callers in `product_window`
//! own *when* the resulting values are applied to the window or saved into
//! settings; nothing in this module can observe or change that timing.

use super::ORB_SIZE_DIP;
use crate::settings::{ProductSettings, SidebarSide};
use crate::window_mode;
use tauri::PhysicalPosition;

const SNAP_THRESHOLD: i32 = 24;
const MIN_VISIBLE: i32 = 80;
const ORB_MARGIN_DIP: i32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct WorkArea {
    pub(super) identity: Option<String>,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) scale_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WindowRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn normalized_scale_factor(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

pub(super) fn logical_i32_to_physical(value: i32, scale_factor: f64) -> i32 {
    (value as f64 * normalized_scale_factor(scale_factor)).round() as i32
}

pub(super) fn logical_u32_to_physical(value: u32, scale_factor: f64) -> u32 {
    (value as f64 * normalized_scale_factor(scale_factor))
        .round()
        .max(1.0) as u32
}

pub(super) fn physical_i32_to_logical(value: i32, scale_factor: f64) -> i32 {
    (value as f64 / normalized_scale_factor(scale_factor)).round() as i32
}

pub(super) fn physical_u32_to_logical(value: u32, scale_factor: f64) -> u32 {
    (value as f64 / normalized_scale_factor(scale_factor))
        .round()
        .max(1.0) as u32
}

fn logical_bounds_to_physical(
    bounds: window_mode::DesktopBounds,
    scale_factor: f64,
) -> window_mode::DesktopBounds {
    window_mode::DesktopBounds {
        x: logical_i32_to_physical(bounds.x, scale_factor),
        y: logical_i32_to_physical(bounds.y, scale_factor),
        width: logical_u32_to_physical(bounds.width, scale_factor),
        height: logical_u32_to_physical(bounds.height, scale_factor),
    }
}

pub(super) fn physical_bounds_to_logical(
    bounds: window_mode::DesktopBounds,
    scale_factor: f64,
) -> window_mode::DesktopBounds {
    window_mode::DesktopBounds {
        x: physical_i32_to_logical(bounds.x, scale_factor),
        y: physical_i32_to_logical(bounds.y, scale_factor),
        width: physical_u32_to_logical(bounds.width, scale_factor),
        height: physical_u32_to_logical(bounds.height, scale_factor),
    }
}

pub(super) fn orb_rect(
    settings: &ProductSettings,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    let target = select_orb_work_area(settings, areas).unwrap_or_else(|| fallback.clone());
    let scale = normalized_scale_factor(target.scale_factor);
    let size = logical_u32_to_physical(ORB_SIZE_DIP, scale)
        .min(target.width)
        .min(target.height);
    let margin = logical_i32_to_physical(ORB_MARGIN_DIP, scale);
    let requested_x = settings
        .floating_orb_x
        .map(|value| logical_i32_to_physical(value, scale))
        .unwrap_or(target.x + target.width as i32 - size as i32 - margin);
    let requested_y = settings
        .floating_orb_y
        .map(|value| logical_i32_to_physical(value, scale))
        .unwrap_or(target.y + target.height as i32 - size as i32 - margin);
    WindowRect {
        x: requested_x.clamp(target.x, target.x + target.width as i32 - size as i32),
        y: requested_y.clamp(target.y, target.y + target.height as i32 - size as i32),
        width: size,
        height: size,
    }
}

pub(super) fn smart_expanded_rect(
    settings: &ProductSettings,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    let target = select_orb_work_area(settings, areas).unwrap_or_else(|| fallback.clone());
    let orb = orb_rect(settings, areas, fallback);
    let scale = normalized_scale_factor(target.scale_factor);
    let width = logical_u32_to_physical(settings.width.clamp(360, 1100), scale).min(target.width);
    let height =
        logical_u32_to_physical(settings.height.clamp(500, 1200), scale).min(target.height);
    let orb_center_x = orb.x + orb.width as i32 / 2;
    let orb_center_y = orb.y + orb.height as i32 / 2;
    let area_center_x = target.x + target.width as i32 / 2;
    let area_center_y = target.y + target.height as i32 / 2;
    let requested_x = if orb_center_x >= area_center_x {
        orb.x + orb.width as i32 - width as i32
    } else {
        orb.x
    };
    let requested_y = if orb_center_y >= area_center_y {
        orb.y + orb.height as i32 - height as i32
    } else {
        orb.y
    };
    WindowRect {
        x: requested_x.clamp(target.x, target.x + target.width as i32 - width as i32),
        y: requested_y.clamp(target.y, target.y + target.height as i32 - height as i32),
        width,
        height,
    }
}

pub(super) fn desktop_request_from_settings(
    settings: &ProductSettings,
    scale_factor: f64,
) -> window_mode::DesktopWidgetRequest {
    let logical_width = settings.desktop_width.clamp(360, 1100);
    let logical_height = settings.desktop_height.clamp(500, 1200);
    let logical_bounds = match (settings.desktop_x, settings.desktop_y) {
        (Some(x), Some(y)) => format!("{x},{y},{logical_width}x{logical_height} DIP"),
        _ => format!("auto-right,{logical_width}x{logical_height} DIP"),
    };
    let bounds = match (settings.desktop_x, settings.desktop_y) {
        (Some(x), Some(y)) => Some(logical_bounds_to_physical(
            window_mode::DesktopBounds {
                x,
                y,
                width: logical_width,
                height: logical_height,
            },
            scale_factor,
        )),
        _ => None,
    };
    window_mode::DesktopWidgetRequest {
        bounds,
        default_width: logical_u32_to_physical(logical_width, scale_factor),
        default_height: logical_u32_to_physical(logical_height, scale_factor),
        margin: logical_i32_to_physical(32, scale_factor),
        tauri_scale_factor: scale_factor,
        logical_bounds,
    }
}

pub(super) fn migrate_geometry_values_to_dip(settings: &mut ProductSettings, scale_factor: f64) {
    // Before the unit contract was explicit, Floating and Sidebar values came
    // directly from Tauri physical events. Convert those once so a 150%
    // display keeps the same visible rect after switching to DIP.
    settings.x = settings
        .x
        .map(|value| physical_i32_to_logical(value, scale_factor));
    settings.y = settings
        .y
        .map(|value| physical_i32_to_logical(value, scale_factor));
    settings.width = physical_u32_to_logical(settings.width, scale_factor);
    settings.height = physical_u32_to_logical(settings.height, scale_factor);
    settings.sidebar_width = physical_u32_to_logical(settings.sidebar_width, scale_factor);
    // Desktop fields were introduced as logical product defaults but were
    // accidentally passed straight to SetWindowPos. Their numeric values
    // already express the intended DIP size and must not be divided here.
    settings.geometry_units_version = 1;
}

pub(super) fn select_work_area(settings: &ProductSettings, areas: &[WorkArea]) -> Option<WorkArea> {
    // Saved monitor identity wins over saved coordinates: a monitor that was
    // unplugged and reattached, or a layout that changed the origin, moves every
    // coordinate while the identity stays stable. Coordinates are the fallback
    // for a document written before the identity was recorded.
    if let Some(identity) = settings.monitor_identity.as_deref() {
        if let Some(area) = areas
            .iter()
            .find(|area| area.identity.as_deref() == Some(identity))
        {
            return Some(area.clone());
        }
    }
    if let (Some(x), Some(y)) = (settings.x, settings.y) {
        areas
            .iter()
            .find(|area| {
                point_in_area(
                    logical_i32_to_physical(x, area.scale_factor),
                    logical_i32_to_physical(y, area.scale_factor),
                    area,
                )
            })
            .cloned()
    } else {
        None
    }
}

pub(super) fn select_orb_work_area(
    settings: &ProductSettings,
    areas: &[WorkArea],
) -> Option<WorkArea> {
    if let Some(identity) = settings.floating_orb_monitor_identity.as_deref() {
        if let Some(area) = areas
            .iter()
            .find(|area| area.identity.as_deref() == Some(identity))
        {
            return Some(area.clone());
        }
    }
    if let (Some(x), Some(y)) = (settings.floating_orb_x, settings.floating_orb_y) {
        areas
            .iter()
            .find(|area| {
                point_in_area(
                    logical_i32_to_physical(x, area.scale_factor),
                    logical_i32_to_physical(y, area.scale_factor),
                    area,
                )
            })
            .cloned()
    } else {
        None
    }
}

fn point_in_area(x: i32, y: i32, area: &WorkArea) -> bool {
    x >= area.x && x < area.x + area.width as i32 && y >= area.y && y < area.y + area.height as i32
}

pub(super) fn validate_window_rect(
    saved: WindowRect,
    areas: &[WorkArea],
    fallback: &WorkArea,
) -> WindowRect {
    // Require more than a one-pixel intersection: disconnected displays often
    // leave a technically intersecting resize border that users cannot grab.
    // An 80x80 area keeps enough title/content surface available for recovery.
    if areas
        .iter()
        .any(|area| visible_intersection(saved, area) >= MIN_VISIBLE * MIN_VISIBLE)
    {
        return saved;
    }
    let width = saved.width.min(fallback.width.saturating_sub(32)).max(360);
    let height = saved
        .height
        .min(fallback.height.saturating_sub(32))
        .max(500);
    WindowRect {
        x: fallback.x + 32,
        y: fallback.y + 32,
        width,
        height,
    }
}

fn visible_intersection(window: WindowRect, area: &WorkArea) -> i32 {
    let left = window.x.max(area.x);
    let top = window.y.max(area.y);
    let right = (window.x + window.width as i32).min(area.x + area.width as i32);
    let bottom = (window.y + window.height as i32).min(area.y + area.height as i32);
    (right - left).max(0) * (bottom - top).max(0)
}

pub(super) fn snap_side(
    position: PhysicalPosition<i32>,
    window_width: u32,
    areas: &[WorkArea],
) -> Option<SidebarSide> {
    areas.iter().find_map(|area| {
        if position.y < area.y - SNAP_THRESHOLD
            || position.y > area.y + area.height as i32 + SNAP_THRESHOLD
        {
            return None;
        }
        if (position.x - area.x).abs() <= SNAP_THRESHOLD {
            Some(SidebarSide::Left)
        } else if (position.x + window_width as i32 - (area.x + area.width as i32)).abs()
            <= SNAP_THRESHOLD
        {
            Some(SidebarSide::Right)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        desktop_request_from_settings, logical_bounds_to_physical, migrate_geometry_values_to_dip,
        orb_rect, physical_bounds_to_logical, select_work_area, smart_expanded_rect, snap_side,
        validate_window_rect, WindowRect, WorkArea, ORB_SIZE_DIP,
    };
    use crate::settings::{ProductSettings, SidebarSide};
    use crate::window_mode::DesktopBounds;
    use tauri::PhysicalPosition;

    const PRIMARY: WorkArea = WorkArea {
        identity: None,
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
        scale_factor: 1.0,
    };

    #[test]
    fn preserves_visible_saved_rect() {
        let saved = WindowRect {
            x: 120,
            y: 80,
            width: 620,
            height: 720,
        };
        assert_eq!(validate_window_rect(saved, &[PRIMARY], &PRIMARY), saved);
    }

    #[test]
    fn recovers_fully_offscreen_rect_to_primary_work_area() {
        let saved = WindowRect {
            x: 5000,
            y: 5000,
            width: 620,
            height: 720,
        };
        assert_eq!(
            validate_window_rect(saved, &[PRIMARY], &PRIMARY),
            WindowRect {
                x: 32,
                y: 32,
                width: 620,
                height: 720
            }
        );
    }

    #[test]
    fn detects_left_and_right_edge_snap() {
        assert_eq!(
            snap_side(PhysicalPosition::new(8, 200), 620, &[PRIMARY]),
            Some(SidebarSide::Left)
        );
        assert_eq!(
            snap_side(PhysicalPosition::new(1292, 200), 620, &[PRIMARY]),
            Some(SidebarSide::Right)
        );
    }

    #[test]
    fn monitor_identity_wins_over_stale_coordinates() {
        let secondary = WorkArea {
            identity: Some("DISPLAY-2".into()),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1040,
            scale_factor: 1.0,
        };
        let settings = ProductSettings {
            monitor_identity: Some("DISPLAY-2".into()),
            x: Some(200),
            y: Some(200),
            ..ProductSettings::default()
        };
        assert_eq!(
            select_work_area(&settings, &[PRIMARY, secondary.clone()]),
            Some(secondary)
        );
    }

    #[test]
    fn desktop_geometry_is_independent_from_floating_geometry() {
        let settings = ProductSettings {
            x: Some(120),
            y: Some(80),
            width: 620,
            height: 720,
            desktop_x: Some(1400),
            desktop_y: Some(32),
            desktop_width: 420,
            desktop_height: 700,
            ..ProductSettings::default()
        };
        assert_eq!(
            desktop_request_from_settings(&settings, 1.5).bounds,
            Some(DesktopBounds {
                x: 2100,
                y: 48,
                width: 630,
                height: 1050,
            })
        );
        assert_eq!((settings.x, settings.y), (Some(120), Some(80)));
        assert_eq!((settings.width, settings.height), (620, 720));
    }

    #[test]
    fn desktop_dip_conversion_covers_common_windows_scaling_levels() {
        let logical = DesktopBounds {
            x: 40,
            y: 32,
            width: 420,
            height: 700,
        };
        for (scale, expected) in [
            (1.0, (420, 700)),
            (1.25, (525, 875)),
            (1.5, (630, 1050)),
            (2.0, (840, 1400)),
        ] {
            let physical = logical_bounds_to_physical(logical, scale);
            assert_eq!((physical.width, physical.height), expected);
            assert_eq!(physical_bounds_to_logical(physical, scale), logical);
        }
    }

    #[test]
    fn orb_size_is_exactly_56_dip_at_common_windows_scaling_levels() {
        for (scale, expected) in [(1.0, 56), (1.25, 70), (1.5, 84), (2.0, 112)] {
            let area = WorkArea {
                scale_factor: scale,
                ..PRIMARY
            };
            let rect = orb_rect(
                &ProductSettings::default(),
                std::slice::from_ref(&area),
                &area,
            );
            assert_eq!(ORB_SIZE_DIP, 56);
            assert_eq!((rect.width, rect.height), (expected, expected));
        }
    }

    #[test]
    fn smart_expansion_keeps_the_orb_corner_anchor_and_stays_visible() {
        let cases = [
            ((40, 40), (40, 40)),
            ((1824, 40), (1260, 40)),
            ((40, 944), (40, 280)),
            ((1824, 944), (1260, 280)),
        ];
        for ((orb_x, orb_y), expected_origin) in cases {
            let settings = ProductSettings {
                floating_orb_x: Some(orb_x),
                floating_orb_y: Some(orb_y),
                ..ProductSettings::default()
            };
            let expanded = smart_expanded_rect(&settings, &[PRIMARY], &PRIMARY);
            assert_eq!((expanded.x, expanded.y), expected_origin);
            assert_eq!((expanded.width, expanded.height), (620, 720));
            assert!(expanded.x >= PRIMARY.x && expanded.y >= PRIMARY.y);
            assert!(expanded.x + expanded.width as i32 <= PRIMARY.x + PRIMARY.width as i32);
            assert!(expanded.y + expanded.height as i32 <= PRIMARY.y + PRIMARY.height as i32);
        }
    }

    #[test]
    fn legacy_physical_geometry_migrates_without_shrinking_desktop_dip_defaults() {
        let mut settings = ProductSettings {
            geometry_units_version: 0,
            x: Some(711),
            y: Some(422),
            width: 642,
            height: 733,
            sidebar_width: 570,
            desktop_x: Some(1400),
            desktop_y: Some(32),
            desktop_width: 420,
            desktop_height: 700,
            ..ProductSettings::default()
        };
        migrate_geometry_values_to_dip(&mut settings, 1.5);
        assert_eq!(settings.geometry_units_version, 1);
        assert_eq!((settings.x, settings.y), (Some(474), Some(281)));
        assert_eq!((settings.width, settings.height), (428, 489));
        assert_eq!(settings.sidebar_width, 380);
        assert_eq!(
            (settings.desktop_width, settings.desktop_height),
            (420, 700)
        );
        assert_eq!(
            (settings.desktop_x, settings.desktop_y),
            (Some(1400), Some(32))
        );
    }
}
