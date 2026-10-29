// SPDX-License-Identifier: MIT
//! The small controls the window draws itself: panels, buttons, a text field, a slider, level
//! meters and a spectrum.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Background, Bounds, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    SharedString, Stateful, canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, relative,
    rgb, size,
};

use super::icons::icon;
use crate::theme;

/// The corner radius of controls, rows and fields.
pub const RADIUS: f32 = 6.0;
/// The corner radius of panels and dialogs.
pub const RADIUS_LARGE: f32 = 10.0;

/// The window's colors, from [`crate::theme`].
pub struct Palette {
    pub bg: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub disabled: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    pub selection: Hsla,
    pub loop_band: Hsla,
    pub loop_mark: Hsla,
    pub loop_band_played: Hsla,
    pub loop_mark_played: Hsla,
    pub on_accent: Hsla,
    pub focus_ring: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub scrim: Hsla,
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

impl Palette {
    pub fn get() -> Self {
        Self {
            bg: color(theme::BG),
            surface: color(theme::SURFACE),
            raised: color(theme::SURFACE_RAISED),
            hover: color(theme::SURFACE_HOVER),
            border: color(theme::BORDER),
            border_strong: color(theme::BORDER_STRONG),
            text: color(theme::TEXT),
            muted: color(theme::TEXT_MUTED),
            disabled: color(theme::TEXT_DISABLED),
            accent: color(theme::ACCENT),
            accent_hover: color(theme::ACCENT_HOVER),
            accent_pressed: color(theme::ACCENT_PRESSED),
            selection: color(theme::SELECTION),
            loop_band: color(theme::LOOP_BAND),
            loop_mark: color(theme::LOOP_MARK),
            loop_band_played: color(theme::LOOP_BAND_PLAYED),
            loop_mark_played: color(theme::LOOP_MARK_PLAYED),
            on_accent: color(theme::ON_ACCENT),
            focus_ring: color(theme::FOCUS_RING),
            success: color(theme::SUCCESS),
            danger: color(theme::DANGER),
            warning: color(theme::WARNING),
            scrim: Hsla { a: 0.6, ..color(theme::SCRIM) },
        }
    }

    /// The spectrum's gradient: the loud end at the top of a bar, the quiet end at the bottom.
    pub fn spectrum(angle: f32) -> Background {
        linear_gradient(
            angle,
            linear_color_stop(rgb(theme::SPECTRUM_HIGH), 0.0),
            linear_color_stop(rgb(theme::SPECTRUM_LOW), 1.0),
        )
    }
}

/// A panel: a raised surface with a 1 px border.
pub fn panel() -> Div {
    let palette = Palette::get();
    div().bg(palette.surface).border_1().border_color(palette.border).rounded(px(RADIUS_LARGE))
}

/// A clickable button with an icon, a label or both. The caller adds `on_click`.
pub fn button(id: &'static str, icon_name: Option<&'static str>, label: &str, enabled: bool) -> Stateful<Div> {
    let palette = Palette::get();
    let color = if enabled { palette.text } else { palette.disabled };
    let mut base = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(32.0))
        .min_w(px(32.0))
        .px_4()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(palette.border_strong)
        .bg(palette.raised)
        .text_color(color)
        .font_weight(FontWeight::MEDIUM);
    if let Some(name) = icon_name {
        base = base.child(icon(name, 16.0).text_color(color));
    }
    if !label.is_empty() {
        base = base.child(label.to_string());
    }
    if enabled {
        base.cursor_pointer().hover(|style| style.bg(palette.hover)).active(|style| style.bg(palette.border))
    } else {
        base.border_color(palette.border)
    }
}

/// A round transport button of `diameter` pixels with an icon. `primary` is the accent-colored
/// play button, the only filled accent control. The caller adds `on_click`.
pub fn transport(
    id: &'static str,
    icon_name: &'static str,
    diameter: f32,
    enabled: bool,
    primary: bool,
) -> Stateful<Div> {
    let palette = Palette::get();
    let color = match (enabled, primary) {
        (false, _) => palette.disabled,
        (true, true) => palette.on_accent,
        (true, false) => palette.text,
    };
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(diameter))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if primary { palette.accent } else { palette.border })
        .bg(if primary { palette.accent } else { palette.raised })
        .child(icon(icon_name, diameter * 0.42).text_color(color));
    match (enabled, primary) {
        (false, _) => base,
        (true, true) => base
            .cursor_pointer()
            .hover(|style| style.bg(palette.accent_hover).border_color(palette.accent_hover))
            .active(|style| style.bg(palette.accent_pressed).border_color(palette.accent_pressed)),
        (true, false) => base
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).border_color(palette.border_strong))
            .active(|style| style.bg(palette.border)),
    }
}

/// A switch with an icon and a label. The caller adds `on_click`.
pub fn chip(id: &'static str, icon_name: &'static str, text: &str, on: bool) -> Stateful<Div> {
    let palette = Palette::get();
    let color = if on { palette.accent_hover } else { palette.muted };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(28.0))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(if on { palette.accent } else { palette.border_strong })
        .bg(if on { palette.selection } else { palette.bg })
        .text_color(color)
        .cursor_pointer()
        .hover(|style| style.bg(if on { palette.selection } else { palette.hover }))
        .child(icon(icon_name, 14.0).text_color(color))
        .child(text.to_string())
}

