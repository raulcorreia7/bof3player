// SPDX-License-Identifier: MIT
//! The window's icons: small SVG shapes compiled into the binary, so they look the same on every
//! platform. gpui tints them with the text color.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString, Styled, Svg, px, svg};

const FILL: &str = r##"fill="#fff""##;
const STROKE: &str = r##"fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round""##;

/// The SVG source of an icon, by name.
fn source(name: &str) -> Option<String> {
    let (attrs, shapes) = match name {
        "play" => (FILL, r#"<path d="M7 4.5v15l12.5-7.5z"/>"#),
        "pause" => (
            FILL,
            r#"<rect x="6" y="4.5" width="4.2" height="15" rx="1"/><rect x="13.8" y="4.5" width="4.2" height="15" rx="1"/>"#,
        ),
        "stop" => (FILL, r#"<rect x="5.5" y="5.5" width="13" height="13" rx="2"/>"#),
        "prev" => (FILL, r#"<rect x="4.5" y="5" width="2.6" height="14" rx="1"/><path d="M19.5 5v14L9 12z"/>"#),
        "next" => (FILL, r#"<rect x="16.9" y="5" width="2.6" height="14" rx="1"/><path d="M4.5 5v14L15 12z"/>"#),
        "loop" => {
            (STROKE, r#"<path d="M17 3.5l3 3-3 3M20 6.5H8a4 4 0 0 0-4 4M7 20.5l-3-3 3-3M4 17.5h12a4 4 0 0 0 4-4"/>"#)
        }
        "queue" => (STROKE, r#"<path d="M4 6h11M4 12h11M4 18h6M16 15v6l5-3z"/>"#),
        "volume-off" => (STROKE, r#"<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5zM16 9.5l5 5M21 9.5l-5 5"/>"#),
        "volume" => {
            (STROKE, r#"<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5zM15.5 9a4 4 0 0 1 0 6M18 6.5a8 8 0 0 1 0 11"/>"#)
        }
        "search" => (STROKE, r#"<circle cx="10.5" cy="10.5" r="6"/><path d="M15 15l5 5"/>"#),
        "folder" => (
            STROKE,
            r#"<path d="M3.5 6.5a1.5 1.5 0 0 1 1.5-1.5h4l2 2.5h8a1.5 1.5 0 0 1 1.5 1.5v8.5a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5z"/>"#,
        ),
        "export" => (
            STROKE,
            r#"<path d="M14 4h6v6M20 4l-9 9M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 5 18.5v-10A1.5 1.5 0 0 1 6.5 7H10"/>"#,
        ),
        "filter" => (STROKE, r#"<path d="M4 6h16M7 12h10M10 18h4"/>"#),
        "music" => (
            STROKE,
            r#"<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>"#,
        ),
        "wave" => (STROKE, r#"<path d="M3 12h2.5l2-6 3 12 3-9 2 3H21"/>"#),
        "disc" => (STROKE, r#"<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="2"/>"#),
        "box" => (STROKE, r#"<path d="M4 8l8-4 8 4v8l-8 4-8-4zM4 8l8 4 8-4M12 12v8"/>"#),
        "chevron-down" => (STROKE, r#"<path d="M6.5 9.5L12 15l5.5-5.5"/>"#),
        "chevron-right" => (STROKE, r#"<path d="M9.5 6.5L15 12l-5.5 5.5"/>"#),
        "help" => (
            STROKE,
            r#"<circle cx="12" cy="12" r="8.5"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 .9-1 1.7M12 17h.01"/>"#,
        ),
        "alert" => (STROKE, r#"<path d="M12 4l9 16H3zM12 10v4M12 17h.01"/>"#),
        _ => return None,
    };
    Some(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {attrs}>{shapes}</svg>"#))
}

/// Serves the icons to gpui.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let name = path.strip_suffix(".svg").unwrap_or(path);
        Ok(source(name).map(|svg| Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// An icon of `size` pixels, in the current text color.
pub fn icon(name: &'static str, size: f32) -> Svg {
    svg().path(format!("{name}.svg")).size(px(size)).flex_none()
}
