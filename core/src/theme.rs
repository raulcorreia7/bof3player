// SPDX-License-Identifier: MIT
//! The window's colors, as semantic tokens that match the terminal player.
//!
//! The terminal player (`cli/tui.rs`) names ANSI colors, which each terminal draws its own way.
//! These tokens use the VS Code "Dark+" terminal values for those names, so the window reads as the
//! same app: cyan accent, green for playing, yellow for warnings, a navy selection and a gray text
//! ladder. The surfaces, hover and pressed shades are dark blue-grays on the selection hue. A test
//! checks that the tokens still match the terminal player's colors. Each color is a 24-bit
//! `0xRRGGBB` value.

/// The window backdrop and text inputs: the selection hue, nearly black.
pub const BG: u32 = 0x0d0f14;
/// Panels and lists.
pub const SURFACE: u32 = 0x12151c;
/// Raised controls and the tracks of bars.
pub const SURFACE_RAISED: u32 = 0x1a1e28;
/// The fill under the pointer.
pub const SURFACE_HOVER: u32 = 0x232937;
/// Panel borders and dividers.
pub const BORDER: u32 = 0x2a3141;
/// Borders that mark a control, such as an input or a button.
pub const BORDER_STRONG: u32 = 0x626d85;
/// Body text and the selected row's text: ANSI white and gray.
pub const TEXT: u32 = 0xe5e5e5;
/// Secondary text and idle icons.
pub const TEXT_MUTED: u32 = 0xa6a6a6;
/// Hints and controls that cannot be used: ANSI dark gray, the terminal player's dim.
pub const TEXT_DISABLED: u32 = 0x666666;
/// The interactive accent: ANSI cyan, the terminal player's accent.
pub const ACCENT: u32 = 0x11a8cd;
/// The accent under the pointer.
pub const ACCENT_HOVER: u32 = 0x3dc3e6;
/// The accent while pressed.
pub const ACCENT_PRESSED: u32 = 0x0e8dad;
/// Text and icons on an accent fill: ANSI black, as on the terminal player's badges.
pub const ON_ACCENT: u32 = 0x000000;
/// The selected row's fill: the terminal player's selection navy.
pub const SELECTION: u32 = 0x1f3470;
/// The keyboard focus outline.
pub const FOCUS_RING: u32 = 0x3dc3e6;
/// The playing track and a healthy level: ANSI green.
pub const SUCCESS: u32 = 0x0dbc79;
/// Warnings, the paused state and a hot level: ANSI yellow.
pub const WARNING: u32 = 0xe5e510;
/// Errors and a clipping level: ANSI bright red, which reads on dark surfaces.
pub const DANGER: u32 = 0xf14c4c;
/// The quiet end of the spectrum.
pub const SPECTRUM_LOW: u32 = 0x0e8dad;
/// The loud end of the spectrum.
pub const SPECTRUM_HIGH: u32 = 0x3dc3e6;
/// The shaded span of a track's loop window on the progress bar: a dim tint of the accent.
pub const LOOP_BAND: u32 = 0x184051;
/// The thin markers at the start and end of the loop window.
pub const LOOP_MARK: u32 = 0x3dc3e6;
/// The overlay behind dialogs: black, drawn translucent.
pub const SCRIM: u32 = 0x000000;

#[cfg(test)]
mod tests {
    use super::*;

    /// The relative luminance of a 24-bit color.
    fn luminance(rgb: u32) -> f64 {
        let channel = |shift: u32| {
            let value = f64::from((rgb >> shift) & 0xff) / 255.0;
            if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    /// The contrast ratio of two colors, from 1 to 21.
    fn contrast(a: u32, b: u32) -> f64 {
        let (high, low) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
        (high + 0.05) / (low + 0.05)
    }

    fn assert_contrast(foreground: u32, background: u32, minimum: f64) {
        let ratio = contrast(foreground, background);
        assert!(ratio >= minimum, "{foreground:06x} on {background:06x} is {ratio:.2}, below {minimum}");
    }

    const SURFACES: [u32; 3] = [BG, SURFACE, SURFACE_RAISED];

    #[test]
    fn text_is_readable_on_every_surface() {
        for background in SURFACES.into_iter().chain([SELECTION, SURFACE_HOVER]) {
            for text in [TEXT, TEXT_MUTED, ACCENT_HOVER, SUCCESS, WARNING] {
                assert_contrast(text, background, 4.5);
            }
        }
        // Errors and the plain accent never sit on a selected or hovered row.
        for background in SURFACES {
            assert_contrast(ACCENT, background, 4.5);
            assert_contrast(DANGER, background, 4.5);
        }
        // Disabled text is exempt from the body limit; it keeps the large-text limit where it sits.
        for background in [BG, SURFACE] {
            assert_contrast(TEXT_DISABLED, background, 3.0);
        }
    }

    #[test]
    fn accent_fills_carry_readable_content() {
        for fill in [ACCENT, ACCENT_HOVER, ACCENT_PRESSED] {
            assert_contrast(ON_ACCENT, fill, 4.5);
        }
    }

    #[test]
    fn borders_icons_and_bars_are_visible() {
        for background in SURFACES {
            for part in [BORDER_STRONG, FOCUS_RING, SPECTRUM_LOW, SPECTRUM_HIGH, SUCCESS] {
                assert_contrast(part, background, 3.0);
            }
        }
        assert_contrast(FOCUS_RING, SELECTION, 3.0);
        // The markers show on the band and on the bar's empty track.
        assert_contrast(LOOP_MARK, LOOP_BAND, 3.0);
        assert_contrast(LOOP_MARK, SURFACE_RAISED, 3.0);
        assert_contrast(ACCENT, LOOP_BAND, 1.5);
        assert_contrast(ACCENT, SURFACE_RAISED, 3.0);
    }

    /// The tokens equal the ANSI colors the terminal player names, in the Dark+ terminal scheme.
    /// `DANGER` is the bright red: the plain red fails the text limit on dark surfaces.
    #[cfg(feature = "cli")]
    #[test]
    fn tokens_match_the_terminal_player() {
        use ratatui::style::Color;
        let rgb = |color: Color| match color {
            Color::Black => 0x000000,
            Color::Red => 0xcd3131,
            Color::Green => 0x0dbc79,
            Color::Yellow => 0xe5e510,
            Color::Cyan => 0x11a8cd,
            Color::Gray | Color::White => 0xe5e5e5,
            Color::DarkGray => 0x666666,
            Color::LightRed => 0xf14c4c,
            Color::Rgb(r, g, b) => u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
            other => panic!("{other:?} is not a terminal player color"),
        };
        // Accent, dim, playing, warning, selection, text and badge text, as `cli/tui.rs` draws them.
        assert_eq!(ACCENT, rgb(Color::Cyan));
        assert_eq!(TEXT_DISABLED, rgb(Color::DarkGray));
        assert_eq!(SUCCESS, rgb(Color::Green));
        assert_eq!(WARNING, rgb(Color::Yellow));
        assert_eq!(SELECTION, rgb(Color::Rgb(31, 52, 112)));
        assert_eq!(TEXT, rgb(Color::Gray));
        assert_eq!(ON_ACCENT, rgb(Color::Black));
        assert_eq!(DANGER, rgb(Color::LightRed));
    }
}
