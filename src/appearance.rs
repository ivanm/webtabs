//! Tab bar look shared by the Linux and Windows front ends: geometry for an
//! icon size, the tint colours, and the brand colours. Mirrors
//! `macos/TabBar.swift` (`BarMetrics`, `Tint`, `webtabsAccent`, `webtabsBadge`)
//! so all three bars match.

use crate::config::TINTS;

/// Active-tab marker (the orange square in the app icon), light and dark.
pub const ACCENT_LIGHT: u32 = 0xF0520F;
pub const ACCENT_DARK: u32 = 0xFF6A2B;
/// Unread bubble; white bold text on it passes WCAG AA.
pub const BADGE: u32 = 0xD93036;

/// Tab bar geometry, in device-independent pixels, for an icon size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarMetrics {
    pub icon: i32,
    /// Bar height: 40 for icons up to 24, 52 above.
    pub height: i32,
    /// The tile (icon + 10, square), relative to the tab's own box.
    pub tile_x: i32,
    pub tile_y: i32,
    pub tile: i32,
    /// Tile corner radius (a quarter of the tile).
    pub radius: i32,
    /// Space between tiles.
    pub gap: i32,
}

impl BarMetrics {
    pub fn new(icon_size: u32) -> BarMetrics {
        let icon = icon_size as i32;
        let height = if icon > 24 { 52 } else { 40 };
        let tile = icon + 10;
        BarMetrics {
            icon,
            height,
            tile_x: 0,
            // Room under the tile for the 3 px marker (floor, as on the Mac).
            tile_y: (height - tile - 4).div_euclid(2) + 1,
            tile,
            radius: (tile as f64 / 4.0).round() as i32,
            gap: 8,
        }
    }

    pub fn tall(&self) -> bool {
        self.height > 40
    }

    /// The active marker: 14×3, centred, 1 px under the tile.
    pub fn marker(&self) -> (i32, i32, i32, i32) {
        (
            self.tile_x + self.tile / 2 - 7,
            self.tile_y + self.tile + 1,
            14,
            3,
        )
    }
}

/// A resolved `tint`: what to paint the bar with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    /// A preset: separate colours for light and dark themes.
    Preset { light: u32, dark: u32 },
    /// A custom `#rrggbb`, the same in both themes.
    Custom(u32),
}

const PRESETS: &[(&str, u32, u32)] = &[
    ("graphite", 0xE4E5E8, 0x2B2D31),
    ("sand", 0xEFE7DA, 0x332E27),
    ("sage", 0xE1EADF, 0x273029),
    ("sky", 0xDDE8F3, 0x24303C),
    ("lavender", 0xE7E2F2, 0x2E2A38),
    ("rose", 0xF2E1E4, 0x382A2D),
];

impl Tint {
    /// `None` for no tint (the theme's own bar colour) or an unknown value.
    pub fn parse(tint: Option<&str>) -> Option<Tint> {
        let tint = tint?;
        if let Some(&(_, light, dark)) = PRESETS.iter().find(|(n, _, _)| *n == tint) {
            return Some(Tint::Preset { light, dark });
        }
        let hex = tint.strip_prefix('#').filter(|h| h.len() == 6)?;
        u32::from_str_radix(hex, 16).ok().map(Tint::Custom)
    }

    /// The bar colour in the given theme.
    pub fn color(&self, dark_theme: bool) -> u32 {
        match *self {
            Tint::Preset { light, dark } => {
                if dark_theme {
                    dark
                } else {
                    light
                }
            }
            Tint::Custom(rgb) => rgb,
        }
    }

    /// Whether text and glyphs on the bar should be light: presets follow the
    /// theme, a custom colour decides by its own luminance.
    pub fn light_text(&self, dark_theme: bool) -> bool {
        match *self {
            Tint::Preset { .. } => dark_theme,
            Tint::Custom(rgb) => is_dark(rgb),
        }
    }
}

/// Relative luminance under 0.5 (sRGB components, Rec. 709 weights), the
/// Mac's rule for picking light text on a custom colour.
pub fn is_dark(rgb: u32) -> bool {
    let c = |shift: u32| ((rgb >> shift) & 0xff) as f64 / 255.0;
    0.2126 * c(16) + 0.7152 * c(8) + 0.0722 * c(0) < 0.5
}

/// The preset names in display order, for swatches.
pub fn tint_names() -> &'static [&'static str] {
    TINTS
}

/// "Sky", "Custom #aabbcc" or the default's label.
pub fn tint_label(tint: Option<&str>, default: &str) -> String {
    match tint {
        None => default.to_string(),
        Some(t) if t.starts_with('#') => format!("Custom {t}"),
        Some(t) => {
            let mut c = t.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_match_the_mac() {
        let m = BarMetrics::new(22);
        assert_eq!((m.height, m.tile, m.tile_y, m.radius), (40, 32, 3, 8));
        assert_eq!(m.marker(), (9, 36, 14, 3));
        let m = BarMetrics::new(24);
        assert_eq!((m.height, m.tile, m.tile_y), (40, 34, 2));
        let m = BarMetrics::new(30);
        assert!(m.tall());
        assert_eq!((m.height, m.tile, m.tile_y, m.radius), (52, 40, 5, 10));
        let m = BarMetrics::new(16);
        assert_eq!((m.tile, m.tile_y, m.radius), (26, 6, 7));
    }

    #[test]
    fn every_shared_tint_has_colours() {
        for name in TINTS {
            assert!(
                matches!(Tint::parse(Some(name)), Some(Tint::Preset { .. })),
                "{name}"
            );
        }
        assert_eq!(Tint::parse(Some("sky")).unwrap().color(true), 0x24303C);
        assert_eq!(Tint::parse(Some("#aabbcc")), Some(Tint::Custom(0xaabbcc)));
        assert_eq!(Tint::parse(Some("nope")), None);
        assert_eq!(Tint::parse(None), None);
    }

    #[test]
    fn text_colour_follows_luminance_for_custom_tints() {
        assert!(Tint::Custom(0x202020).light_text(false));
        assert!(!Tint::Custom(0xf4e3d0).light_text(true));
        let sage = Tint::parse(Some("sage")).unwrap();
        assert!(sage.light_text(true) && !sage.light_text(false));
    }

    #[test]
    fn labels() {
        assert_eq!(tint_label(Some("sky"), "Default"), "Sky");
        assert_eq!(tint_label(Some("#aabbcc"), "Default"), "Custom #aabbcc");
        assert_eq!(tint_label(None, "Default"), "Default");
    }
}