/// A one-line text box. The caller routes the keys; `|` marks the end of the text while active.
pub fn field(id: &'static str, text: &str, hint: &'static str, active: bool) -> Stateful<Div> {
    let palette = Palette::get();
    let shown: SharedString = match (text.is_empty(), active) {
        (true, true) => "|".into(),
        (true, false) => hint.into(),
        (false, true) => format!("{text}|").into(),
        (false, false) => text.to_string().into(),
    };
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(RADIUS))
        .bg(palette.bg)
        .border_1()
        .border_color(if active { palette.focus_ring } else { palette.border_strong })
        .text_color(if text.is_empty() && !active { palette.disabled } else { palette.text })
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_text()
        .child(shown)
}

/// A horizontal bar that sets a value from `0.0` to `1.0` on click and drag. `bounds` receives the
/// bar's last painted area, which maps the pointer to the value. `marks` are `(start, end)`
/// fractions for the loop window: a band under the fill, a tint over the played part of it, and
/// markers above everything.
pub fn slider<T: 'static>(
    id: &'static str,
    fraction: f32,
    mark: Option<(f32, f32)>,
    bounds: &Rc<Cell<Bounds<Pixels>>>,
    cx: &mut Context<T>,
    set: fn(&mut T, f32),
) -> Stateful<Div> {
    let palette = Palette::get();
    let fraction_at = {
        let bounds = Rc::clone(bounds);
        move |x: Pixels| {
            let area = bounds.get();
            if area.size.width <= Pixels::ZERO { 0.0 } else { ((x - area.origin.x) / area.size.width).clamp(0.0, 1.0) }
        }
    };
    let pressed = fraction_at.clone();
    let dragged = fraction_at;
    let measure = Rc::clone(bounds);
    let fraction = fraction.clamp(0.0, 1.0);
    let window =
        mark.filter(|(start, end)| end > start).map(|(start, end)| (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0)));
    let mut bar = div()
        .id(id)
        .relative()
        .h(px(10.0))
        .rounded_full()
        .bg(palette.raised)
        .cursor_pointer()
        .child(canvas(move |area, _, _| measure.set(area), |_, (), _, _| {}).absolute().size_full());
    if let Some((start, end)) = window {
        bar = bar.child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(relative(start))
                .w(relative(end - start))
                .rounded_full()
                .bg(palette.loop_band),
        );
    }
    bar = bar.child(div().absolute().top_0().left_0().h_full().w(relative(fraction)).rounded_full().bg(palette.accent));
    if let Some((start, end)) = window.filter(|(start, _)| *start < fraction) {
        bar = bar.child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(relative(start))
                .w(relative(end.min(fraction) - start))
                .bg(palette.loop_band_played),
        );
    }
    bar = bar.child(
        div()
            .absolute()
            .top(px(-2.0))
            .left(relative(fraction))
            .ml(px(-7.0))
            .size(px(14.0))
            .rounded_full()
            .border_1()
            .border_color(palette.border_strong)
            .bg(palette.text),
    );
    if let Some((start, end)) = window {
        for edge in [start, end] {
            let color = if edge <= fraction { palette.loop_mark_played } else { palette.loop_mark };
            bar = bar.child(div().absolute().top_0().h_full().left(relative(edge)).w(px(2.0)).bg(color));
        }
    }
    bar.on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            set(this, pressed(event.position.x));
            cx.notify();
        }),
    )
    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
        if event.dragging() {
            set(this, dragged(event.position.x));
            cx.notify();
        }
    }))
}

/// A level meter for a peak of `0.0` to `1.0` and beyond.
pub fn meter(level: f32) -> Div {
    let palette = Palette::get();
    let color = match level {
        l if l > 0.9 => palette.danger,
        l if l > 0.6 => palette.warning,
        _ => palette.success,
    };
    div()
        .h(px(6.0))
        .rounded_full()
        .bg(palette.raised)
        .child(div().h_full().w(relative(level.clamp(0.0, 1.0))).rounded_full().bg(color))
}

/// The spectrum: rounded bars in a subtle blue gradient with a thin cap on each peak.
pub fn spectrum(levels: Vec<f32>, caps: Vec<f32>) -> Div {
    let palette = Palette::get();
    let cap_color = palette.muted;
    div().h(px(112.0)).rounded(px(RADIUS)).bg(palette.bg).child(
        canvas(
            |area, _, _| area,
            move |_, area, window, _| {
                let bands = levels.len().max(1);
                let slot = area.size.width / bands as f32;
                let base = area.origin.y + area.size.height * 0.88;
                let rise = area.size.height * 0.8;
                for (band, (&level, &cap)) in levels.iter().zip(&caps).enumerate() {
                    let x = area.origin.x + slot * band as f32 + slot * 0.16;
                    let width = slot * 0.68;
                    if level >= 0.02 {
                        let height = (rise * level).max(px(3.0));
                        let mut bar =
                            fill(Bounds::new(point(x, base - height), size(width, height)), Palette::spectrum(0.0));
                        bar.corner_radii = (width * 0.25).into();
                        window.paint_quad(bar);
                    }
                    if cap > 0.03 {
                        let y = base - rise * cap - px(6.0);
                        let mut dot = fill(Bounds::new(point(x, y), size(width, px(2.0))), cap_color);
                        dot.corner_radii = px(1.0).into();
                        window.paint_quad(dot);
                    }
                }
            },
        )
        .size_full(),
    )
}
